use crate::*;
use demo2::bus::TelemetrySourceKind;
use demo2::domain::telemetry::{sent_angle_label, sent_torque_label};
use demo2::signal::{SignalKind, SignalSample};
use std::time::Instant;

async fn request_control_command(
    address: &str,
    payload: serde_json::Value,
    timeout: std::time::Duration,
) -> Result<(), String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    const MAX_RESPONSE: usize = 8192;
    tokio::time::timeout(timeout, async {
        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .map_err(|err| format!("connect collector control failed: {err}"))?;
        stream
            .write_all(format!("{payload}\n").as_bytes())
            .await
            .map_err(|err| format!("send collector control failed: {err}"))?;
        let mut response = Vec::new();
        BufReader::new(stream)
            .take((MAX_RESPONSE + 1) as u64)
            .read_until(b'\n', &mut response)
            .await
            .map_err(|err| format!("read collector control response failed: {err}"))?;
        if response.len() > MAX_RESPONSE || !response.ends_with(b"\n") {
            return Err("invalid collector control response length or missing newline".into());
        }
        let value: serde_json::Value = serde_json::from_slice(&response)
            .map_err(|err| format!("parse collector control response failed: {err}"))?;
        if value.get("ok").and_then(|ok| ok.as_bool()).unwrap_or(false) {
            return Ok(());
        }
        let message = value
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("unknown collector control error");
        Err(format!("collector control rejected: {message}"))
    })
    .await
    .map_err(|_| "collector control request timed out".to_string())?
}

impl UiClientApp {
    pub(crate) fn alarm_key(event: &AlarmEvent) -> String {
        format!("{}::{}", event.device_id, event.alarm_id)
    }

    pub(crate) fn apply_alarm(&mut self, update: AlarmUpdate) {
        if !self.alarm_tracker.apply_update(&update) {
            return;
        }
        let alarm = update.event;
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

    pub(crate) fn dismiss_channel_alarms(&mut self, channel: u8) {
        self.dismissed_can_channels.insert(channel);
        let keys = self
            .active_alarms
            .iter()
            .filter(|(_, item)| {
                Self::is_can_signal_timeout(&item.event)
                    && Self::can_channel_from_device_id(&item.event.device_id) == Some(channel)
            })
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            self.active_alarms.remove(&key);
            self.acknowledged_alarms.remove(&key);
        }
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

    pub(crate) fn apply_alarm_snapshot(&mut self, snapshot: AlarmSnapshot) {
        let same_epoch = self.alarm_tracker.epoch() == Some(snapshot.epoch);
        if !self.alarm_tracker.apply_snapshot(&snapshot) {
            return;
        }
        if !same_epoch {
            self.active_alarms.clear();
            self.acknowledged_alarms.clear();
            self.dismissed_can_channels.clear();
        }
        let alarms: Vec<_> = self.alarm_tracker.active().cloned().collect();
        let received_at = Instant::now();
        let previous = std::mem::take(&mut self.active_alarms);
        self.active_alarms = alarms
            .into_iter()
            .filter(|a| !a.cleared)
            .filter(|event| {
                !Self::is_can_signal_timeout(event)
                    || !Self::can_channel_from_device_id(&event.device_id)
                        .is_some_and(|channel| self.dismissed_can_channels.contains(&channel))
            })
            .map(|event| {
                let key = Self::alarm_key(&event);
                let received_at = previous
                    .get(&key)
                    .map(|item| item.received_at)
                    .unwrap_or(received_at);
                (key, AlarmViewItem { event, received_at })
            })
            .collect();
        self.state
            .acknowledged_alarms
            .retain(|key| self.state.active_alarms.contains_key(key));
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

        self.send_control_command(serde_json::json!({
            "type": "set_can_alarm_thresholds",
            "high": ([&x_high, &y_high, &z_high].map(|v| Self::parse_optional_threshold(v))),
            "low": ([&x_low, &y_low, &z_low].map(|v| Self::parse_optional_threshold(v))),
        }))?;

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

    fn parse_frame_gap_threshold(text: &str, signal: &str) -> Result<u64, String> {
        let trimmed = text.trim();
        match trimmed.parse::<u64>() {
            Ok(value) if value > 0 => Ok(value),
            _ => Err(format!(
                "{signal} frame gap threshold must be a positive integer"
            )),
        }
    }

    pub(crate) fn sent_frame_gap_thresholds(&self) -> (u64, u64, u64) {
        let t1_us = self
            .sent_frame_gap_thresholds
            .t1_us_applied
            .parse()
            .unwrap_or(1_500);
        let t2_us = self
            .sent_frame_gap_thresholds
            .t2_us_applied
            .parse()
            .unwrap_or(3_000);
        let s_us = self
            .sent_frame_gap_thresholds
            .s_us_applied
            .parse()
            .unwrap_or(1_500);
        (t1_us, t2_us, s_us)
    }

    pub(crate) fn apply_sent_frame_gap_thresholds(&mut self) -> Result<(), String> {
        let t1_us =
            Self::parse_frame_gap_threshold(&self.sent_frame_gap_thresholds.t1_us_input, "T1")?;
        let t2_us =
            Self::parse_frame_gap_threshold(&self.sent_frame_gap_thresholds.t2_us_input, "T2")?;
        let s_us =
            Self::parse_frame_gap_threshold(&self.sent_frame_gap_thresholds.s_us_input, "S")?;
        let payload = serde_json::json!({
            "type": "set_sent_frame_gap_thresholds",
            "t1_us": t1_us,
            "t2_us": t2_us,
            "s_us": s_us,
        });
        self.send_control_command(payload)?;
        self.sent_frame_gap_thresholds.t1_us_applied = t1_us.to_string();
        self.sent_frame_gap_thresholds.t2_us_applied = t2_us.to_string();
        self.sent_frame_gap_thresholds.s_us_applied = s_us.to_string();
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
        self.send_control_command(payload)
    }

    fn send_control_command(&self, payload: serde_json::Value) -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| format!("control runtime failed: {err}"))?;
        let result = runtime.block_on(request_control_command(
            &self.control_addr,
            payload,
            std::time::Duration::from_secs(2),
        ));
        // DNS resolution may use a blocking worker. Do not wait indefinitely
        // for that worker when a timed-out request tears down its runtime.
        runtime.shutdown_timeout(std::time::Duration::from_millis(50));
        result
    }

