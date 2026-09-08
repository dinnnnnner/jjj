use demo2::domain::AlarmEvent;
use demo2::domain::alarm_sync::{AlarmSnapshot, AlarmUpdate};
use demo2::ingress::can::enqueue_can_tx;
use demo2::transport::can::CanTxFrame;
use eframe::egui;
use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[path = "collector_service.rs"]
mod embedded_collector_service;
#[path = "../ui_client/mod.rs"]
mod ui_client;

use ui_client::feed::FeedStats;
use ui_client::messages::TelemetryMsg;
pub(crate) use ui_client::models::*;
use ui_client::series::SensorSeries;
pub(crate) use ui_client::settings::*;
use ui_client::state::UiClientState;
pub(crate) use ui_client::time::*;

#[derive(Debug)]
enum UiMsg {
    Status(String),
    Sample(TelemetryMsg),
    Alarm(AlarmUpdate),
    AlarmSnapshot(AlarmSnapshot),
    CanReplayLoaded(ReplayMode, Result<CanReplayData, String>),
    CanReplayExported(Result<String, String>),
    AlarmRecordsLoaded(Result<AlarmRecordData, String>),
}

struct UiClientApp {
    rx: Receiver<UiMsg>,
    ui_tx: SyncSender<UiMsg>,
    feed_stats: Arc<FeedStats>,
    feed_addr: String,
    control_addr: String,
    state: UiClientState,
}

impl Deref for UiClientApp {
    type Target = UiClientState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for UiClientApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl UiClientApp {
    const MAX_ALARM_HISTORY: usize = 50;
    const DEMO_ALARM_ID: &'static str = "demo_alarm_bit";

    fn default_window_pos(index: usize) -> egui::Pos2 {
        let col = (index % 4) as f32;
        let row = (index / 4) as f32;
        egui::pos2(180.0 + col * (300.0 + 20.0), 220.0 + row * (200.0 + 50.0))
    }

    fn new(
        rx: Receiver<UiMsg>,
        ui_tx: SyncSender<UiMsg>,
        feed_stats: Arc<FeedStats>,
        feed_addr: String,
        control_addr: String,
        pg_dsn: String,
    ) -> Self {
        Self {
            rx,
            ui_tx,
            feed_stats,
            feed_addr,
            control_addr,
            state: UiClientState::new(pg_dsn, Self::MAX_ALARM_HISTORY),
        }
    }

    fn reset_layout(&mut self) {
        for (idx, window) in self.dynamic_windows.iter_mut().enumerate() {
            window.position = Self::default_window_pos(idx);
            window.scale = 1.0;
            window.rect = None;
        }
    }
}

fn main() -> eframe::Result<()> {
    ui_client::runtime::run_collector_then_ui()
}
