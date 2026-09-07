use crate::domain::{AlarmEvent, ConnState, DeviceId, DeviceSnapshot, RequestId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};

pub use crate::domain::telemetry::TelemetrySourceKind;

/// Device domain events emitted by session/transport/app layers.
#[derive(Clone, Debug)]
pub enum DeviceEvent {
    /// Connection state transition for a specific device.
    ConnStateChanged {
        device_id: DeviceId,
        from: ConnState,
        to: ConnState,
    },
    /// Full snapshot update (coarser-grained state sync).
    TelemetryUpdated {
        device_id: DeviceId,
        snapshot: DeviceSnapshot,
    },
    /// Single telemetry sample (fine-grained stream for charts/alerts).
    TelemetrySample {
        captured_at_ms: i64,
        device_id: DeviceId,
        sensor_id: usize,
        t_sec: f64,
        value: f64,
        req_id: RequestId,
        alarm_bit: bool,
        source_kind: TelemetrySourceKind,
    },
    /// Alarm lifecycle events.
    AlarmRaised(AlarmEvent),
    AlarmCleared(AlarmEvent),
    /// Command execution result.
    CommandResult {
        device_id: DeviceId,
        request_id: RequestId,
        ok: bool,
    },
    /// Structured runtime log for UI/diagnostics.
    Log {
        device_id: DeviceId,
        level: &'static str,
        msg: String,
    },
}

/// App-level event envelope.
#[derive(Clone, Debug)]
pub enum AppEvent {
    Device(DeviceEvent),
    System(String),
}

/// In-process pub/sub bus.
///
/// Note: broadcast channel is bounded; if a subscriber is too slow,
/// it will lag and may lose older messages.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
    active_alarms: Arc<std::sync::Mutex<HashMap<(String, String), AlarmEvent>>>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            active_alarms: Arc::default(),
        }
    }

    pub fn publish(&self, evt: AppEvent) {
        // Update authoritative state before the lossy broadcast channel.
        match &evt {
            AppEvent::Device(DeviceEvent::AlarmRaised(alarm)) => {
                self.active_alarms
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(
                        (alarm.device_id.clone(), alarm.alarm_id.clone()),
                        alarm.clone(),
                    );
            }
            AppEvent::Device(DeviceEvent::AlarmCleared(alarm)) => {
                self.active_alarms
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&(alarm.device_id.clone(), alarm.alarm_id.clone()));
            }
            _ => {}
        }
        let _ = self.tx.send(evt);
    }

    pub fn active_alarms(&self) -> Vec<AlarmEvent> {
        self.active_alarms
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }

    /// A processing stage has a separate event queue but shares alarm truth.
    pub fn processing_stage(&self, capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            active_alarms: self.active_alarms.clone(),
        }
    }

    /// Forward an event whose alarm state was already recorded at ingress.
    pub fn publish_forwarded(&self, evt: AppEvent) {
        let _ = self.tx.send(evt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AlarmLevel;
    #[test]
    fn alarm_snapshot_survives_lag_and_stale_forwarding() {
        let raw = EventBus::new(1);
        let stage = raw.processing_stage(1);
        let mut rx = raw.subscribe();
        let mut alarm = AlarmEvent {
            device_id: "can://test:ch0".into(),
            alarm_id: "test".into(),
            level: AlarmLevel::Critical,
            message: "test".into(),
            raised_at: std::time::SystemTime::now(),
            cleared: false,
        };
        raw.publish(AppEvent::Device(DeviceEvent::AlarmRaised(alarm.clone())));
        assert_eq!(stage.active_alarms().len(), 1);
        alarm.cleared = true;
        raw.publish(AppEvent::Device(DeviceEvent::AlarmCleared(alarm.clone())));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        alarm.cleared = false;
        stage.publish_forwarded(AppEvent::Device(DeviceEvent::AlarmRaised(alarm)));
        assert!(stage.active_alarms().is_empty());
    }
}

/// Lightweight state store for latest device snapshots.
#[derive(Clone, Default)]
pub struct Store {
    snapshots: Arc<RwLock<HashMap<DeviceId, DeviceSnapshot>>>,
}

impl Store {
    pub async fn upsert_snapshot(&self, snapshot: DeviceSnapshot) {
        self.snapshots
            .write()
            .await
            .insert(snapshot.device_id.clone(), snapshot);
    }

    pub async fn snapshot(&self, device_id: &str) -> Option<DeviceSnapshot> {
        self.snapshots.read().await.get(device_id).cloned()
    }
}
