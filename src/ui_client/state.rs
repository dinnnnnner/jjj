use crate::{
    AlarmRecordState, AlarmViewItem, CanAlarmThresholds, CanReplayState, DynamicSignalWindow,
    SENSOR_COUNT, SentAngleJumpThresholds, SentTorqueJumpThresholds, TestSignalView,
};
use demo2::signal::{SignalProcessor, default_signal_specs};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use super::series::SensorSeries;

pub(crate) struct UiClientState {
    pub(crate) status: String,
    pub(crate) sensors: Vec<SensorSeries>,
    pub(crate) tcp_sensors: Vec<SensorSeries>,
    pub(crate) total_samples: u64,
    pub(crate) last_req: u64,
    pub(crate) can_self_test_counter: u8,
    pub(crate) signal_processor: SignalProcessor,
    pub(crate) derived_signals: HashMap<(String, String), SensorSeries>,
    pub(crate) selected_view: TestSignalView,
    pub(crate) dynamic_windows: Vec<DynamicSignalWindow>,
    pub(crate) demo_alarm_bit_state: Option<bool>,
    pub(crate) demo_group_index: u32,
    pub(crate) sent1_group_index: u32,
    pub(crate) can_group_index: u32,
    pub(crate) tcp_group_index: u32,
    pub(crate) active_alarms: HashMap<String, AlarmViewItem>,
    pub(crate) acknowledged_alarms: HashSet<String>,
    pub(crate) alarm_history: VecDeque<AlarmViewItem>,
    pub(crate) total_alarm_count: u64,
    pub(crate) can_channel_last_seen: HashMap<u8, Instant>,
    pub(crate) can_alarm_thresholds: CanAlarmThresholds,
    pub(crate) sent_jump_thresholds: SentTorqueJumpThresholds,
    pub(crate) sent_angle_jump_thresholds: SentAngleJumpThresholds,
    pub(crate) last_can_self_test_result: String,
    pub(crate) can_replay: CanReplayState,
    pub(crate) alarm_records: AlarmRecordState,
}

impl UiClientState {
    pub(crate) fn new(pg_dsn: String, max_alarm_history: usize) -> Self {
        Self {
            status: "starting...".to_string(),
            sensors: (0..SENSOR_COUNT).map(|_| SensorSeries::new()).collect(),
            tcp_sensors: (0..SENSOR_COUNT).map(|_| SensorSeries::new()).collect(),
            total_samples: 0,
            last_req: 0,
            can_self_test_counter: 0,
            signal_processor: SignalProcessor::new(default_signal_specs(SENSOR_COUNT)),
            derived_signals: HashMap::new(),
            selected_view: TestSignalView::Sent,
            dynamic_windows: Vec::new(),
            demo_alarm_bit_state: None,
            demo_group_index: 1,
            sent1_group_index: 1,
            can_group_index: 1,
            tcp_group_index: 1,
            active_alarms: HashMap::new(),
            acknowledged_alarms: HashSet::new(),
            alarm_history: VecDeque::with_capacity(max_alarm_history),
            total_alarm_count: 0,
            can_channel_last_seen: HashMap::new(),
            can_alarm_thresholds: CanAlarmThresholds::default(),
            sent_jump_thresholds: SentTorqueJumpThresholds::default(),
            sent_angle_jump_thresholds: SentAngleJumpThresholds::default(),
            last_can_self_test_result: "not run".to_string(),
            can_replay: CanReplayState::new(pg_dsn.clone()),
            alarm_records: AlarmRecordState::new(pg_dsn),
        }
    }
}
