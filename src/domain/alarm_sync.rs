use super::AlarmEvent;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlarmUpdate {
    pub epoch: Uuid,
    pub revision: u64,
    pub event: AlarmEvent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AlarmSnapshot {
    pub epoch: Uuid,
    pub revision: u64,
    pub alarms: Vec<AlarmEvent>,
}

type AlarmKey = (String, String);

/// Merge snapshots and deltas even when processing stages deliver them late.
/// Snapshots arrive on the ordered collector feed; a new epoch snapshot starts
/// a new collector session. Deltas from other epochs are ignored.
#[derive(Default)]
pub struct AlarmTracker {
    epoch: Option<Uuid>,
    snapshot_revision: Option<u64>,
    revisions: HashMap<AlarmKey, u64>,
    active: HashMap<AlarmKey, AlarmEvent>,
}

impl AlarmTracker {
    pub fn epoch(&self) -> Option<Uuid> {
        self.epoch
    }

    pub fn active(&self) -> impl Iterator<Item = &AlarmEvent> {
        self.active.values()
    }

    pub fn apply_update(&mut self, update: &AlarmUpdate) -> bool {
        let epoch = *self.epoch.get_or_insert(update.epoch);
        if update.epoch != epoch || self.snapshot_revision.is_some_and(|r| update.revision <= r) {
            return false;
        }
        let key = (
            update.event.device_id.clone(),
            update.event.alarm_id.clone(),
        );
        if self
            .revisions
            .get(&key)
            .is_some_and(|r| update.revision <= *r)
        {
            return false;
        }
        self.revisions.insert(key.clone(), update.revision);
        if update.event.cleared {
            self.active.remove(&key);
        } else {
            self.active.insert(key, update.event.clone());
        }
        true
    }

    pub fn apply_snapshot(&mut self, snapshot: &AlarmSnapshot) -> bool {
        if self.epoch != Some(snapshot.epoch) {
            *self = Self {
                epoch: Some(snapshot.epoch),
                ..Self::default()
            };
        }
        if self
            .snapshot_revision
            .is_some_and(|r| snapshot.revision <= r)
        {
            return false;
        }
        // Keep deltas newer than this snapshot, including tombstones for clears.
        self.revisions.retain(|_, r| *r > snapshot.revision);
        self.active
            .retain(|key, _| self.revisions.contains_key(key));
        for alarm in &snapshot.alarms {
            let key = (alarm.device_id.clone(), alarm.alarm_id.clone());
            if !alarm.cleared && !self.revisions.contains_key(&key) {
                self.active.insert(key, alarm.clone());
            }
        }
        self.snapshot_revision = Some(snapshot.revision);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AlarmLevel;

    fn update(epoch: Uuid, revision: u64, id: &str, cleared: bool) -> AlarmUpdate {
        AlarmUpdate {
            epoch,
            revision,
            event: AlarmEvent {
                device_id: "can://test:ch0".into(),
                alarm_id: id.into(),
                cleared,
                level: AlarmLevel::Critical,
                message: id.into(),
                raised_at: std::time::SystemTime::UNIX_EPOCH,
            },
        }
    }

    #[test]
    fn newer_snapshot_rejects_delayed_raise_and_clear() {
        let epoch = Uuid::new_v4();
        let mut tracker = AlarmTracker::default();
        tracker.apply_snapshot(&AlarmSnapshot {
            epoch,
            revision: 2,
            alarms: vec![],
        });
        assert!(!tracker.apply_update(&update(epoch, 1, "a", false)));
        assert_eq!(tracker.active().count(), 0);
        tracker.apply_snapshot(&AlarmSnapshot {
            epoch,
            revision: 4,
            alarms: vec![update(epoch, 4, "a", false).event],
        });
        assert!(!tracker.apply_update(&update(epoch, 3, "a", true)));
        assert_eq!(tracker.active().count(), 1);
    }

    #[test]
    fn late_snapshot_preserves_newer_raise_and_clear_and_other_keys() {
        let epoch = Uuid::new_v4();
        let mut tracker = AlarmTracker::default();
        assert!(tracker.apply_update(&update(epoch, 4, "a", false)));
        assert!(tracker.apply_update(&update(epoch, 3, "b", true)));
        tracker.apply_snapshot(&AlarmSnapshot {
            epoch,
            revision: 2,
            alarms: vec![
                update(epoch, 2, "b", false).event,
                update(epoch, 1, "c", false).event,
            ],
        });
        let mut ids: Vec<_> = tracker.active().map(|a| a.alarm_id.as_str()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a", "c"]);
        assert!(!tracker.apply_update(&update(epoch, 3, "a", false)));
        assert!(!tracker.apply_snapshot(&AlarmSnapshot {
            epoch,
            revision: 1,
            alarms: vec![]
        }));
    }

    #[test]
    fn collector_restart_resets_revision_but_old_epoch_deltas_are_ignored() {
        let old = Uuid::new_v4();
        let new = Uuid::new_v4();
        let mut tracker = AlarmTracker::default();
        tracker.apply_update(&update(old, 100, "a", false));
        tracker.apply_snapshot(&AlarmSnapshot {
            epoch: new,
            revision: 0,
            alarms: vec![],
        });
        assert_eq!(tracker.active().count(), 0);
        assert!(!tracker.apply_update(&update(old, 101, "a", false)));
        assert!(tracker.apply_update(&update(new, 1, "a", false)));
        assert!(!tracker.apply_update(&update(new, 1, "a", false)));
    }
}
