use crate::{
    CAN_REPLAY_MIN_SPAN_SEC, MAX_UI_MESSAGES_PER_UPDATE, SENSOR_COUNT, TestSignalView,
    UI_MESSAGE_TIME_BUDGET_MS, UiClientApp, UiMsg,
};
use demo2::bus::TelemetrySourceKind;
use demo2::signal::RawSample;
use std::time::{Duration, Instant};

use super::messages::TelemetryMsg;

impl UiClientApp {
    fn detect_view_for_sample(sample: &TelemetryMsg) -> Option<TestSignalView> {
        match sample.source_kind {
            TelemetrySourceKind::SerialDemo => Some(TestSignalView::Demo),
            TelemetrySourceKind::SerialSent1
            | TelemetrySourceKind::SerialSent2
            | TelemetrySourceKind::SerialSent3
            | TelemetrySourceKind::CanSent => Some(TestSignalView::Sent),
            TelemetrySourceKind::CanAxis => Some(TestSignalView::CanFrame),
            TelemetrySourceKind::TcpFrame => Some(TestSignalView::TcpFrame),
            TelemetrySourceKind::Unknown | TelemetrySourceKind::FrameStream => {
                if sample.device_id.starts_with("can://") {
                    return Some(TestSignalView::CanFrame);
                }
                if sample.device_id.starts_with("tcp://") {
                    return Some(TestSignalView::TcpFrame);
                }

                match sample.sensor_id {
                    0..=4 => Some(TestSignalView::Sent),
                    _ => None,
                }
            }
        }
    }

    fn switch_to_view(&mut self, view: TestSignalView) {
        self.selected_view = view;
        self.dynamic_windows.clear();
        self.add_dynamic_window();
        self.reset_layout();
    }

    /// Process a bounded slice of queued work so input and painting remain
    /// responsive even when the feed arrives in a large burst.
    ///
    /// Returns `true` when the per-update limit was reached and another repaint
    /// should be scheduled immediately.
    pub(crate) fn drain_messages(&mut self) -> bool {
        let started_at = Instant::now();
        let budget = Duration::from_millis(UI_MESSAGE_TIME_BUDGET_MS);
        let mut processed = 0;

        while processed < MAX_UI_MESSAGES_PER_UPDATE {
            let Ok(msg) = self.rx.try_recv() else {
                return false;
            };
            self.handle_ui_msg(msg);
            processed += 1;

            // Avoid a clock read for every signal while still enforcing a
            // tight enough budget for a smooth UI frame.
            if processed % 64 == 0 && started_at.elapsed() >= budget {
                return true;
            }
        }

        true
    }

