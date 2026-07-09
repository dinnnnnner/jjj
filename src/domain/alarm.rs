use crate::domain::telemetry::{sent_angle_label, sent_torque_label};
use crate::domain::{AlarmEvent, AlarmLevel};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, Default)]
pub struct CanAxisAlarmState {
    pub high_active: bool,
    pub low_active: bool,
}

impl AlarmBound {
    pub fn short_code(self) -> &'static str {
        match self {
            Self::High => "h",
            Self::Low => "l",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Low => "low",
        }
    }
}

pub fn can_threshold_alarm_event(
    device_id: &str,
    axis: &str,
    bound: AlarmBound,
    threshold: f64,
    value: f64,
    cleared: bool,
) -> AlarmEvent {
    let bound_code = bound.short_code();
    AlarmEvent {
        device_id: device_id.to_string(),
        alarm_id: format!("can_{axis}_{bound_code}"),
        level: AlarmLevel::Warning,
        message: format!(
            "axis={axis}, bound={bound_code}, threshold={threshold:.3}, value={value:.3}"
        ),
        raised_at: SystemTime::now(),
        cleared,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlarmBound {
    High,
    Low,
}

#[derive(Clone, Copy, Debug)]
pub struct CanThresholdTransition {
    pub bound: AlarmBound,
    pub threshold: f64,
    pub cleared: bool,
}

impl CanAxisAlarmState {
    pub fn evaluate_thresholds(
        mut self,
        high: Option<f64>,
        low: Option<f64>,
        value: f64,
    ) -> (Self, Vec<CanThresholdTransition>) {
        let mut transitions = Vec::new();

        match high {
            Some(threshold) => {
                if !self.high_active && value >= threshold {
                    self.high_active = true;
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::High,
                        threshold,
                        cleared: false,
                    });
                } else if self.high_active && value < threshold {
                    self.high_active = false;
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::High,
                        threshold,
                        cleared: true,
                    });
                }
            }
            None => {
                if self.high_active {
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::High,
                        threshold: value,
                        cleared: true,
                    });
                }
                self.high_active = false;
            }
        }

        match low {
            Some(threshold) => {
                if !self.low_active && value <= threshold {
                    self.low_active = true;
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::Low,
                        threshold,
                        cleared: false,
                    });
                } else if self.low_active && value > threshold {
                    self.low_active = false;
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::Low,
                        threshold,
                        cleared: true,
                    });
                }
            }
            None => {
                if self.low_active {
                    transitions.push(CanThresholdTransition {
                        bound: AlarmBound::Low,
                        threshold: value,
                        cleared: true,
                    });
                }
                self.low_active = false;
            }
        }

        (self, transitions)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SentJumpLevel {
    Normal,
    Warning,
    Critical,
    Purple,
}

impl SentJumpLevel {
    pub fn alarm_level(self) -> AlarmLevel {
        match self {
            Self::Purple => AlarmLevel::Purple,
            Self::Critical => AlarmLevel::Critical,
            Self::Normal | Self::Warning => AlarmLevel::Warning,
        }
    }
}

pub fn sent_torque_jump_alarm_event(
    device_id: &str,
    sensor_id: usize,
    delta_abs: f64,
    thresholds: SentTorqueJumpThresholds,
    level: SentJumpLevel,
    cleared: bool,
) -> AlarmEvent {
    AlarmEvent {
        device_id: device_id.to_string(),
        alarm_id: format!(
            "sent_torque_jump_{}",
            if sensor_id == 1 { "t1" } else { "t2" }
        ),
        level: level.alarm_level(),
        message: format!(
            "{} jump={delta_abs:.3}, warn={:.3}, red={:.3}, purple={:.3}",
            sent_torque_label(sensor_id),
            thresholds.warn,
            thresholds.red,
            thresholds.purple
        ),
        raised_at: SystemTime::now(),
        cleared,
    }
}

