use super::messages::{FeedMsg, TelemetryMsg};
use crate::UiMsg;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::thread;
use std::time::Duration;

#[derive(Default)]
pub struct FeedStats {
    pub dropped_samples: AtomicU64,
    pub decode_errors: AtomicU64,
}

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
            stats.dropped_samples.fetch_add(1, Ordering::Relaxed);
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

fn decode_and_send_line(line: &str, tx: &SyncSender<UiMsg>, stats: &FeedStats) -> bool {
    match serde_json::from_str::<FeedMsg>(line) {
        Ok(msg) => send_feed_msg(tx, stats, msg),
        Err(_) => match serde_json::from_str::<TelemetryMsg>(line) {
            Ok(msg) => try_send_ui_msg(tx, stats, UiMsg::Sample(msg)),
            Err(_) => {
                stats.decode_errors.fetch_add(1, Ordering::Relaxed);
                try_send_ui_msg(tx, stats, UiMsg::Status("feed decode error".to_string()))
            }
        },
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
    let reader = BufReader::new(stream);

    for line in reader.lines() {
        let Ok(line) = line else {
            let _ = tx.try_send(UiMsg::Status("feed closed".to_string()));
            break;
        };

        if !decode_and_send_line(&line, &tx, &stats) {
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
        let reader = BufReader::new(stream);

        for line in reader.lines() {
            let Ok(line) = line else {
                let _ = tx.try_send(UiMsg::Status(
                    "collector feed disconnected, reconnecting".to_string(),
                ));
                break;
            };

            if !decode_and_send_line(&line, &tx, &stats) {
                return;
            }
        }

        thread::sleep(Duration::from_millis(300));
    }
}