    pub(crate) fn apply_signal_sample(&mut self, sample: SignalSample) {
        let Some(spec) = self.signal_processor.spec(&sample.signal_id).cloned() else {
            return;
        };

        match spec.kind {
            // Raw series are stored with their source identity in handle_sample.
            SignalKind::SourceSensor { .. } => {}
            SignalKind::Derived { .. } => {
                let device_id = sample.device_id.clone();
                let signal_id = sample.signal_id;
                let msg = TelemetryMsg {
                    captured_at_ms: 0,
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

#[cfg(test)]
mod control_tests {
    use super::request_control_command;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn control_timeout_includes_waiting_for_a_reply() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            let mut request = String::new();
            reader.read_line(&mut request).await.unwrap();
            assert!(request.ends_with('\n'));
            std::future::pending::<()>().await;
        });
        let result = request_control_command(
            &address,
            serde_json::json!({"type":"test"}),
            Duration::from_millis(100),
        )
        .await;
        server.abort();
        assert_eq!(result.unwrap_err(), "collector control request timed out");
    }

    async fn response_result(response: Vec<u8>) -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            reader.read_line(&mut String::new()).await.unwrap();
            for chunk in response.chunks(7) {
                if reader.get_mut().write_all(chunk).await.is_err() {
                    break;
                }
            }
        });
        let result =
            request_control_command(&address, serde_json::json!({}), Duration::from_secs(2)).await;
        server.await.unwrap();
        result
    }

    #[tokio::test]
    async fn control_accepts_fragmented_success_and_reports_rejection() {
        response_result(b"{\"ok\":true}\n".to_vec()).await.unwrap();
        assert!(
            response_result(b"{\"ok\":false,\"error\":\"bad threshold\"}\n".to_vec())
                .await
                .unwrap_err()
                .contains("bad threshold")
        );
    }

    #[tokio::test]
    async fn control_rejects_unterminated_and_oversized_responses() {
        assert!(response_result(b"{\"ok\":true}".to_vec()).await.is_err());
        assert!(response_result(vec![b' '; 8193]).await.is_err());
    }
}
