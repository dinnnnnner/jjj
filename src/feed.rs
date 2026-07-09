use crate::bus::TelemetrySourceKind;
use crate::domain::AlarmEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetryMsg {
    pub device_id: String,
    pub sensor_id: usize,
    pub axis: String,
    pub alarm_bit: bool,
    pub t_sec: f64,
    pub value: f64,
    pub request_id: u64,
    pub source_kind: TelemetrySourceKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum UiFeedMsg {
    Telemetry(TelemetryMsg),
    Alarm(AlarmEvent),
    Status(String),
}
