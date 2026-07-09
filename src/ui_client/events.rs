use crate::{CAN_REPLAY_MIN_SPAN_SEC, SENSOR_COUNT, TestSignalView, UiClientApp, UiMsg};
use demo2::bus::TelemetrySourceKind;
use demo2::signal::RawSample;

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

    pub(crate) fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.handle_ui_msg(msg);
        }
    }

    fn handle_ui_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Status(status) => self.handle_status(status),
            UiMsg::Alarm(alarm) => self.apply_alarm(alarm),
            UiMsg::CanReplayLoaded(mode, result) => {
                self.can_replay.loading = false;
                if mode != self.can_replay.mode {
                    return;
                }
                match result {
                    Ok(data) => {
                        let span_sec = data.total_span_sec();
                        self.can_replay.view_start_sec = 0.0;
                        self.can_replay.view_span_sec =
                            span_sec.clamp(CAN_REPLAY_MIN_SPAN_SEC, 120.0);
                        self.can_replay.status = if data.is_empty() {
                            self.can_replay.mode.empty_status().to_string()
                        } else {
                            let point_count = data.x_points.len()
                                + data.y_points.len()
                                + data.z_points.len()
                                + data.u_points.len()
                                + data.v_points.len();
                            format!("loaded {point_count} replay points")
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
        if sample.source_kind == TelemetrySourceKind::SerialDemo {
            self.demo_alarm_bit_state = Some(sample.alarm_bit);
        }

        if let Some(view) = Self::detect_view_for_sample(&sample) {
            let should_switch = self.selected_view != view || self.dynamic_windows.is_empty();
            if should_switch {
                self.switch_to_view(view);
            }
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