pub fn sent_angle_jump_alarm_event(
    device_id: &str,
    sensor_id: usize,
    delta_abs: f64,
    red: f64,
    cleared: bool,
) -> AlarmEvent {
    let angle_name = match sensor_id {
        0 => "t1",
        2 => "t2",
        4 => "s",
        _ => "unknown",
    };
    AlarmEvent {
        device_id: device_id.to_string(),
        alarm_id: format!("sent_angle_jump_{angle_name}"),
        level: AlarmLevel::Critical,
        message: format!(
            "{} jump={delta_abs:.3}, red={red:.3}",
            sent_angle_label(sensor_id)
        ),
        raised_at: SystemTime::now(),
        cleared,
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SentTorqueJumpThresholds {
    pub warn: f64,
    pub red: f64,
    pub purple: f64,
}

impl SentTorqueJumpThresholds {
    pub fn normalized(warn: f64, red: f64, purple: f64) -> Self {
        let warn = warn.abs();
        let red = red.abs().max(warn);
        let purple = purple.abs().max(red);
        Self { warn, red, purple }
    }

    pub fn level_for(self, delta_abs: f64) -> SentJumpLevel {
        if delta_abs >= self.purple {
            SentJumpLevel::Purple
        } else if delta_abs >= self.red {
            SentJumpLevel::Critical
        } else if delta_abs > self.warn {
            SentJumpLevel::Warning
        } else {
            SentJumpLevel::Normal
        }
    }
}

pub const SENT_JUMP_BASELINE_WINDOW: usize = 10;

#[derive(Clone, Copy, Debug)]
pub struct SentTorqueJumpState {
    pub history: [f64; SENT_JUMP_BASELINE_WINDOW],
    pub history_len: usize,
    pub history_next: usize,
    pub last_delta: f64,
    pub alert_count: u64,
    pub level: SentJumpLevel,
}

#[derive(Clone, Copy, Debug)]
pub struct SentTorqueJumpTransition {
    pub previous_level: SentJumpLevel,
    pub next_level: SentJumpLevel,
    pub delta_abs: f64,
}

impl Default for SentTorqueJumpState {
    fn default() -> Self {
        Self {
            history: [0.0; SENT_JUMP_BASELINE_WINDOW],
            history_len: 0,
            history_next: 0,
            last_delta: 0.0,
            alert_count: 0,
            level: SentJumpLevel::Normal,
        }
    }
}

impl SentTorqueJumpState {
    pub fn evaluate(
        mut self,
        value: f64,
        thresholds: SentTorqueJumpThresholds,
    ) -> (Self, Option<SentTorqueJumpTransition>) {
        let previous = self;
        let Some(baseline) = history_mean(&self.history, self.history_len) else {
            push_history(
                &mut self.history,
                &mut self.history_len,
                &mut self.history_next,
                value,
            );
            return (self, None);
        };

        let delta_abs = (value - baseline).abs();
        let next_level = thresholds.level_for(delta_abs);
        self.last_delta = delta_abs;

        let transition = (next_level != previous.level).then_some(SentTorqueJumpTransition {
            previous_level: previous.level,
            next_level,
            delta_abs,
        });
        if previous.level != next_level && next_level != SentJumpLevel::Normal {
            self.alert_count = self.alert_count.saturating_add(1);
        }
        self.level = next_level;
        push_history(
            &mut self.history,
            &mut self.history_len,
            &mut self.history_next,
            value,
        );

        (self, transition)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SentAngleJumpState {
    pub history: [f64; SENT_JUMP_BASELINE_WINDOW],
    pub history_len: usize,
    pub history_next: usize,
    pub last_delta: f64,
    pub alert_count: u64,
    pub active: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct SentAngleJumpTransition {
    pub delta_abs: f64,
    pub cleared: bool,
}

impl Default for SentAngleJumpState {
    fn default() -> Self {
        Self {
            history: [0.0; SENT_JUMP_BASELINE_WINDOW],
            history_len: 0,
            history_next: 0,
            last_delta: 0.0,
            alert_count: 0,
            active: false,
        }
    }
}

impl SentAngleJumpState {
    pub fn evaluate(mut self, value: f64, red: f64) -> (Self, Option<SentAngleJumpTransition>) {
        let previous = self;
        let Some(baseline) = history_angle_mean(&self.history, self.history_len, value) else {
            push_history(
                &mut self.history,
                &mut self.history_len,
                &mut self.history_next,
                value,
            );
            return (self, None);
        };

        let delta_abs = angle_delta_abs(value, baseline);
        let next_active = delta_abs >= red.abs();
        self.last_delta = delta_abs;

        let transition = (next_active != previous.active).then_some(SentAngleJumpTransition {
            delta_abs,
            cleared: previous.active && !next_active,
        });
        if !previous.active && next_active {
            self.alert_count = self.alert_count.saturating_add(1);
        }
        self.active = next_active;
        push_history(
            &mut self.history,
            &mut self.history_len,
            &mut self.history_next,
            value,
        );

        (self, transition)
    }
}

pub fn angle_delta_abs(value: f64, baseline: f64) -> f64 {
    let delta = (value - baseline).abs().rem_euclid(360.0);
    delta.min(360.0 - delta)
}

fn history_ready(len: usize) -> bool {
    len >= SENT_JUMP_BASELINE_WINDOW
}

fn push_history(
    history: &mut [f64; SENT_JUMP_BASELINE_WINDOW],
    len: &mut usize,
    next: &mut usize,
    value: f64,
) {
    history[*next] = value;
    *next = (*next + 1) % SENT_JUMP_BASELINE_WINDOW;
    *len = (*len + 1).min(SENT_JUMP_BASELINE_WINDOW);
}

fn history_mean(history: &[f64; SENT_JUMP_BASELINE_WINDOW], len: usize) -> Option<f64> {
    if !history_ready(len) {
        return None;
    }
    Some(history.iter().take(len).sum::<f64>() / len as f64)
}

fn history_angle_mean(
    history: &[f64; SENT_JUMP_BASELINE_WINDOW],
    len: usize,
    reference: f64,
) -> Option<f64> {
    if !history_ready(len) {
        return None;
    }

    let (sin_sum, cos_sum) =
        history
            .iter()
            .take(len)
            .fold((0.0, 0.0), |(sin_sum, cos_sum), value| {
                let radians = value.to_radians();
                (sin_sum + radians.sin(), cos_sum + radians.cos())
            });

    let mut mean = sin_sum.atan2(cos_sum).to_degrees();
    while mean - reference > 180.0 {
        mean -= 360.0;
    }
    while reference - mean > 180.0 {
        mean += 360.0;
    }
    Some(mean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn can_threshold_tracks_edges() {
        let state = CanAxisAlarmState::default();
        let (state, transitions) = state.evaluate_thresholds(Some(10.0), Some(-10.0), 11.0);
        assert!(state.high_active);
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].bound, AlarmBound::High);
        assert!(!transitions[0].cleared);

        let (state, transitions) = state.evaluate_thresholds(Some(10.0), Some(-10.0), 9.0);
        assert!(!state.high_active);
        assert_eq!(transitions.len(), 1);
        assert!(transitions[0].cleared);
    }

    #[test]
    fn torque_jump_uses_history_baseline() {
        let thresholds = SentTorqueJumpThresholds::normalized(0.2, 0.3, 0.4);
        let mut state = SentTorqueJumpState::default();
        for _ in 0..SENT_JUMP_BASELINE_WINDOW {
            let result = state.evaluate(1.0, thresholds);
            state = result.0;
            assert!(result.1.is_none());
        }

        let (state, transition) = state.evaluate(1.35, thresholds);
        assert_eq!(state.level, SentJumpLevel::Critical);
        assert_eq!(state.alert_count, 1);
        assert_eq!(transition.unwrap().next_level, SentJumpLevel::Critical);
    }

    #[test]
    fn angle_delta_wraps_at_zero() {
        assert!((angle_delta_abs(359.0, 1.0) - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn can_threshold_event_preserves_ui_alarm_format() {
        let event = can_threshold_alarm_event("dev", "x", AlarmBound::High, 10.0, 11.25, false);

        assert_eq!(event.device_id, "dev");
        assert_eq!(event.alarm_id, "can_x_h");
        assert!(matches!(event.level, AlarmLevel::Warning));
        assert_eq!(
            event.message,
            "axis=x, bound=h, threshold=10.000, value=11.250"
        );
        assert!(!event.cleared);
    }

    #[test]
    fn sent_jump_events_preserve_ui_alarm_format() {
        let thresholds = SentTorqueJumpThresholds::normalized(0.2, 0.3, 0.4);
        let torque = sent_torque_jump_alarm_event(
            "dev",
            1,
            0.3456,
            thresholds,
            SentJumpLevel::Critical,
            false,
        );
        assert_eq!(torque.alarm_id, "sent_torque_jump_t1");
        assert!(matches!(torque.level, AlarmLevel::Critical));
        assert_eq!(
            torque.message,
            "T1 torque jump=0.346, warn=0.200, red=0.300, purple=0.400"
        );

        let angle = sent_angle_jump_alarm_event("dev", 4, 1.2345, 1.0, true);
        assert_eq!(angle.alarm_id, "sent_angle_jump_s");
        assert!(matches!(angle.level, AlarmLevel::Critical));
        assert_eq!(angle.message, "S angle jump=1.234, red=1.000");
        assert!(angle.cleared);
    }
}
