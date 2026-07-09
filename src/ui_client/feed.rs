use super::messages::FeedMsg;
use crate::UiMsg;
use std::io::{self, BufReader, Read};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

#[derive(Default)]
pub struct FeedStats {
    pub dropped_messages: AtomicU64,
    pub decode_errors: AtomicU64,
}

const MAX_FEED_FRAME_LEN: usize = 1024 * 1024;

fn send_feed_msg(tx: &SyncSender<UiMsg>, stats: &FeedStats, msg: FeedMsg) -> bool {
    let ui_msg = match msg {
        FeedMsg::Telemetry(msg) => UiMsg::Sample(msg),
        FeedMsg::Alarm(alarm) => UiMsg::Alarm(alarm),
        FeedMsg::Status(status) => UiMsg::Status(status),
    };
    try_send_ui_msg(tx, stats, ui_msg)
}

fn try_send_ui_msg(tx: &SyncSender<UiMsg>, stats: &FeedStats, msg: UiMsg) -> bool {
    match tx.try_send(msg) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            stats.dropped_messages.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

fn read_feed_frame(reader: &mut BufReader<TcpStream>) -> io::Result<Vec<u8>> {
    let mut len_buf = [0_u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FEED_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "feed frame too large",
        ));
    }

    let mut frame = vec![0_u8; len];
    reader.read_exact(&mut frame)?;
    Ok(frame)
}

fn decode_and_send_frame(frame: &[u8], tx: &SyncSender<UiMsg>, stats: &FeedStats) -> bool {
    match bincode::deserialize::<FeedMsg>(frame) {
        Ok(msg) => send_feed_msg(tx, stats, msg),
        Err(_) => {
            stats.decode_errors.fetch_add(1, Ordering::Relaxed);
            try_send_ui_msg(tx, stats, UiMsg::Status("feed decode error".to_string()))
        }
    }
}

#[allow(dead_code)]
pub fn feed_thread(feed_addr: String, tx: SyncSender<UiMsg>, stats: Arc<FeedStats>) {
    let stream = match TcpStream::connect(&feed_addr) {
        Ok(s) => s,
        Err(err) => {
            let _ = tx.try_send(UiMsg::Status(format!("connect failed: {err}")));
            return;
        }
    };

    let _ = tx.try_send(UiMsg::Status(format!("connected to {feed_addr}")));
    let mut reader = BufReader::new(stream);

    loop {
        let frame = match read_feed_frame(&mut reader) {
            Ok(frame) => frame,
            Err(_) => {
                let _ = tx.try_send(UiMsg::Status("feed closed".to_string()));
                break;
            }
        };

        if !decode_and_send_frame(&frame, &tx, &stats) {
            return;
        }
    }
}

pub fn resilient_feed_thread(feed_addr: String, tx: SyncSender<UiMsg>, stats: Arc<FeedStats>) {
    loop {
        let stream = match TcpStream::connect(&feed_addr) {
            Ok(s) => s,
            Err(err) => {
                let _ = tx.try_send(UiMsg::Status(format!("waiting for collector: {err}")));
                thread::sleep(Duration::from_millis(500));
                continue;
            }
        };

        let _ = tx.try_send(UiMsg::Status(format!(
            "connected to collector feed: {feed_addr}"
        )));
        let mut reader = BufReader::new(stream);

        loop {
            let frame = match read_feed_frame(&mut reader) {
                Ok(frame) => frame,
                Err(_) => {
                    let _ = tx.try_send(UiMsg::Status(
                        "collector feed disconnected, reconnecting".to_string(),
                    ));
                    break;
                }
            };

            if !decode_and_send_frame(&frame, &tx, &stats) {
                return;
            }
        }

        thread::sleep(Duration::from_millis(300));
    }
}
