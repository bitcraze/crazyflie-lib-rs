mod log;
mod param;

pub use crate::log::Log;
pub use crate::param::Param;

use std::{collections::HashMap, sync::Arc};
use crazyflie_link::Packet;
use flume as channel;
use flume::{Sender, Receiver};
use async_trait::async_trait;

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

        // Create subsystems one by one
        // The future is passed to join!() later down so that all modules initializes at the same time
        let log_downlink = dispatcher.get_port_receiver(5).unwrap();
        let log = Log::new(log_downlink, uplink.clone());

        let param_downlink = dispatcher.get_port_receiver(12).unwrap();
        let param = Param::new(param_downlink, uplink.clone());

        // Start the downlink packet dispatcher
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

#[async_trait]
trait WaitForPacket {
    async fn wait_packet(&self, port: u8, channel: u8, data_prefix: &[u8]) -> Packet;
}

#[async_trait]
impl WaitForPacket for channel::Receiver<Packet> {
    async fn wait_packet(&self, port: u8, channel: u8, data_prefix: &[u8]) -> Packet {
        let mut pk = self.recv_async().await.unwrap();

        loop {
            if pk.get_port() == port && pk.get_channel() == channel && pk.get_data().starts_with(data_prefix) {
                break;
            }
            pk = self.recv_async().await.unwrap();
        }

        pk
    }
}
