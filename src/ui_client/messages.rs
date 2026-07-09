use demo2::bus::TelemetrySourceKind;
use demo2::domain::AlarmEvent;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct TelemetryMsg {
    pub device_id: String,
    pub sensor_id: usize,
    pub t_sec: f64,
    pub value: f64,
    pub request_id: u64,
    #[serde(default)]
    pub alarm_bit: bool,
    #[serde(default)]
    pub source_kind: TelemetrySourceKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum FeedMsg {
    Telemetry(TelemetryMsg),
    Alarm(AlarmEvent),
    Status(String),
}
