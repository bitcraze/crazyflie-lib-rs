use crazyflie_link::Packet;
use flume as channel;
use std::collections::BTreeMap;

#[repr(u8)]
#[derive(Debug)]
enum LogItemType {
    Float = 0,
}

impl From<u8> for LogItemType {
    fn from(_: u8) -> Self {
        LogItemType::Float
    }
}

#[derive(Debug)]
pub struct Log {
    uplink: channel::Sender<Packet>,
    toc: BTreeMap<String, (u16, LogItemType)>,
}

const LOG_PORT: u8 = 5;

impl Log {
    pub(crate) async fn new(
        downlink: channel::Receiver<Packet>,
        uplink: channel::Sender<Packet>,
    ) -> Self {
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

        let toc = crate::fetch_toc(LOG_PORT, uplink.clone(), toc_downlink).await;

        Self { uplink, toc }
    }
}
