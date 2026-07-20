use crate::*;
use demo2::bus::TelemetrySourceKind;
use demo2::domain::AlarmEvent;
use demo2::domain::telemetry::{sent_angle_label, sent_torque_label};
use demo2::signal::{SignalKind, SignalSample};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Instant;

impl UiClientApp {
    pub(crate) fn alarm_key(event: &AlarmEvent) -> String {
        format!("{}::{}", event.device_id, event.alarm_id)
    }

    pub(crate) fn apply_alarm(&mut self, alarm: AlarmEvent) {
        let key = Self::alarm_key(&alarm);
        let received_at = Instant::now();

        if alarm.cleared {
            self.active_alarms.remove(&key);
            self.acknowledged_alarms.remove(&key);
        } else {
            let is_new = !self.active_alarms.contains_key(&key);
            self.active_alarms.insert(
                key.clone(),
                AlarmViewItem {
                    event: alarm.clone(),
                    received_at,
                },
            );
            if is_new {
                self.total_alarm_count = self.total_alarm_count.saturating_add(1);
                self.acknowledged_alarms.remove(&key);
            }
        }

        self.alarm_history.push_front(AlarmViewItem {
            event: alarm,
            received_at,
        });
        while self.alarm_history.len() > Self::MAX_ALARM_HISTORY {
            self.alarm_history.pop_back();
        }
    }

    pub(crate) fn can_channel_from_device_id(device_id: &str) -> Option<u8> {
        let (_, channel) = device_id.rsplit_once(":ch")?;
        channel.parse().ok()
    }

    pub(crate) fn is_can_signal_timeout(event: &AlarmEvent) -> bool {
        event.alarm_id.starts_with("can_signal_timeout_")
            && Self::can_channel_from_device_id(&event.device_id).is_some()
    }

    pub(crate) fn acknowledge_channel_alarms(&mut self, channel: u8) {
        let keys = self
            .active_alarms
            .iter()
            .filter(|(_, item)| {
                Self::is_can_signal_timeout(&item.event)
                    && Self::can_channel_from_device_id(&item.event.device_id) == Some(channel)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        self.acknowledged_alarms.extend(keys);
    }

    pub(crate) fn acknowledge_all_alarms(&mut self) {
        let keys = self.active_alarms.keys().cloned().collect::<Vec<_>>();
        self.acknowledged_alarms.extend(keys);
    }

    pub(crate) fn parse_optional_threshold(text: &str) -> Option<f64> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            None
        } else {
            trimmed.parse::<f64>().ok()
        }
    }

