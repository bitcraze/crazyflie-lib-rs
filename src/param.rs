
use flume as channel;
use crazyflie_link::Packet;

pub struct Param{
    uplink: channel::Sender<Packet>,
    downlink: channel::Receiver<Packet>,
    toc: Vec<String>,
}

impl Param {
    pub(crate) async fn new(downlink: channel::Receiver<Packet>, uplink: channel::Sender<Packet>) -> Self {
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
