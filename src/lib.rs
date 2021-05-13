use std::{collections::HashMap, convert::TryInto, sync::Arc};
use async_std::future;
use crazyflie_link::Packet;
use flume as channel;
use flume::{Sender, Receiver};
use futures::channel::oneshot::channel;

pub struct Crazyflie {
    uplink: channel::Sender<Packet>,
    pub log: Log,
    pub param: Param,
}

impl Crazyflie {
    pub async fn connect_from_uri(link_context: &crazyflie_link::LinkContext, uri: &str) -> Self {

        let link = link_context.open_link(uri).await.unwrap();

        Self::connect_from_link(link).await
    }

    pub async fn connect_from_link(link: crazyflie_link::Connection) -> Self {

        // Downlink dispatcher
        let link = Arc::new(link);
        let mut dispatcher = CrtpDispatch::new(link.clone());

        // Uplink queue
        let (uplink, rx) = channel::unbounded();
        async_std::task::spawn(async move {
            loop {
                if let Ok(pk) = rx.recv_async().await {
                    if let Err(_) = link.send_packet(pk).await {
                        break;
                    }
                } else {
                    break;
                }
                
            }
        });

        // Create subsystem one by one
        // The future is passed to join!() later down so that all modules initializes at the same time

        let log_downlink = dispatcher.get_port_receiver(5).unwrap();
        let log = Log::new(log_downlink, uplink.clone());

        let param_downlink = dispatcher.get_port_receiver(12).unwrap();
        let param = Param::new(param_downlink, uplink.clone());

        dispatcher.run().await;

        // Intitialize all modules in parallel
        let (log, param) = futures::join!(log, param);

        Crazyflie {
            uplink,
            log,
            param
        }
    }


}

struct CrtpDispatch {
    link: Arc<crazyflie_link::Connection>,
    // port_callbacks: [Arc<Mutex<Option<Sender<Packet>>>>; 15]
    port_channels: HashMap<u8, Sender<Packet>>,
}

impl CrtpDispatch {
    fn new(link: Arc<crazyflie_link::Connection>) -> Self {
        CrtpDispatch {
            link,
            port_channels: HashMap::new(),
        }
    }

    fn get_port_receiver(&mut self, port: u8) -> Option<Receiver<Packet>> {
        if self.port_channels.contains_key(&port) {
            None
        } else {
            let (tx, rx) = channel::unbounded();
            self.port_channels.insert(port, tx);
            Some(rx)
        }
    }

    async fn run(self) {
        let link = self.link.clone();
        async_std::task::spawn(async move {
            loop {
                let packet = link.recv_packet().await.unwrap();
                if packet.get_port() < 16 {
                    let channel = self.port_channels.get(&packet.get_port());  // get(packet.get_port()).lock().await;
                    if let Some(channel) = channel.as_ref() {
                        let _ = channel.send_async(packet).await;
                    }
                }
            }
        });
    }
}

pub struct Log{
    uplink: channel::Sender<Packet>,
    toc_downlink: channel::Receiver<Packet>,
    toc: HashMap<(String, String), u16>,
}

impl Log {
    async fn new(downlink: channel::Receiver<Packet>, uplink: channel::Sender<Packet>) -> Self {

        let (tx, toc_downlink) = channel::unbounded();
        async_std::task::spawn(async move {
            loop {
                let packet = downlink.recv_async().await;
                if let Ok(packet) = packet {
                    if packet.get_channel() == 0 {
                        let _ = tx.send_async(packet).await;
                    }
                } else {
                    break;
                }
            }
        });

        let mut log = Self {
            uplink,
            toc_downlink,
            toc: HashMap::new(),
        };

        log.fetch_toc().await;

        log
    }

    async fn fetch_toc(&mut self) {
        println!("Sending log request ...");
        let pk = Packet::new(5, 0, vec![0x03]);
        self.uplink.send_async(pk).await.unwrap();

        let mut pk = self.toc_downlink.recv_async().await.unwrap();

        loop {
            if pk.get_port() == 0x05 && pk.get_channel() == 0 && pk.get_data()[0] == 0x03 {
                break;
            }
            pk = self.toc_downlink.recv_async().await.unwrap();
        }

        let toc_len = u16::from_le_bytes(pk.get_data()[1..3].try_into().unwrap());

        println!("Log len: {}", toc_len);


        for i in 0..toc_len {
            let pk = Packet::new(5, 0, vec![0x02, (i&0x0ff) as u8, (i>>8) as u8]);
            self.uplink.send_async(pk).await.unwrap();
            
            let mut pk = self.toc_downlink.recv_async().await.unwrap();

            loop {
                if pk.get_port() == 0x05 && pk.get_channel() == 0 && pk.get_data()[0] == 0x02 {
                    break;
                }
                pk = self.toc_downlink.recv_async().await.unwrap();
            }

            let mut strings = pk.get_data()[4..].split(|b| *b == 0);
            let group = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));
            let name = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));
            println!("{}.{}", group, name);
        }
    }
}

pub struct Param{
    uplink: channel::Sender<Packet>,
    downlink: channel::Receiver<Packet>,
    toc: Vec<String>,
}

impl Param {
    async fn new(downlink: channel::Receiver<Packet>, uplink: channel::Sender<Packet>) -> Self {
        let mut param = Self {
            uplink,
            downlink,
            toc: Vec::new(),
        };

        param.fetch_toc().await;

        param
    }

    async fn fetch_toc(&mut self) {

    }
}