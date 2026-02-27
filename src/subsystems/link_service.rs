//! # Link layer services
//!
//! Implementation of the [CRTP link service](https://www.bitcraze.io/documentation/repository/crazyflie-firmware/master/functional-areas/crtp/crtp_link/)
//! on port 15. The link service provides:
//!
//! - **Echo** (channel 0): packets sent are echoed back unaltered, used for latency and bandwidth measurement
//! - **Source** (channel 1): responds with a 32-byte identification string
//! - **Sink** (channel 2): packets are dropped and ignored

use crate::crazyflie::LINK_PORT;
use crate::{Error, Result};
use crazyflie_link::Packet;
use flume as channel;
use futures::lock::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};

const ECHO_CHANNEL: u8 = 0;
const SOURCE_CHANNEL: u8 = 1;
const SINK_CHANNEL: u8 = 2;

/// Max CRTP data payload
const MAX_DATA_SIZE: usize = 30;

/// Fill byte used for bandwidth test payloads
const FILL_PATTERN: u8 = 0xAA;

/// A snapshot of radio link statistics
#[derive(Debug, Clone)]
pub struct Statistics {
    /// ACK success rate (0.0 to 1.0). `None` for USB connections.
    pub link_quality: Option<f32>,
    /// Data packets sent per second (excludes null/keepalive packets). `None` for USB connections.
    pub uplink_rate: Option<f32>,
    /// Packets received per second. `None` for USB connections.
    pub downlink_rate: Option<f32>,
    /// Total radio packets sent per second (data + null/keepalive). `None` for USB connections.
    pub radio_send_rate: Option<f32>,
    /// Average retries per acknowledged packet. `None` for USB connections.
    pub avg_retries: Option<f32>,
    /// Fraction of ACKs where the nRF24 power detector triggered (0.0 to 1.0). `None` for USB connections.
    pub power_detector_rate: Option<f32>,
    /// Average RSSI in dBm measured by the radio dongle on received ACK packets.
    /// `None` if the radio doesn't support RSSI.
    pub rssi: Option<f32>,
}

/// Result of a bandwidth test
#[derive(Debug, Clone)]
pub struct BandwidthResult {
    /// Uplink throughput in bytes per second
    pub uplink_bytes_per_sec: f64,
    /// Downlink throughput in bytes per second
    pub downlink_bytes_per_sec: f64,
    /// Round-trip packet rate achieved during the test
    pub packets_per_sec: f64,
}

/// Access to link layer services
///
/// Provides on-demand echo/ping for latency measurement, bandwidth testing,
/// and exposes radio-level link quality metrics from the underlying connection.
///
/// See the [link_service module documentation](crate::subsystems::link_service) for
/// more context and information.
pub struct LinkService {
    uplink: channel::Sender<Packet>,
    echo_downlink: Mutex<channel::Receiver<Packet>>,
    source_downlink: Mutex<channel::Receiver<Packet>>,
    link: Arc<crazyflie_link::Connection>,
}

impl LinkService {
    pub(crate) fn new(
        uplink: channel::Sender<Packet>,
        downlink: channel::Receiver<Packet>,
        link: Arc<crazyflie_link::Connection>,
    ) -> Self {
        let (echo_downlink, source_downlink, _, _) =
            crate::crtp_utils::crtp_channel_dispatcher(downlink);

        Self {
            uplink,
            echo_downlink: Mutex::new(echo_downlink),
            source_downlink: Mutex::new(source_downlink),
            link,
        }
    }

    /// Send a ping and return the round-trip time in milliseconds
    ///
    /// Sends an echo packet on the link service port and waits for the
    /// Crazyflie to echo it back. Returns the measured round-trip time.
    ///
    /// Note that no other packets should be sent on the echo channel while a ping is in flight,
    /// as this may interfere with the measurement or cause the function to return a protocol error.
    pub async fn ping(&self) -> Result<f64> {
        let echo_downlink = self.echo_downlink.lock().await;
        const PING_PAYLOAD: [u8; 1] = [0x01];
        let start = Instant::now();

        let pk = Packet::new(LINK_PORT, ECHO_CHANNEL, PING_PAYLOAD.to_vec());
        self.uplink
            .send_async(pk)
            .await
            .map_err(|_| Error::Disconnected)?;

        let answer = tokio::time::timeout(Duration::from_secs(1), echo_downlink.recv_async())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Disconnected)?;

        if answer.get_data() != &PING_PAYLOAD {
            return Err(Error::ProtocolError("Ping got wrong echo back".to_string()));
        }

