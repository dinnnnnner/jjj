use crate::bus::{AppEvent, DeviceEvent, EventBus, TelemetrySourceKind};
use crate::domain::alarm::{
    SentAngleJumpState, SentJumpLevel, SentTorqueJumpState, SentTorqueJumpThresholds,
    sent_angle_jump_alarm_event, sent_torque_jump_alarm_event,
};
use crate::domain::telemetry::{is_sent_angle_sample, is_sent_torque_sample};
use crate::domain::{AlarmEvent, AlarmLevel};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::SystemTime;

const DEFAULT_SENT_TORQUE_WARN: f64 = 0.2;
const DEFAULT_SENT_TORQUE_RED: f64 = 0.3;
const DEFAULT_SENT_TORQUE_PURPLE: f64 = 0.4;
const DEFAULT_SENT_T1_ANGLE_RED: f64 = 0.2;
const DEFAULT_SENT_T2_ANGLE_RED: f64 = 0.2;
const DEFAULT_SENT_S_ANGLE_RED: f64 = 1.0;

#[derive(Clone, Copy, Debug)]
pub struct SentJumpAlarmConfig {
    pub torque_warn: f64,
    pub torque_red: f64,
    pub torque_purple: f64,
    pub angle_t1_red: f64,
    pub angle_t2_red: f64,
    pub angle_s_red: f64,
}

impl Default for SentJumpAlarmConfig {
    fn default() -> Self {
        Self {
            torque_warn: DEFAULT_SENT_TORQUE_WARN,
            torque_red: DEFAULT_SENT_TORQUE_RED,
            torque_purple: DEFAULT_SENT_TORQUE_PURPLE,
            angle_t1_red: DEFAULT_SENT_T1_ANGLE_RED,
            angle_t2_red: DEFAULT_SENT_T2_ANGLE_RED,
            angle_s_red: DEFAULT_SENT_S_ANGLE_RED,
        }
    }
}

impl SentJumpAlarmConfig {
    pub fn normalized(self) -> Result<Self, &'static str> {
        if !self.torque_warn.is_finite()
            || !self.torque_red.is_finite()
            || !self.torque_purple.is_finite()
            || !self.angle_t1_red.is_finite()
            || !self.angle_t2_red.is_finite()
            || !self.angle_s_red.is_finite()
        {
            return Err("SENT jump thresholds must be finite numbers");
        }

        let thresholds = SentTorqueJumpThresholds::normalized(
            self.torque_warn,
            self.torque_red,
            self.torque_purple,
        );
        Ok(Self {
            torque_warn: thresholds.warn,
            torque_red: thresholds.red,
            torque_purple: thresholds.purple,
            angle_t1_red: self.angle_t1_red.abs(),
            angle_t2_red: self.angle_t2_red.abs(),
            angle_s_red: self.angle_s_red.abs(),
        })
    }
}

/// Per-sensor threshold rule.
///
/// - high/high_clear: raise high alarm when value >= high, clear when value <= high_clear.
/// - low/low_clear: raise low alarm when value <= low, clear when value >= low_clear.
#[derive(Clone, Debug)]
pub struct AlarmRule {
    pub high: Option<f64>,
    pub high_clear: Option<f64>,
    pub low: Option<f64>,
    pub low_clear: Option<f64>,
    pub level: AlarmLevel,
    pub name: String,
}

impl Default for AlarmRule {
    fn default() -> Self {
        Self {
            high: Some(90.0),
            high_clear: Some(85.0),
            low: Some(10.0),
            low_clear: Some(15.0),
            level: AlarmLevel::Warning,
            name: "default_rule".to_string(),
        }
    }
}

/// App-layer alarm evaluator.
///
/// Telemetry-derived alarms are produced here so UI clients can stay read-only
/// alarm consumers.
#[derive(Clone)]
pub struct AlarmService {
    bus: EventBus,
    rules: Arc<RwLock<HashMap<usize, AlarmRule>>>,
    states: Arc<RwLock<HashMap<(String, usize), SensorAlarmState>>>,
    sent_torque_states: Arc<RwLock<HashMap<(String, usize), SentTorqueJumpState>>>,
    sent_angle_states: Arc<RwLock<HashMap<(String, usize), SentAngleJumpState>>>,
    sent_jump_config: Arc<RwLock<SentJumpAlarmConfig>>,
}

#[derive(Clone, Copy, Debug, Default)]
struct SensorAlarmState {
    high_active: bool,
    low_active: bool,
}

