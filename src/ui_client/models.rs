use demo2::domain::AlarmEvent;
pub(crate) use demo2::domain::alarm::SentJumpLevel;
use eframe::egui;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestSignalView {
    Demo,
    Sent,
    CanFrame,
    TcpFrame,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SignalBinding {
    DemoAxisX,
    DemoAxisY,
    DemoAxisZ,
    Sent1V1,
    Sent1P1,
    Sent2V2,
    Sent2P2,
    Sent3Angle,
    CanAxisX,
    CanAxisY,
    CanAxisZ,
    TcpSensor(usize),
}

impl SignalBinding {
    pub(crate) fn title(self, index: u32) -> String {
        match self {
            Self::DemoAxisX => format!("demo_x_{index}"),
            Self::DemoAxisY => format!("demo_y_{index}"),
            Self::DemoAxisZ => format!("demo_z_{index}"),
            Self::Sent1V1 => format!("sent_t1_angle_{index}"),
            Self::Sent1P1 => format!("sent_t1_torque_{index}"),
            Self::Sent2V2 => format!("sent_t2_angle_{index}"),
            Self::Sent2P2 => format!("sent_t2_torque_{index}"),
            Self::Sent3Angle => format!("sent_s_angle_{index}"),
            Self::CanAxisX => format!("can_x_{index}"),
            Self::CanAxisY => format!("can_y_{index}"),
            Self::CanAxisZ => format!("can_z_{index}"),
            Self::TcpSensor(sensor_id) => format!("tcp_sensor_{sensor_id}_{index}"),
        }
    }

    pub(crate) fn sensor_id(self) -> usize {
        match self {
            Self::DemoAxisX => 0,
            Self::DemoAxisY => 1,
            Self::DemoAxisZ => 2,
            Self::Sent1V1 => 0,
            Self::Sent1P1 => 1,
            Self::Sent2V2 => 2,
            Self::Sent2P2 => 3,
            Self::Sent3Angle => 4,
            Self::CanAxisX => 0,
            Self::CanAxisY => 1,
            Self::CanAxisZ => 2,
            Self::TcpSensor(sensor_id) => sensor_id,
        }
    }

    pub(crate) fn uses_tcp_series(self) -> bool {
        matches!(self, Self::TcpSensor(_))
    }

    pub(crate) fn chart_label(self) -> Option<&'static str> {
        match self {
            Self::DemoAxisX => Some("demo axis X"),
            Self::DemoAxisY => Some("demo axis Y"),
            Self::DemoAxisZ => Some("demo axis Z"),
            Self::Sent1V1 => Some("SENT T1 angle"),
            Self::Sent1P1 => Some("SENT T1 torque"),
            Self::Sent2V2 => Some("SENT T2 angle"),
            Self::Sent2P2 => Some("SENT T2 torque"),
            Self::Sent3Angle => Some("SENT S angle"),
            Self::CanAxisX => Some("CAN axis X"),
            Self::CanAxisY => Some("CAN axis Y"),
            Self::CanAxisZ => Some("CAN axis Z"),
            _ => None,
        }
    }

    pub(crate) fn is_can_axis(self) -> bool {
        matches!(self, Self::CanAxisX | Self::CanAxisY | Self::CanAxisZ)
    }

    pub(crate) fn value_label(self) -> &'static str {
        match self {
            Self::Sent1V1 | Self::Sent2V2 | Self::Sent3Angle => "angle",
            Self::Sent1P1 | Self::Sent2P2 => "torque",
            _ => "raw",
        }
    }

    pub(crate) fn demo_derived_angle_id(self) -> Option<&'static str> {
        match self {
            Self::DemoAxisX => Some("sensor_0_angle"),
            Self::DemoAxisY => Some("sensor_1_angle"),
            Self::DemoAxisZ => Some("sensor_2_angle"),
            _ => None,
        }
    }
}

pub(crate) struct DynamicSignalWindow {
    pub(crate) title: String,
    pub(crate) binding: Option<SignalBinding>,
    pub(crate) position: egui::Pos2,
    pub(crate) scale: f32,
    pub(crate) rect: Option<egui::Rect>,
}

