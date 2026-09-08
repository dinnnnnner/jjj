use crate::domain::alarm_sync::{AlarmSnapshot, AlarmUpdate};
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
    /// Alarm transition versioned atomically with shared state.
    Alarm(AlarmUpdate),
}

/// One ordered stream for alarm history and state repair, shared by all stages.
#[derive(Clone, Debug)]
pub enum AlarmFeedEvent {
    Update(AlarmUpdate),
    Snapshot(AlarmSnapshot),
}

/// In-process pub/sub bus.
///
/// Note: broadcast channel is bounded; if a subscriber is too slow,
/// it will lag and may lose older messages.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<AppEvent>,
    alarm_tx: broadcast::Sender<AlarmFeedEvent>,
    active_alarms: Arc<std::sync::Mutex<AlarmState>>,
}

struct AlarmState {
    epoch: uuid::Uuid,
    revision: u64,
    active: HashMap<(String, String), AlarmEvent>,
}

impl Default for AlarmState {
    fn default() -> Self {
        Self {
            epoch: uuid::Uuid::new_v4(),
            revision: 0,
            active: HashMap::new(),
        }
    }
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        let (alarm_tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            alarm_tx,
            active_alarms: Arc::default(),
        }
    }

    pub fn publish(&self, evt: AppEvent) {
        match evt {
            AppEvent::Device(DeviceEvent::AlarmRaised(mut alarm)) => {
                alarm.cleared = false;
                self.publish_alarm(alarm);
            }
            AppEvent::Device(DeviceEvent::AlarmCleared(mut alarm)) => {
                alarm.cleared = true;
                self.publish_alarm(alarm);
            }
            other => {
                let _ = self.tx.send(other);
            }
        }
    }

    fn publish_alarm(&self, alarm: AlarmEvent) {
        let mut state = self.active_alarms.lock().unwrap_or_else(|e| e.into_inner());
        state.revision = state
            .revision
            .checked_add(1)
            .expect("alarm revision exhausted");
        let key = (alarm.device_id.clone(), alarm.alarm_id.clone());
        if alarm.cleared {
            state.active.remove(&key);
        } else {
            state.active.insert(key, alarm.clone());
        }
        // Assign revisions, update truth and publish in the same critical section.
        let update = AlarmUpdate {
            epoch: state.epoch,
            revision: state.revision,
            event: alarm,
        };
        let _ = self.alarm_tx.send(AlarmFeedEvent::Update(update.clone()));
        let _ = self.tx.send(AppEvent::Alarm(update));
    }

    pub fn alarm_snapshot(&self) -> AlarmSnapshot {
        let state = self.active_alarms.lock().unwrap_or_else(|e| e.into_inner());
        AlarmSnapshot {
            epoch: state.epoch,
            revision: state.revision,
            alarms: state.active.values().cloned().collect(),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.tx.subscribe()
    }

    pub fn subscribe_alarm_feed(&self) -> broadcast::Receiver<AlarmFeedEvent> {
        self.alarm_tx.subscribe()
    }

    /// Queue the snapshot under the same lock as transitions. Reading state and
    /// sending later would let a snapshot overtake otherwise deliverable history.
    pub fn publish_alarm_snapshot(&self) {
        let state = self.active_alarms.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = AlarmSnapshot {
            epoch: state.epoch,
            revision: state.revision,
            alarms: state.active.values().cloned().collect(),
        };
        let _ = self.alarm_tx.send(AlarmFeedEvent::Snapshot(snapshot));
    }

    /// A processing stage has a separate event queue but shares alarm truth.
    pub fn processing_stage(&self, capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self {
            tx,
            alarm_tx: self.alarm_tx.clone(),
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
        let raised = rx.try_recv().unwrap();
        assert!(
            matches!(&raised, AppEvent::Alarm(update) if update.revision == 1 && !update.event.cleared)
        );
        assert_eq!(stage.alarm_snapshot().alarms.len(), 1);
        alarm.cleared = true;
        raw.publish(AppEvent::Device(DeviceEvent::AlarmCleared(alarm.clone())));
        let cleared = rx.try_recv().unwrap();
        assert!(
            matches!(&cleared, AppEvent::Alarm(update) if update.revision == 2 && update.event.cleared)
        );
        raw.publish(AppEvent::System("overflow 1".into()));
        raw.publish(AppEvent::System("overflow 2".into()));
        assert!(matches!(
            rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        let snapshot = stage.alarm_snapshot();
        assert_eq!(snapshot.revision, 2);
        let mut stage_rx = stage.subscribe();
        stage.publish_forwarded(raised);
        let AppEvent::Alarm(delayed) = stage_rx.try_recv().unwrap() else {
            panic!("expected forwarded alarm update")
        };
        assert_eq!(delayed.epoch, snapshot.epoch);
        assert_eq!(delayed.revision, 1);
        let mut tracker = crate::domain::alarm_sync::AlarmTracker::default();
        tracker.apply_snapshot(&snapshot);
        assert!(!tracker.apply_update(&delayed));
        assert_eq!(tracker.active().count(), 0);
        assert!(stage.alarm_snapshot().alarms.is_empty());
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
