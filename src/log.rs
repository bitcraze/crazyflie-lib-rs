
use flume as channel;
use crazyflie_link::Packet;
use std::collections::HashMap;
use std::convert::TryInto;
use async_trait::async_trait;
use crate::WaitForPacket;

pub struct Log{
    uplink: channel::Sender<Packet>,
    toc_downlink: channel::Receiver<Packet>,
    toc: HashMap<(String, String), u16>,
}

const LOG_PORT: u8 = 5;
const LOG_CHANNEL_TOC: u8 = 0;

const LOG_TOC_GET_ITEM: u8 = 0x02;
const LOG_TOC_GET_INFO: u8 = 0x03;



impl Log {
    pub(crate) async fn new(downlink: channel::Receiver<Packet>, uplink: channel::Sender<Packet>) -> Self {

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

    pub(crate) async fn fetch_toc(&mut self) {
        println!("Sending log request ...");
        let pk = Packet::new(5, 0, vec![LOG_TOC_GET_INFO]);
        self.uplink.send_async(pk).await.unwrap();

        let pk = self.toc_downlink.wait_packet(LOG_PORT, LOG_CHANNEL_TOC, &[LOG_TOC_GET_INFO]).await;

        let toc_len = u16::from_le_bytes(pk.get_data()[1..3].try_into().unwrap());

        println!("Log len: {}", toc_len);


        for i in 0..toc_len {
            let pk = Packet::new(5, 0, vec![LOG_TOC_GET_ITEM, (i&0x0ff) as u8, (i>>8) as u8]);
            self.uplink.send_async(pk).await.unwrap();

            let pk = self.toc_downlink.wait_packet(5, 0, &[LOG_TOC_GET_ITEM]).await;

            let mut strings = pk.get_data()[4..].split(|b| *b == 0);
            let group = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));
            let name = String::from_utf8_lossy(strings.next().expect("TOC packet format error"));

            println!("{}.{}", &group, &name);
            self.toc.insert((group.into(), name.into()), 0);
        }
    }
}