impl AlarmService {
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            rules: Arc::new(RwLock::new(HashMap::new())),
            states: Arc::new(RwLock::new(HashMap::new())),
            sent_torque_states: Arc::new(RwLock::new(HashMap::new())),
            sent_angle_states: Arc::new(RwLock::new(HashMap::new())),
            sent_jump_config: Arc::new(RwLock::new(SentJumpAlarmConfig::default())),
        }
    }

    /// Set or replace rule by sensor id.
    pub fn set_rule(&self, sensor_id: usize, rule: AlarmRule) {
        if let Ok(mut rules) = self.rules.write() {
            rules.insert(sensor_id, rule);
        }
    }

    pub fn set_sent_jump_config(&self, config: SentJumpAlarmConfig) -> Result<(), &'static str> {
        let config = config.normalized()?;
        if let Ok(mut current) = self.sent_jump_config.write() {
            *current = config;
        }
        Ok(())
    }

    pub fn sent_jump_config(&self) -> SentJumpAlarmConfig {
        self.sent_jump_config
            .read()
            .map(|config| *config)
            .unwrap_or_default()
    }

    /// Evaluate one telemetry sample.
    pub fn evaluate_sample(
        &self,
        device_id: &str,
        sensor_id: usize,
        value: f64,
        source_kind: TelemetrySourceKind,
    ) {
        self.evaluate_rule_sample(device_id, sensor_id, value);
        self.evaluate_sent_jump_sample(device_id, sensor_id, value, source_kind);
    }

    fn evaluate_rule_sample(&self, device_id: &str, sensor_id: usize, value: f64) {
        let Some(rule) = self
            .rules
            .read()
            .ok()
            .and_then(|rules| rules.get(&sensor_id).cloned())
        else {
            return;
        };

        let key = (device_id.to_string(), sensor_id);
        let mut events = Vec::new();
        if let Ok(mut states) = self.states.write() {
            let state = states.entry(key).or_default();
            evaluate_high(device_id, sensor_id, value, &rule, state, &mut events);
            evaluate_low(device_id, sensor_id, value, &rule, state, &mut events);
        }

        for event in events {
            self.publish_alarm(event);
        }
    }

    fn evaluate_sent_jump_sample(
        &self,
        device_id: &str,
        sensor_id: usize,
        value: f64,
        source_kind: TelemetrySourceKind,
    ) {
        if is_sent_torque_sample(source_kind, sensor_id) {
            self.evaluate_sent_torque_jump(device_id, sensor_id, value);
        }
        if is_sent_angle_sample(source_kind, sensor_id) {
            self.evaluate_sent_angle_jump(device_id, sensor_id, value);
        }
    }

    fn evaluate_sent_torque_jump(&self, device_id: &str, sensor_id: usize, value: f64) {
        let key = (device_id.to_string(), sensor_id);
        let config = self.sent_jump_config();
        let thresholds = SentTorqueJumpThresholds::normalized(
            config.torque_warn,
            config.torque_red,
            config.torque_purple,
        );
        let mut events = Vec::new();

        if let Ok(mut states) = self.sent_torque_states.write() {
            let previous = states.get(&key).copied().unwrap_or_default();
            let (next, transition) = previous.evaluate(value, thresholds);
            if let Some(transition) = transition {
                if transition.previous_level != SentJumpLevel::Normal {
                    events.push(sent_torque_jump_alarm_event(
                        device_id,
                        sensor_id,
                        transition.delta_abs,
                        thresholds,
                        transition.previous_level,
                        true,
                    ));
                }
                if transition.next_level != SentJumpLevel::Normal {
                    events.push(sent_torque_jump_alarm_event(
                        device_id,
                        sensor_id,
                        transition.delta_abs,
                        thresholds,
                        transition.next_level,
                        false,
                    ));
                }
            }
            states.insert(key, next);
        }

        for event in events {
            self.publish_alarm(event);
        }
    }

    fn evaluate_sent_angle_jump(&self, device_id: &str, sensor_id: usize, value: f64) {
        let key = (device_id.to_string(), sensor_id);
        let config = self.sent_jump_config();
        let red = sent_angle_red_threshold(config, sensor_id);
        let mut events = Vec::new();

        if let Ok(mut states) = self.sent_angle_states.write() {
            let previous = states.get(&key).copied().unwrap_or_default();
            let (next, transition) = previous.evaluate(value, red);
            if let Some(transition) = transition {
                events.push(sent_angle_jump_alarm_event(
                    device_id,
                    sensor_id,
                    transition.delta_abs,
                    red,
                    transition.cleared,
                ));
            }
            states.insert(key, next);
        }

        for event in events {
            self.publish_alarm(event);
        }
    }

    fn publish_alarm(&self, event: AlarmEvent) {
        let app_event = if event.cleared {
            AppEvent::Device(DeviceEvent::AlarmCleared(event))
        } else {
            AppEvent::Device(DeviceEvent::AlarmRaised(event))
        };
        self.bus.publish(app_event);
    }
}