    pub(crate) fn validate_threshold_text(text: &str) -> Result<String, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        match trimmed.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(trimmed.to_string()),
            _ => Err(format!("无效阈值: {trimmed}")),
        }
    }

    pub(crate) fn apply_can_alarm_thresholds(&mut self) -> Result<(), String> {
        let x_high = Self::validate_threshold_text(&self.can_alarm_thresholds.x_high_input)?;
        let x_low = Self::validate_threshold_text(&self.can_alarm_thresholds.x_low_input)?;
        let y_high = Self::validate_threshold_text(&self.can_alarm_thresholds.y_high_input)?;
        let y_low = Self::validate_threshold_text(&self.can_alarm_thresholds.y_low_input)?;
        let z_high = Self::validate_threshold_text(&self.can_alarm_thresholds.z_high_input)?;
        let z_low = Self::validate_threshold_text(&self.can_alarm_thresholds.z_low_input)?;

        self.can_alarm_thresholds.x_high_applied = x_high;
        self.can_alarm_thresholds.x_low_applied = x_low;
        self.can_alarm_thresholds.y_high_applied = y_high;
        self.can_alarm_thresholds.y_low_applied = y_low;
        self.can_alarm_thresholds.z_high_applied = z_high;
        self.can_alarm_thresholds.z_low_applied = z_low;
        Ok(())
    }

    pub(crate) fn can_alarm_rule(
        &self,
        sensor_id: usize,
    ) -> (Option<f64>, Option<f64>, &'static str) {
        match sensor_id {
            0 => (
                Self::parse_optional_threshold(&self.can_alarm_thresholds.x_high_applied),
                Self::parse_optional_threshold(&self.can_alarm_thresholds.x_low_applied),
                "x",
            ),
            1 => (
                Self::parse_optional_threshold(&self.can_alarm_thresholds.y_high_applied),
                Self::parse_optional_threshold(&self.can_alarm_thresholds.y_low_applied),
                "y",
            ),
            2 => (
                Self::parse_optional_threshold(&self.can_alarm_thresholds.z_high_applied),
                Self::parse_optional_threshold(&self.can_alarm_thresholds.z_low_applied),
                "z",
            ),
            _ => (None, None, ""),
        }
    }

    pub(crate) fn can_chart_thresholds(
        &self,
        binding: SignalBinding,
    ) -> Option<(Option<f64>, Option<f64>)> {
        if !binding.is_can_axis() {
            return None;
        }
        let (high, low, _) = self.can_alarm_rule(binding.sensor_id());
        Some((high, low))
    }

    pub(crate) fn sent_torque_label(sensor_id: usize) -> &'static str {
        sent_torque_label(sensor_id)
    }

    pub(crate) fn sent_angle_label(sensor_id: usize) -> &'static str {
        sent_angle_label(sensor_id)
    }

    pub(crate) fn sent_jump_thresholds(&self) -> (f64, f64, f64) {
        let warn = Self::parse_optional_threshold(&self.sent_jump_thresholds.warn_applied)
            .unwrap_or(0.2)
            .abs();
        let red = Self::parse_optional_threshold(&self.sent_jump_thresholds.red_applied)
            .unwrap_or(0.3)
            .abs()
            .max(warn);
        let purple = Self::parse_optional_threshold(&self.sent_jump_thresholds.purple_applied)
            .unwrap_or(0.4)
            .abs()
            .max(red);
        (warn, red, purple)
    }

    pub(crate) fn apply_sent_jump_thresholds(&mut self) -> Result<(), String> {
        let warn = Self::validate_threshold_text(&self.sent_jump_thresholds.warn_input)?;
        let red = Self::validate_threshold_text(&self.sent_jump_thresholds.red_input)?;
        let purple = Self::validate_threshold_text(&self.sent_jump_thresholds.purple_input)?;
        let warn_value = Self::parse_optional_threshold(&warn).unwrap_or(0.2).abs();
        let red_value = Self::parse_optional_threshold(&red).unwrap_or(0.3).abs();
        let purple_value = Self::parse_optional_threshold(&purple).unwrap_or(0.4).abs();
        if red_value < warn_value {
            return Err("red threshold must be >= warning threshold".to_string());
        }
        if purple_value < red_value {
            return Err("purple threshold must be >= red threshold".to_string());
        }
        self.push_sent_jump_thresholds_to_collector(
            warn_value,
            red_value,
            purple_value,
            self.sent_angle_jump_threshold(0),
            self.sent_angle_jump_threshold(2),
            self.sent_angle_jump_threshold(4),
        )?;
        self.sent_jump_thresholds.warn_applied = format!("{warn_value:.3}");
        self.sent_jump_thresholds.red_applied = format!("{red_value:.3}");
        self.sent_jump_thresholds.purple_applied = format!("{purple_value:.3}");
        Ok(())
    }

    pub(crate) fn sent_angle_jump_threshold(&self, sensor_id: usize) -> f64 {
        let applied = match sensor_id {
            0 => &self.sent_angle_jump_thresholds.t1_red_applied,
            2 => &self.sent_angle_jump_thresholds.t2_red_applied,
            4 => &self.sent_angle_jump_thresholds.s_red_applied,
            _ => &self.sent_angle_jump_thresholds.t1_red_applied,
        };
        let default = if sensor_id == 4 { 1.0 } else { 0.2 };
        Self::parse_optional_threshold(applied)
            .unwrap_or(default)
            .abs()
    }

    pub(crate) fn apply_sent_angle_jump_threshold(&mut self) -> Result<(), String> {
        let t1_red = Self::validate_threshold_text(&self.sent_angle_jump_thresholds.t1_red_input)?;
        let t2_red = Self::validate_threshold_text(&self.sent_angle_jump_thresholds.t2_red_input)?;
        let s_red = Self::validate_threshold_text(&self.sent_angle_jump_thresholds.s_red_input)?;
        let t1_red_value = Self::parse_optional_threshold(&t1_red).unwrap_or(0.2).abs();
        let t2_red_value = Self::parse_optional_threshold(&t2_red).unwrap_or(0.2).abs();
        let s_red_value = Self::parse_optional_threshold(&s_red).unwrap_or(1.0).abs();
        let (torque_warn, torque_red, torque_purple) = self.sent_jump_thresholds();
        self.push_sent_jump_thresholds_to_collector(
            torque_warn,
            torque_red,
            torque_purple,
            t1_red_value,
            t2_red_value,
            s_red_value,
        )?;
        self.sent_angle_jump_thresholds.t1_red_applied = format!("{t1_red_value:.3}");
        self.sent_angle_jump_thresholds.t2_red_applied = format!("{t2_red_value:.3}");
        self.sent_angle_jump_thresholds.s_red_applied = format!("{s_red_value:.3}");
        Ok(())
    }

    fn push_sent_jump_thresholds_to_collector(
        &self,
        torque_warn: f64,
        torque_red: f64,
        torque_purple: f64,
        angle_t1_red: f64,
        angle_t2_red: f64,
        angle_s_red: f64,
    ) -> Result<(), String> {
        let payload = serde_json::json!({
            "type": "set_sent_jump_thresholds",
            "torque_warn": torque_warn,
            "torque_red": torque_red,
            "torque_purple": torque_purple,
            "angle_t1_red": angle_t1_red,
            "angle_t2_red": angle_t2_red,
            "angle_s_red": angle_s_red,
        });
        let mut stream = TcpStream::connect(&self.control_addr)
            .map_err(|err| format!("connect collector control failed: {err}"))?;
        writeln!(stream, "{payload}")
            .map_err(|err| format!("send collector control failed: {err}"))?;

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        reader
            .read_line(&mut response)
            .map_err(|err| format!("read collector control response failed: {err}"))?;
        let value = serde_json::from_str::<serde_json::Value>(response.trim())
            .map_err(|err| format!("parse collector control response failed: {err}"))?;
        if value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false) {
            return Ok(());
        }

        let message = value
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("unknown collector control error");
        Err(format!("collector control rejected: {message}"))
    }

    pub(crate) fn apply_signal_sample(&mut self, sample: SignalSample) {
        let Some(spec) = self.signal_processor.spec(&sample.signal_id).cloned() else {
            return;
        };

        match spec.kind {
            SignalKind::SourceSensor { sensor_id } => {
                let is_tcp = sample.device_id.starts_with("tcp://");
                let msg = TelemetryMsg {
                    device_id: sample.device_id,
                    sensor_id,
                    axis: String::new(),
                    t_sec: sample.t_sec,
                    value: sample.value,
                    request_id: sample.req_id,
                    alarm_bit: false,
                    source_kind: TelemetrySourceKind::Unknown,
                };
                if is_tcp {
                    self.tcp_sensors[sensor_id].push(&msg);
                } else {
                    self.sensors[sensor_id].push(&msg);
                }
            }
            SignalKind::Derived { .. } => {
                let device_id = sample.device_id.clone();
                let signal_id = sample.signal_id;
                let msg = TelemetryMsg {
                    device_id: device_id.clone(),
                    sensor_id: 0,
                    axis: String::new(),
                    t_sec: sample.t_sec,
                    value: sample.value,
                    request_id: sample.req_id,
                    alarm_bit: false,
                    source_kind: TelemetrySourceKind::Unknown,
                };
                self.derived_signals
                    .entry((device_id, signal_id))
                    .or_insert_with(SensorSeries::new)
                    .push(&msg);
            }
        }
    }
}