        Ok(start.elapsed().as_secs_f64() * 1000.0)
    }

    /// Test uplink bandwidth using the sink channel
    ///
    /// Sends `n_packets` max-size packets to the sink channel (which drops them)
    /// as fast as possible. Uses the echo channel to measure the time taken to send all
    /// packets and receive the final echo response,
    ///
    /// Returns the measured uplink throughput in bytes per second.
    pub async fn test_uplink_bandwidth(&self, n_packets: u64) -> Result<f64> {
        let data = vec![FILL_PATTERN; MAX_DATA_SIZE];
        let start = Instant::now();
        let mut total_bytes: u64 = 0;

        for _ in 0..n_packets {
            // Send a sink packet (Crazyflie drops it, but the radio ACKs it)
            let pk = Packet::new(LINK_PORT, SINK_CHANNEL, data.clone());
            self.uplink
                .send_async(pk)
                .await
                .map_err(|_| Error::Disconnected)?;
            total_bytes += MAX_DATA_SIZE as u64;
        }

        // Send an echo to detect the end of the test — wait for the round trip,
        // will only happen when all previous packets have been sent and ACKed by the radio
        const ECHO_PAYLOAD: [u8; 1] = [0x00];
        let echo = Packet::new(LINK_PORT, ECHO_CHANNEL, ECHO_PAYLOAD.to_vec());
        let echo_downlink = self.echo_downlink.lock().await;
        self.uplink
            .send_async(echo)
            .await
            .map_err(|_| Error::Disconnected)?;
        let answer = tokio::time::timeout(Duration::from_secs(10), echo_downlink.recv_async())
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Disconnected)?;

        if answer.get_data() != &ECHO_PAYLOAD {
            return Err(Error::ProtocolError(
                "Echo got wrong payload back".to_string(),
            ));
        }

        let elapsed = start.elapsed().as_secs_f64();
        Ok(total_bytes as f64 / elapsed)
    }

    /// Test downlink bandwidth using the source channel
    ///
    /// Sends `n_packets` requests to the source channel as fast as possible.
    /// The Crazyflie responds to each request with a 32-byte packet.
    ///
    /// Returns the measured downlink throughput in bytes per second.
    pub async fn test_downlink_bandwidth(&self, n_packets: u64) -> Result<f64> {
        let source_downlink = self.source_downlink.lock().await;
        let start = Instant::now();
        let mut total_bytes: u64 = 0;

        for _ in 0..n_packets {
            let pk = Packet::new(LINK_PORT, SOURCE_CHANNEL, vec![0x00]);
            self.uplink
                .send_async(pk)
                .await
                .map_err(|_| Error::Disconnected)?;
        }

        for _ in 0..n_packets {
            let response =
                tokio::time::timeout(Duration::from_secs(1), source_downlink.recv_async())
                    .await
                    .map_err(|_| Error::Timeout)?
                    .map_err(|_| Error::Disconnected)?;

            total_bytes += response.get_data().len() as u64;
        }

        let elapsed = start.elapsed().as_secs_f64();
        Ok(total_bytes as f64 / elapsed)
    }

    /// Test round-trip bandwidth using the echo channel
    ///
    /// Sends `n_packets` max-size packets to the echo channel and waits for each response.
    /// This measures the achievable throughput when both uplink and downlink
    /// carry full payloads.
    pub async fn test_echo_bandwidth(&self, n_packets: u64) -> Result<BandwidthResult> {
        let echo_downlink = self.echo_downlink.lock().await;
        let data = vec![FILL_PATTERN; MAX_DATA_SIZE];
        let start = Instant::now();
        let mut packets: u64 = 0;

        for _ in 0..n_packets {
            let pk = Packet::new(LINK_PORT, ECHO_CHANNEL, data.clone());
            self.uplink
                .send_async(pk)
                .await
                .map_err(|_| Error::Disconnected)?;
        }

        for _ in 0..n_packets {
            let answer = tokio::time::timeout(Duration::from_secs(1), echo_downlink.recv_async())
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(|_| Error::Disconnected)?;

            if answer.get_data() != data.as_slice() {
                return Err(Error::ProtocolError(
                    "Echo got wrong payload back".to_string(),
                ));
            }

            packets += 1;
        }

        let elapsed = start.elapsed().as_secs_f64();
        let bytes = packets as f64 * MAX_DATA_SIZE as f64;

        Ok(BandwidthResult {
            uplink_bytes_per_sec: bytes / elapsed,
            downlink_bytes_per_sec: bytes / elapsed,
            packets_per_sec: packets as f64 / elapsed,
        })
    }

    /// Get a snapshot of current radio link statistics
    pub async fn get_statistics(&self) -> Statistics {
        let radio_stats = self.link.link_statistics().await;

        Statistics {
            link_quality: radio_stats.as_ref().map(|s| s.link_quality),
            uplink_rate: radio_stats.as_ref().map(|s| s.uplink_rate),
            downlink_rate: radio_stats.as_ref().map(|s| s.downlink_rate),
            radio_send_rate: radio_stats.as_ref().map(|s| s.radio_send_rate),
            avg_retries: radio_stats.as_ref().map(|s| s.avg_retries),
            power_detector_rate: radio_stats.as_ref().map(|s| s.power_detector_rate),
            rssi: radio_stats.and_then(|s| s.rssi),
        }
    }
}