fn sent_angle_red_threshold(config: SentJumpAlarmConfig, sensor_id: usize) -> f64 {
    match sensor_id {
        0 => config.angle_t1_red,
        2 => config.angle_t2_red,
        4 => config.angle_s_red,
        _ => config.angle_t1_red,
    }
}

fn evaluate_high(
    device_id: &str,
    sensor_id: usize,
    value: f64,
    rule: &AlarmRule,
    state: &mut SensorAlarmState,
    events: &mut Vec<AlarmEvent>,
) {
    match rule.high {
        Some(threshold) => {
            if !state.high_active && value >= threshold {
                state.high_active = true;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "high", threshold, value, false,
                ));
            } else if state.high_active && value <= rule.high_clear.unwrap_or(threshold) {
                state.high_active = false;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "high", threshold, value, true,
                ));
            }
        }
        None => {
            if state.high_active {
                state.high_active = false;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "high", value, value, true,
                ));
            }
        }
    }
}

fn evaluate_low(
    device_id: &str,
    sensor_id: usize,
    value: f64,
    rule: &AlarmRule,
    state: &mut SensorAlarmState,
    events: &mut Vec<AlarmEvent>,
) {
    match rule.low {
        Some(threshold) => {
            if !state.low_active && value <= threshold {
                state.low_active = true;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "low", threshold, value, false,
                ));
            } else if state.low_active && value >= rule.low_clear.unwrap_or(threshold) {
                state.low_active = false;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "low", threshold, value, true,
                ));
            }
        }
        None => {
            if state.low_active {
                state.low_active = false;
                events.push(alarm_event(
                    device_id, sensor_id, rule, "low", value, value, true,
                ));
            }
        }
    }
}

