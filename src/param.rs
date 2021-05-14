use crazyflie_link::Packet;
use flume as channel;
use std::collections::HashMap;

#[derive(Debug)]
enum ParamItemType {
    PARAM,
}

impl From<u8> for ParamItemType {
    fn from(_: u8) -> Self {
        Self::PARAM
    }
}

pub struct Param {
    uplink: channel::Sender<Packet>,
    toc: HashMap<String, (u16, ParamItemType)>,
}

const PARAM_PORT: u8 = 2;

impl Param {
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

        let toc = crate::fetch_toc(PARAM_PORT, uplink.clone(), toc_downlink).await;
        dbg!(&toc);

        let param = Self { uplink, toc };

        param
    }
}
