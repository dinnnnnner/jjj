use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
pub enum TelemetrySourceKind {
    #[default]
    Unknown,
    SerialDemo,
    SerialSent1,
    SerialSent2,
    SerialSent3,
    CanAxis,
    CanSent,
    TcpFrame,
    FrameStream,
}

pub fn axis_name(source_kind: TelemetrySourceKind, sensor_id: usize) -> &'static str {
    match source_kind {
        TelemetrySourceKind::SerialDemo | TelemetrySourceKind::CanAxis => match sensor_id {
            0 => "x",
            1 => "y",
            2 => "z",
            _ => "",
        },
        TelemetrySourceKind::CanSent
        | TelemetrySourceKind::SerialSent1
        | TelemetrySourceKind::SerialSent2
        | TelemetrySourceKind::SerialSent3 => match sensor_id {
            0 => "t1_angle",
            1 => "t1_torque",
            2 => "t2_angle",
            3 => "t2_torque",
            4 => "s_angle",
            _ => "",
        },
        _ => "",
    }
}

pub fn is_sent_torque_sample(source_kind: TelemetrySourceKind, sensor_id: usize) -> bool {
    is_sent_sample_source(source_kind) && matches!(sensor_id, 1 | 3)
}

pub fn is_sent_angle_sample(source_kind: TelemetrySourceKind, sensor_id: usize) -> bool {
    is_sent_sample_source(source_kind) && matches!(sensor_id, 0 | 2 | 4)
}

pub fn sent_torque_label(sensor_id: usize) -> &'static str {
    match sensor_id {
        1 => "T1 torque",
        3 => "T2 torque",
        _ => "SENT torque",
    }
}

pub fn sent_angle_label(sensor_id: usize) -> &'static str {
    match sensor_id {
        0 => "T1 angle",
        2 => "T2 angle",
        4 => "S angle",
        _ => "SENT angle",
    }
}

fn is_sent_sample_source(source_kind: TelemetrySourceKind) -> bool {
    matches!(
        source_kind,
        TelemetrySourceKind::CanSent
            | TelemetrySourceKind::SerialSent1
            | TelemetrySourceKind::SerialSent2
            | TelemetrySourceKind::SerialSent3
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_axis_names_by_source() {
        assert_eq!(axis_name(TelemetrySourceKind::CanAxis, 0), "x");
        assert_eq!(axis_name(TelemetrySourceKind::SerialDemo, 2), "z");
        assert_eq!(axis_name(TelemetrySourceKind::CanSent, 3), "t2_torque");
        assert_eq!(axis_name(TelemetrySourceKind::TcpFrame, 3), "");
    }

    #[test]
    fn classifies_sent_samples() {
        assert!(is_sent_torque_sample(TelemetrySourceKind::CanSent, 1));
        assert!(is_sent_angle_sample(TelemetrySourceKind::SerialSent3, 4));
        assert!(!is_sent_torque_sample(TelemetrySourceKind::CanAxis, 1));
    }
}