fn alarm_event(
    device_id: &str,
    sensor_id: usize,
    rule: &AlarmRule,
    bound: &str,
    threshold: f64,
    value: f64,
    cleared: bool,
) -> AlarmEvent {
    AlarmEvent {
        device_id: device_id.to_string(),
        alarm_id: format!("{}_sensor_{}_{}", rule.name, sensor_id, bound),
        level: rule.level.clone(),
        message: format!(
            "rule={}, sensor={}, bound={}, threshold={threshold:.3}, value={value:.3}",
            rule.name, sensor_id, bound
        ),
        raised_at: SystemTime::now(),
        cleared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next_alarm(bus_rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> AlarmEvent {
        match bus_rx.try_recv().unwrap() {
            AppEvent::Alarm(update) => update.event,
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn high_alarm_uses_clear_threshold_and_dedupes() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let service = AlarmService::new(bus);
        service.set_rule(
            1,
            AlarmRule {
                high: Some(10.0),
                high_clear: Some(8.0),
                low: None,
                low_clear: None,
                level: AlarmLevel::Warning,
                name: "temperature".to_string(),
            },
        );

        service.evaluate_sample("dev1", 1, 11.0, TelemetrySourceKind::Unknown);
        let raised = next_alarm(&mut rx);
        assert_eq!(raised.alarm_id, "temperature_sensor_1_high");
        assert!(!raised.cleared);

        service.evaluate_sample("dev1", 1, 9.0, TelemetrySourceKind::Unknown);
        assert!(rx.try_recv().is_err());

        service.evaluate_sample("dev1", 1, 8.0, TelemetrySourceKind::Unknown);
        let cleared = next_alarm(&mut rx);
        assert_eq!(cleared.alarm_id, "temperature_sensor_1_high");
        assert!(cleared.cleared);
    }

    #[test]
    fn low_alarm_is_tracked_per_device() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let service = AlarmService::new(bus);
        service.set_rule(
            2,
            AlarmRule {
                high: None,
                high_clear: None,
                low: Some(-5.0),
                low_clear: Some(-2.0),
                level: AlarmLevel::Critical,
                name: "pressure".to_string(),
            },
        );

        service.evaluate_sample("dev1", 2, -6.0, TelemetrySourceKind::Unknown);
        service.evaluate_sample("dev2", 2, -6.0, TelemetrySourceKind::Unknown);
        assert_eq!(next_alarm(&mut rx).device_id, "dev1");
        assert_eq!(next_alarm(&mut rx).device_id, "dev2");

        service.evaluate_sample("dev1", 2, -1.0, TelemetrySourceKind::Unknown);
        let cleared = next_alarm(&mut rx);
        assert_eq!(cleared.device_id, "dev1");
        assert!(cleared.cleared);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sent_jump_config_normalizes_and_rejects_invalid_numbers() {
        let bus = EventBus::new(16);
        let service = AlarmService::new(bus);

        service
            .set_sent_jump_config(SentJumpAlarmConfig {
                torque_warn: -0.4,
                torque_red: 0.2,
                torque_purple: 0.3,
                angle_t1_red: -0.5,
                angle_t2_red: -0.7,
                angle_s_red: 2.0,
            })
            .unwrap();
        let config = service.sent_jump_config();
        assert_eq!(config.torque_warn, 0.4);
        assert_eq!(config.torque_red, 0.4);
        assert_eq!(config.torque_purple, 0.4);
        assert_eq!(config.angle_t1_red, 0.5);
        assert_eq!(config.angle_t2_red, 0.7);
        assert_eq!(config.angle_s_red, 2.0);

        assert!(
            service
                .set_sent_jump_config(SentJumpAlarmConfig {
                    torque_warn: f64::NAN,
                    torque_red: 0.3,
                    torque_purple: 0.4,
                    angle_t1_red: 0.2,
                    angle_t2_red: 0.2,
                    angle_s_red: 1.0,
                })
                .is_err()
        );
        assert_eq!(service.sent_jump_config().torque_warn, 0.4);
    }

    #[test]
    fn sent_torque_jump_is_emitted_by_service() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let service = AlarmService::new(bus);

        for _ in 0..10 {
            service.evaluate_sample("dev1", 1, 1.0, TelemetrySourceKind::CanSent);
            assert!(rx.try_recv().is_err());
        }

        service.evaluate_sample("dev1", 1, 1.35, TelemetrySourceKind::CanSent);
        let raised = next_alarm(&mut rx);
        assert_eq!(raised.alarm_id, "sent_torque_jump_t1");
        assert!(matches!(raised.level, AlarmLevel::Critical));
        assert!(!raised.cleared);
    }

    #[test]
    fn sent_angle_jump_uses_per_axis_thresholds() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let service = AlarmService::new(bus);

        for _ in 0..10 {
            service.evaluate_sample("dev1", 4, 0.0, TelemetrySourceKind::CanSent);
            assert!(rx.try_recv().is_err());
        }

        service.evaluate_sample("dev1", 4, 2.0, TelemetrySourceKind::CanSent);
        let raised = next_alarm(&mut rx);
        assert_eq!(raised.alarm_id, "sent_angle_jump_s");
        assert!(matches!(raised.level, AlarmLevel::Critical));
        assert!(!raised.cleared);
        assert!(raised.message.contains("red=1.000"));
    }

    #[test]
    fn sent_angle_jump_uses_separate_t1_and_t2_thresholds() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();
        let service = AlarmService::new(bus);
        service
            .set_sent_jump_config(SentJumpAlarmConfig {
                angle_t1_red: 0.3,
                angle_t2_red: 0.8,
                ..SentJumpAlarmConfig::default()
            })
            .unwrap();

        for _ in 0..10 {
            service.evaluate_sample("dev1", 0, 0.0, TelemetrySourceKind::CanSent);
            service.evaluate_sample("dev1", 2, 0.0, TelemetrySourceKind::CanSent);
        }
        assert!(rx.try_recv().is_err());

        service.evaluate_sample("dev1", 0, 0.5, TelemetrySourceKind::CanSent);
        let raised = next_alarm(&mut rx);
        assert_eq!(raised.alarm_id, "sent_angle_jump_t1");
        assert!(raised.message.contains("red=0.300"));

        service.evaluate_sample("dev1", 2, 0.5, TelemetrySourceKind::CanSent);
        assert!(rx.try_recv().is_err());
    }
}