pub(crate) struct AlarmViewItem {
    pub(crate) event: AlarmEvent,
    pub(crate) received_at: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct CanAlarmThresholds {
    pub(crate) x_high_input: String,
    pub(crate) x_low_input: String,
    pub(crate) y_high_input: String,
    pub(crate) y_low_input: String,
    pub(crate) z_high_input: String,
    pub(crate) z_low_input: String,
    pub(crate) x_high_applied: String,
    pub(crate) x_low_applied: String,
    pub(crate) y_high_applied: String,
    pub(crate) y_low_applied: String,
    pub(crate) z_high_applied: String,
    pub(crate) z_low_applied: String,
}

impl Default for CanAlarmThresholds {
    fn default() -> Self {
        Self {
            x_high_input: String::new(),
            x_low_input: String::new(),
            y_high_input: String::new(),
            y_low_input: String::new(),
            z_high_input: String::new(),
            z_low_input: String::new(),
            x_high_applied: String::new(),
            x_low_applied: String::new(),
            y_high_applied: String::new(),
            y_low_applied: String::new(),
            z_high_applied: String::new(),
            z_low_applied: String::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SentTorqueJumpThresholds {
    pub(crate) warn_input: String,
    pub(crate) red_input: String,
    pub(crate) purple_input: String,
    pub(crate) warn_applied: String,
    pub(crate) red_applied: String,
    pub(crate) purple_applied: String,
}

impl Default for SentTorqueJumpThresholds {
    fn default() -> Self {
        Self {
            warn_input: "0.2".to_string(),
            red_input: "0.3".to_string(),
            purple_input: "0.4".to_string(),
            warn_applied: "0.2".to_string(),
            red_applied: "0.3".to_string(),
            purple_applied: "0.4".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SentAngleJumpThresholds {
    pub(crate) t1_red_input: String,
    pub(crate) t2_red_input: String,
    pub(crate) s_red_input: String,
    pub(crate) t1_red_applied: String,
    pub(crate) t2_red_applied: String,
    pub(crate) s_red_applied: String,
}

impl Default for SentAngleJumpThresholds {
    fn default() -> Self {
        Self {
            t1_red_input: "0.2".to_string(),
            t2_red_input: "0.2".to_string(),
            s_red_input: "1.0".to_string(),
            t1_red_applied: "0.2".to_string(),
            t2_red_applied: "0.2".to_string(),
            s_red_applied: "1.0".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AlarmPlotPoint {
    pub(crate) point: [f64; 2],
    pub(crate) level: String,
    pub(crate) cleared: bool,
}

pub(crate) fn nearest_plot_point(points: &[egui_plot::PlotPoint], x_sec: f64) -> Option<[f64; 2]> {
    let insertion = points.partition_point(|point| point.x < x_sec);
    let nearest = match (insertion.checked_sub(1), points.get(insertion)) {
        (Some(left), Some(right)) => {
            let left = &points[left];
            if (left.x - x_sec).abs() <= (right.x - x_sec).abs() {
                left
            } else {
                right
            }
        }
        (Some(left), None) => &points[left],
        (None, Some(right)) => right,
        (None, None) => return None,
    };
    Some([x_sec, nearest.y])
}

#[derive(Debug, Default)]
pub(crate) struct CanReplayData {
    // PlotPoint lets egui_plot borrow these slices instead of rebuilding and
    // copying every series on every frame.
    pub(crate) x_points: Vec<egui_plot::PlotPoint>,
    pub(crate) y_points: Vec<egui_plot::PlotPoint>,
    pub(crate) z_points: Vec<egui_plot::PlotPoint>,
    pub(crate) u_points: Vec<egui_plot::PlotPoint>,
    pub(crate) v_points: Vec<egui_plot::PlotPoint>,
    pub(crate) x_alarm_points: Vec<AlarmPlotPoint>,
    pub(crate) y_alarm_points: Vec<AlarmPlotPoint>,
    pub(crate) z_alarm_points: Vec<AlarmPlotPoint>,
    pub(crate) u_alarm_points: Vec<AlarmPlotPoint>,
    pub(crate) v_alarm_points: Vec<AlarmPlotPoint>,
    pub(crate) min_ts_ms: i64,
    pub(crate) max_ts_ms: i64,
    pub(crate) raw_point_count: usize,
}

impl CanReplayData {
    pub(crate) fn is_empty(&self) -> bool {
        self.x_points.is_empty()
            && self.y_points.is_empty()
            && self.z_points.is_empty()
            && self.u_points.is_empty()
            && self.v_points.is_empty()
    }

    pub(crate) fn total_span_sec(&self) -> f64 {
        ((self.max_ts_ms - self.min_ts_ms).max(0) as f64) / 1000.0
    }

    pub(crate) fn displayed_point_count(&self) -> usize {
        self.x_points.len()
            + self.y_points.len()
            + self.z_points.len()
            + self.u_points.len()
            + self.v_points.len()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReplayMode {
    Can3Axis,
    Sent,
}

impl ReplayMode {
    pub(crate) fn load_status(self) -> &'static str {
        match self {
            Self::Can3Axis => "正在加载 3轴 回放数据...",
            Self::Sent => "正在加载 SENT 回放数据...",
        }
    }

    pub(crate) fn empty_status(self) -> &'static str {
        match self {
            Self::Can3Axis => "该时间段没有 3轴 数据",
            Self::Sent => "该时间段没有 SENT 数据",
        }
    }

    pub(crate) fn export_header(self) -> &'static str {
        match self {
            Self::Can3Axis => "# CAN 3-axis export",
            Self::Sent => "# SENT export",
        }
    }

    pub(crate) fn series_labels(self) -> [&'static str; 5] {
        match self {
            Self::Can3Axis => ["X", "Y", "Z", "", ""],
            Self::Sent => ["T1 angle", "T1 torque", "T2 angle", "T2 torque", "S angle"],
        }
    }

    pub(crate) fn axis_filters(self) -> [&'static str; 5] {
        match self {
            Self::Can3Axis => ["x", "y", "z", "__unused_4__", "__unused_5__"],
            Self::Sent => ["t1_angle", "t1_torque", "t2_angle", "t2_torque", "s_angle"],
        }
    }
}

pub(crate) struct CanReplayState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) exporting: bool,
    pub(crate) status: String,
    pub(crate) pg_dsn: String,
    pub(crate) start_ts_input: String,
    pub(crate) end_ts_input: String,
    pub(crate) mode: ReplayMode,
    pub(crate) show_x: bool,
    pub(crate) show_y: bool,
    pub(crate) show_z: bool,
    pub(crate) show_u: bool,
    pub(crate) show_v: bool,
    pub(crate) show_alarm_points: bool,
    pub(crate) data: Option<CanReplayData>,
    pub(crate) view_start_sec: f64,
    pub(crate) view_span_sec: f64,
    pub(crate) plot_rect: Option<egui::Rect>,
}

impl CanReplayState {
    pub(crate) fn new(pg_dsn: String) -> Self {
        Self {
            open: false,
            loading: false,
            exporting: false,
            status: "未加载".to_string(),
            pg_dsn,
            start_ts_input: String::new(),
            end_ts_input: String::new(),
            mode: ReplayMode::Can3Axis,
            show_x: true,
            show_y: true,
            show_z: true,
            show_u: true,
            show_v: true,
            show_alarm_points: true,
            data: None,
            view_start_sec: 0.0,
            view_span_sec: 60.0,
            plot_rect: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AlarmRecordRow {
    pub(crate) ts_ms: i64,
    pub(crate) device_id: String,
    pub(crate) alarm_id: String,
    pub(crate) level: String,
    pub(crate) message: String,
    pub(crate) cleared: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AlarmRecordData {
    pub(crate) records: Vec<AlarmRecordRow>,
    pub(crate) page_index: i64,
    pub(crate) has_next: bool,
    pub(crate) total_count: i64,
}

impl AlarmRecordData {
    pub(crate) fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

pub(crate) struct AlarmRecordState {
    pub(crate) open: bool,
    pub(crate) loading: bool,
    pub(crate) status: String,
    pub(crate) pg_dsn: String,
    pub(crate) start_ts_input: String,
    pub(crate) end_ts_input: String,
    pub(crate) show_can_x: bool,
    pub(crate) show_can_y: bool,
    pub(crate) show_can_z: bool,
    pub(crate) show_sent_t1: bool,
    pub(crate) show_sent_t2: bool,
    pub(crate) show_sent_s: bool,
    pub(crate) page_index: i64,
    pub(crate) has_next: bool,
    pub(crate) data: Option<AlarmRecordData>,
}

impl AlarmRecordState {
    pub(crate) fn new(pg_dsn: String) -> Self {
        Self {
            open: false,
            loading: false,
            status: "未加载".to_string(),
            pg_dsn,
            start_ts_input: String::new(),
            end_ts_input: String::new(),
            show_can_x: true,
            show_can_y: true,
            show_can_z: true,
            show_sent_t1: true,
            show_sent_t2: true,
            show_sent_s: true,
            page_index: 0,
            has_next: false,
            data: None,
        }
    }
}