    fn handle_ui_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Status(status) => self.handle_status(status),
            UiMsg::Alarm(alarm) => self.apply_alarm(alarm),
            UiMsg::AlarmSnapshot(alarms) => self.apply_alarm_snapshot(alarms),
            UiMsg::CanReplayLoaded(mode, result) => {
                self.can_replay.loading = false;
                if mode != self.can_replay.mode {
                    return;
                }
                match result {
                    Ok(data) => {
                        self.can_replay.device_input = data.device_id.clone();
                        self.can_replay.available_devices = data.available_devices.clone();
                        let span_sec = data.total_span_sec();
                        self.can_replay.view_start_sec = 0.0;
                        self.can_replay.view_span_sec =
                            span_sec.clamp(CAN_REPLAY_MIN_SPAN_SEC, 120.0);
                        self.can_replay.status = if data.is_empty() {
                            self.can_replay.mode.empty_status().to_string()
                        } else {
                            let displayed = data.displayed_point_count();
                            if displayed < data.raw_point_count {
                                format!(
                                    "已加载 {} 个回放点，图表降采样显示 {} 个点",
                                    data.raw_point_count, displayed
                                )
                            } else {
                                format!("已加载 {} 个回放点", data.raw_point_count)
                            }
                        };
                        self.can_replay.data = Some(data);
                    }
                    Err(err) => {
                        self.can_replay.status = format!("load failed: {err}");
                        self.can_replay.data = None;
                    }
                }
            }
            UiMsg::CanReplayExported(result) => {
                self.can_replay.exporting = false;
                self.can_replay.status = match result {
                    Ok(path) => format!("exported: {path}"),
                    Err(err) => format!("export failed: {err}"),
                };
            }
            UiMsg::AlarmRecordsLoaded(result) => {
                self.alarm_records.loading = false;
                match result {
                    Ok(data) => {
                        let count = data.records.len();
                        self.alarm_records.page_index = data.page_index;
                        self.alarm_records.has_next = data.has_next;
                        self.alarm_records.status = if data.is_empty() {
                            format!(
                                "page {} has no alarm records, total {}",
                                data.page_index + 1,
                                data.total_count
                            )
                        } else {
                            format!(
                                "page {}, {} alarm records, total {}",
                                data.page_index + 1,
                                count,
                                data.total_count
                            )
                        };
                        self.alarm_records.data = Some(data);
                    }
                    Err(err) => {
                        self.alarm_records.status = format!("load failed: {err}");
                        self.alarm_records.data = None;
                    }
                }
            }
            UiMsg::Sample(sample) => self.handle_sample(sample),
        }
    }

    fn handle_status(&mut self, status: String) {
        if status.contains("CAN self-test")
            || status.contains("CAN 鑷")
            || status.contains("CAN 鑷缁撴灉")
        {
            self.last_can_self_test_result = status.clone();
        }
        self.status = status;
    }

    fn handle_sample(&mut self, sample: TelemetryMsg) {
        if sample.sensor_id >= SENSOR_COUNT {
            return;
        }
        if sample.source_kind == TelemetrySourceKind::SerialDemo {
            self.demo_alarm_bit_state = Some(sample.alarm_bit);
        }

        if let Some(view) = Self::detect_view_for_sample(&sample) {
            if !self.view_initialized {
                self.switch_to_view(view);
                self.view_initialized = true;
            }
            self.selected_devices
                .entry(view)
                .or_insert_with(|| sample.device_id.clone());
            self.source_series
                .entry((sample.device_id.clone(), view, sample.sensor_id))
                .or_insert_with(super::series::SensorSeries::new)
                .push(&sample);
        }

        if sample.sensor_id >= SENSOR_COUNT {
            return;
        }

        let processed = self.signal_processor.ingest_raw(RawSample {
            device_id: sample.device_id.clone(),
            sensor_id: sample.sensor_id,
            t_sec: sample.t_sec,
            value: sample.value,
            req_id: sample.request_id,
        });
        for signal in processed {
            self.apply_signal_sample(signal);
        }

        self.total_samples = self.total_samples.saturating_add(1);
        self.last_req = sample.request_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use demo2::domain::{AlarmEvent, AlarmLevel};
    fn app() -> UiClientApp {
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        UiClientApp::new(
            rx,
            tx,
            std::sync::Arc::default(),
            String::new(),
            String::new(),
            String::new(),
        )
    }
    fn sample(device: &str, kind: TelemetrySourceKind, value: f64) -> TelemetryMsg {
        TelemetryMsg {
            captured_at_ms: 1234,
            device_id: device.into(),
            sensor_id: 0,
            axis: String::new(),
            alarm_bit: false,
            t_sec: 1.0,
            value,
            request_id: 1,
            source_kind: kind,
        }
    }
    #[test]
    fn devices_and_source_kinds_do_not_share_curves() {
        let mut app = app();
        app.handle_sample(sample("can://a:ch0", TelemetrySourceKind::CanAxis, 10.0));
        app.handle_sample(sample("can://a:ch1", TelemetrySourceKind::CanAxis, 20.0));
        app.handle_sample(sample("can://a:ch0", TelemetrySourceKind::CanSent, 30.0));
        assert_eq!(app.source_series.len(), 3);
        for (device, view, expected) in [
            ("can://a:ch0", TestSignalView::CanFrame, 10.0),
            ("can://a:ch1", TestSignalView::CanFrame, 20.0),
            ("can://a:ch0", TestSignalView::Sent, 30.0),
        ] {
            let series = &app.source_series[&(device.into(), view, 0)];
            assert_eq!(series.points.len(), 1);
            assert_eq!(series.latest, Some(expected));
        }
    }
    #[test]
    fn mixed_sources_preserve_view_and_user_windows() {
        let mut app = app();
        app.handle_sample(sample("can://a", TelemetrySourceKind::CanAxis, 10.0));
        app.dynamic_windows[0].title = "user window".into();
        app.handle_sample(sample("tcp://b", TelemetrySourceKind::TcpFrame, 20.0));
        assert_eq!(app.selected_view, TestSignalView::CanFrame);
        assert_eq!(app.dynamic_windows[0].title, "user window");
        app.dynamic_windows.clear();
        app.handle_sample(sample("tcp://b", TelemetrySourceKind::TcpFrame, 30.0));
        assert!(app.dynamic_windows.is_empty());
    }
    #[test]
    fn manual_view_before_first_sample_is_respected() {
        let mut app = app();
        app.selected_view = TestSignalView::Demo;
        app.view_initialized = true;
        app.handle_sample(sample("tcp://b", TelemetrySourceKind::TcpFrame, 20.0));
        assert_eq!(app.selected_view, TestSignalView::Demo);
    }
    #[test]
    fn delayed_alarm_cannot_override_snapshot_or_increment_history() {
        let mut app = app();
        let epoch = uuid::Uuid::new_v4();
        let alarm = AlarmEvent {
            device_id: "can://test:ch0".into(),
            alarm_id: "sent_angle_jump_t1".into(),
            level: AlarmLevel::Critical,
            message: "jump".into(),
            raised_at: std::time::SystemTime::UNIX_EPOCH,
            cleared: false,
        };
        app.handle_ui_msg(UiMsg::AlarmSnapshot(crate::AlarmSnapshot {
            epoch,
            revision: 2,
            alarms: vec![],
        }));
        app.handle_ui_msg(UiMsg::Alarm(crate::AlarmUpdate {
            epoch,
            revision: 1,
            event: alarm.clone(),
        }));
        assert!(app.active_alarms.is_empty());
        assert_eq!(app.total_alarm_count, 0);
        assert!(app.alarm_history.is_empty());
        app.handle_ui_msg(UiMsg::Alarm(crate::AlarmUpdate {
            epoch,
            revision: 3,
            event: alarm,
        }));
        assert_eq!(app.active_alarms.len(), 1);
        assert_eq!(app.total_alarm_count, 1);
        app.handle_ui_msg(UiMsg::AlarmSnapshot(crate::AlarmSnapshot {
            epoch,
            revision: 2,
            alarms: vec![],
        }));
        assert_eq!(app.active_alarms.len(), 1);
    }

    #[test]
    fn snapshot_repairs_missing_raise_and_clear_without_counting_duplicates() {
        let mut app = app();
        let alarm = AlarmEvent {
            device_id: "a".into(),
            alarm_id: "high".into(),
            level: AlarmLevel::Critical,
            message: "high".into(),
            raised_at: std::time::SystemTime::now(),
            cleared: false,
        };
        app.handle_ui_msg(UiMsg::AlarmSnapshot(crate::AlarmSnapshot {
            epoch: uuid::Uuid::nil(),
            revision: 1,
            alarms: vec![alarm.clone()],
        }));
        assert_eq!(app.active_alarms.len(), 1);
        let first_received_at = app.active_alarms.values().next().unwrap().received_at;
        app.handle_ui_msg(UiMsg::AlarmSnapshot(crate::AlarmSnapshot {
            epoch: uuid::Uuid::nil(),
            revision: 1,
            alarms: vec![alarm],
        }));
        assert_eq!(
            app.active_alarms.values().next().unwrap().received_at,
            first_received_at
        );
        assert_eq!(app.total_alarm_count, 0);
        app.handle_ui_msg(UiMsg::AlarmSnapshot(crate::AlarmSnapshot {
            epoch: uuid::Uuid::nil(),
            revision: 2,
            alarms: vec![],
        }));
        assert!(app.active_alarms.is_empty());
    }
}
