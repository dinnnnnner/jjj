use crate::bus::{AppEvent, DeviceEvent, EventBus, TelemetrySourceKind};
use crate::domain::{AlarmEvent, AlarmLevel};
use crate::ingress::serial::publish_status;
use crate::protocol::can_data::{
    SentCanError, decode_axis_sample, decode_self_test_response, decode_sent_1, decode_sent_2,
    decode_sent_error, decode_sent_values,
};
use crate::signal::SentMovingAverage;
use crate::transport::can::{
    CanChannelConfig, CanFrame, CanTransport, CanTransportConfig, CanTransportError, CanTxFrame,
    HW_SUBTYPE_TC1012, HW_SUBTYPE_TC1016,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;
use std::time::SystemTime;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::TryRecvError};

static CAN_TX_SENDER: OnceLock<Mutex<Option<UnboundedSender<CanTxFrame>>>> = OnceLock::new();

pub fn enqueue_can_tx(frame: CanTxFrame) -> Result<(), String> {
    let slot = CAN_TX_SENDER.get_or_init(|| Mutex::new(None));
    let guard = slot
        .lock()
        .map_err(|_| "CAN TX queue lock poisoned".to_string())?;
    let Some(sender) = guard.as_ref() else {
        return Err("collector CAN channel not ready".to_string());
    };
    sender
        .send(frame)
        .map_err(|err| format!("collector CAN TX enqueue failed: {err}"))
}

fn set_can_tx_sender(sender: Option<UnboundedSender<CanTxFrame>>) -> Result<(), String> {
    let slot = CAN_TX_SENDER.get_or_init(|| Mutex::new(None));
    let mut guard = slot
        .lock()
        .map_err(|_| "CAN TX queue lock poisoned".to_string())?;
    *guard = sender;
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct SentFilterConfig {
    pub enabled: bool,
    pub window_size: usize,
}

impl Default for SentFilterConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_size: 10,
        }
    }
}

enum CanIngressEvent {
    Status(String),
    Log {
        device_id: String,
        level: &'static str,
        msg: String,
    },
    Telemetry {
        device_id: String,
        sensor_id: usize,
        value: f64,
        frame_count: u64,
        source_kind: TelemetrySourceKind,
    },
    AlarmRaised(AlarmEvent),
    AlarmCleared(AlarmEvent),
}

struct CanIngressCore {
    sent_filter_config: SentFilterConfig,
    channel_states: HashMap<u8, CanChannelRuntime>,
}

impl CanIngressCore {
    fn new(sent_filter_config: SentFilterConfig) -> Self {
        Self {
            sent_filter_config,
            channel_states: HashMap::new(),
        }
    }

    fn handle_frame(
        &mut self,
        config: &CanTransportConfig,
        frame: CanFrame,
        now: SystemTime,
    ) -> Vec<CanIngressEvent> {
        let device_id = can_device_id(config, frame.channel);
        let state = self
            .channel_states
            .entry(frame.channel)
            .or_insert_with(|| CanChannelRuntime {
                frame_count: 0,
                sent_filter: SentMovingAverage::new(self.sent_filter_config.window_size),
                active_sent_errors: HashSet::new(),
            });
        state.frame_count = state.frame_count.saturating_add(1);
        let frame_count = state.frame_count;
        let mut events = Vec::new();

        let is_tx = frame.is_tx();
        let identifier = frame.identifier;
        let data = frame.data_bytes();

        if let Some((x_ok, y_ok, z_ok)) = decode_self_test_response(is_tx, identifier, data) {
            events.push(CanIngressEvent::Status(format!(
                "CAN 自检结果: X={} Y={} Z={}",
                if x_ok { "通过" } else { "失败" },
                if y_ok { "通过" } else { "失败" },
                if z_ok { "通过" } else { "失败" },
            )));
        }

        if let Some(error) = decode_sent_error(is_tx, identifier, data) {
            reconcile_sent_error_events(
                &device_id,
                error,
                &mut state.active_sent_errors,
                now,
                &mut events,
            );
        }

        if let Some(values) = decode_sent_values(is_tx, identifier, data) {
            let output_values = if self.sent_filter_config.enabled {
                state.sent_filter.apply(values)
            } else {
                values
            };
            push_can_sent_value_events(&mut events, &device_id, frame_count, output_values);
        }
        if let Some(values) = decode_sent_1(is_tx, identifier, data) {
            push_can_sent_value_events(&mut events, &device_id, frame_count, values);
        }
        if let Some(values) = decode_sent_2(is_tx, identifier, data) {
            push_can_sent_value_events(&mut events, &device_id, frame_count, values);
        }
        if let Some((sensor_id, value)) = decode_axis_sample(identifier, data) {
            events.push(CanIngressEvent::Telemetry {
                device_id: device_id.clone(),
                sensor_id,
                value,
                frame_count,
                source_kind: TelemetrySourceKind::CanAxis,
            });
        }
        if frame_count <= 5 || frame_count % 100 == 0 {
            let msg = format!(
                "{} frame={} {}",
                device_id,
                frame_count,
                format_can_frame(&frame)
            );
            events.push(CanIngressEvent::Status(msg.clone()));
            events.push(CanIngressEvent::Log {
                device_id,
                level: "info",
                msg,
            });
        }

        events
    }
}

pub async fn run_can_ingress(
    config: CanTransportConfig,
    sent_filter_config: SentFilterConfig,
    bus: EventBus,
) {
    let Some((mut transport, active_config)) = connect_can_transport(config, &bus).await else {
        return;
    };
    let (tx_sender, mut tx_receiver) = mpsc::unbounded_channel();
    let _ = set_can_tx_sender(Some(tx_sender));

    publish_status(
        &bus,
        format!(
            "collector can listening: hw={} channels={}",
            active_config.hardware_name,
            format_can_channels(&active_config.channels)
        ),
    );

    let start = Instant::now();
    let mut core = CanIngressCore::new(sent_filter_config);
    loop {
        drain_tx_queue(&mut transport, &bus, &active_config, &mut tx_receiver).await;
        match transport.recv().await {
            Ok(Some(frame)) => {
                for event in core.handle_frame(&active_config, frame, SystemTime::now()) {
                    publish_can_ingress_event(&bus, &start, event);
                }
            }
            Ok(None) => {}
            Err(CanTransportError::CallbackDisconnected) => {
                publish_status(
                    &bus,
                    format!(
                        "collector can callback disconnected {}",
                        format_can_target(&active_config)
                    ),
                );
                break;
            }
            Err(err) => {
                publish_status(
                    &bus,
                    format!(
                        "collector can read error {}: {err}",
                        format_can_target(&active_config)
                    ),
                );
                break;
            }
        }
    }

    let _ = set_can_tx_sender(None);
    let _ = transport.close().await;
}

async fn connect_can_transport(
    config: CanTransportConfig,
    bus: &EventBus,
) -> Option<(CanTransport, CanTransportConfig)> {
    let candidates = candidate_can_configs(&config);
    let scanning = candidates.len() > 1;
    let mut failures = Vec::new();

    if scanning {
        publish_status(
            bus,
            format!(
                "collector can auto-detect enabled, probing {} candidate(s)",
                candidates.len()
            ),
        );
    }

    for candidate in candidates {
        let candidate_desc = format_can_target(&candidate);
        let mut transport = CanTransport::new(candidate.clone());
        match transport.connect().await {
            Ok(()) => {
                if candidate.hardware_name != config.hardware_name
                    || candidate.hardware_subtype != config.hardware_subtype
                    || candidate.channels != config.channels
                {
                    publish_status(bus, format!("collector can auto-detected {candidate_desc}"));
                }
                return Some((transport, candidate));
            }
            Err(err) => {
                if scanning {
                    publish_status(
                        bus,
                        format!("collector can probe failed {candidate_desc}: {err}"),
                    );
                }
                failures.push(format!("{candidate_desc}: {err}"));
            }
        }
    }

    let requested = format_can_target(&config);
    let detail = failures.join(" | ");
    publish_status(
        bus,
        format!("collector can open failed {requested}; tried: {detail}"),
    );
    None
}

fn candidate_can_configs(config: &CanTransportConfig) -> Vec<CanTransportConfig> {
    let mut result = Vec::new();
    let mut seen = HashSet::new();

    let requested_name = config.hardware_name.trim();
    let is_auto_name = requested_name.is_empty() || requested_name.eq_ignore_ascii_case("auto");

    let hardware_candidates = if is_auto_name {
        vec![
            ("TC1016".to_string(), HW_SUBTYPE_TC1016),
            ("TC1012".to_string(), HW_SUBTYPE_TC1012),
        ]
    } else {
        vec![(requested_name.to_string(), config.hardware_subtype)]
    };

    let configured_channels = normalized_can_channels(config);
    let channel_candidates = if configured_channels.len() == 1 {
        let first = configured_channels[0].clone();
        let mut channels = Vec::new();
        channels.push(vec![first.clone()]);
        for channel in 0..4 {
            let channel = channel as u8;
            if channel != first.index {
                let mut candidate_channel = first.clone();
                candidate_channel.index = channel;
                channels.push(vec![candidate_channel]);
            }
        }
        channels
    } else {
        vec![configured_channels]
    };

    for (hardware_name, hardware_subtype) in hardware_candidates {
        for channels in &channel_candidates {
            let mut candidate = config.clone();
            candidate.hardware_name = hardware_name.clone();
            candidate.hardware_subtype = hardware_subtype;
            candidate.channels = channels.clone();
            let key = (
                candidate.hardware_name.clone(),
                candidate.hardware_subtype,
                format_can_channels(&candidate.channels),
            );
            if seen.insert(key) {
                result.push(candidate);
            }
        }
    }

    result
}

fn format_can_target(config: &CanTransportConfig) -> String {
    format!(
        "hw={} subtype={} channels={}",
        config.hardware_name,
        config.hardware_subtype,
        format_can_channels(&config.channels)
    )
}

fn format_can_channels(channels: &[CanChannelConfig]) -> String {
    channels
        .iter()
        .map(|ch| {
            format!(
                "ch{}:{}kbps/{}kbps",
                ch.index, ch.arbitration_baud_kbps, ch.data_baud_kbps
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn normalized_can_channels(config: &CanTransportConfig) -> Vec<CanChannelConfig> {
    if config.channels.is_empty() {
        CanTransportConfig::default().channels
    } else {
        config.channels.clone()
    }
}

fn can_device_id(config: &CanTransportConfig, channel: u8) -> String {
    format!("can://{}:ch{}", config.hardware_name, channel)
}

async fn drain_tx_queue(
    transport: &mut CanTransport,
    bus: &EventBus,
    config: &CanTransportConfig,
    rx: &mut UnboundedReceiver<CanTxFrame>,
) {
    loop {
        let frame = match rx.try_recv() {
            Ok(frame) => frame,
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
        };
        let device_id = can_device_id(config, frame.channel);
        if let Err(err) = transport.transmit(frame).await {
            publish_status(bus, format!("collector can tx failed {device_id}: {err}"));
        }
    }
}

fn publish_can_ingress_event(bus: &EventBus, start: &Instant, event: CanIngressEvent) {
    match event {
        CanIngressEvent::Status(msg) => publish_status(bus, msg),
        CanIngressEvent::Log {
            device_id,
            level,
            msg,
        } => {
            bus.publish(AppEvent::Device(DeviceEvent::Log {
                device_id,
                level,
                msg,
            }));
        }
        CanIngressEvent::Telemetry {
            device_id,
            sensor_id,
            value,
            frame_count,
            source_kind,
        } => {
            bus.publish(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id,
                sensor_id,
                t_sec: start.elapsed().as_secs_f64(),
                value,
                req_id: frame_count,
                alarm_bit: false,
                source_kind,
            }));
        }
        CanIngressEvent::AlarmRaised(alarm) => {
            bus.publish(AppEvent::Device(DeviceEvent::AlarmRaised(alarm)));
        }
        CanIngressEvent::AlarmCleared(alarm) => {
            bus.publish(AppEvent::Device(DeviceEvent::AlarmCleared(alarm)));
        }
    }
}

fn push_can_sent_value_events(
    events: &mut Vec<CanIngressEvent>,
    device_id: &str,
    frame_count: u64,
    values: impl IntoIterator<Item = (usize, f64)>,
) {
    for (sensor_id, value) in values {
        events.push(CanIngressEvent::Telemetry {
            device_id: device_id.to_string(),
            sensor_id,
            value,
            frame_count,
            source_kind: TelemetrySourceKind::CanSent,
        });
    }
}

struct CanChannelRuntime {
    frame_count: u64,
    sent_filter: SentMovingAverage,
    active_sent_errors: HashSet<u8>,
}

fn reconcile_sent_error_events(
    device_id: &str,
    error: SentCanError,
    active_errors: &mut HashSet<u8>,
    now: SystemTime,
    events: &mut Vec<CanIngressEvent>,
) {
    if error.error_type == 0 {
        for error_type in active_errors.drain().collect::<Vec<_>>() {
            push_sent_error_events(device_id, error_type, error, true, now, events);
        }
        return;
    }

    if active_errors.insert(error.error_type) {
        push_sent_error_events(device_id, error.error_type, error, false, now, events);
    }
}

fn push_sent_error_events(
    device_id: &str,
    error_type: u8,
    error: SentCanError,
    cleared: bool,
    now: SystemTime,
    events: &mut Vec<CanIngressEvent>,
) {
    let detail = format!(
        "SENT error type={} t1={} t2={} s={} {}",
        error_type,
        error.t1_error as u8,
        error.t2_error as u8,
        error.s_error as u8,
        if cleared { "cleared" } else { "active" }
    );
    let alarm = AlarmEvent {
        device_id: device_id.to_string(),
        alarm_id: format!("sent_error_{error_type}"),
        level: AlarmLevel::Warning,
        message: detail.clone(),
        raised_at: now,
        cleared,
    };
    if cleared {
        events.push(CanIngressEvent::AlarmCleared(alarm));
    } else {
        events.push(CanIngressEvent::AlarmRaised(alarm));
    }
    events.push(CanIngressEvent::Status(detail));
}

fn format_can_frame(frame: &CanFrame) -> String {
    let direction = if frame.is_tx() { "TX" } else { "RX" };
    let payload = frame
        .data_bytes()
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{direction} ch={} id=0x{:03X} dlc={} ts={}us data={payload}",
        frame.channel, frame.identifier, frame.dlc, frame.timestamp_us
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis_frame(channel: u8, value: i32) -> CanFrame {
        let mut data = [0u8; 64];
        data[..4].copy_from_slice(&value.to_be_bytes());
        CanFrame {
            channel,
            properties: 0,
            dlc: 4,
            identifier: 0x100,
            timestamp_us: 0,
            data,
        }
    }

    fn axis_frame_count(events: &[CanIngressEvent]) -> Option<u64> {
        events.iter().find_map(|event| match event {
            CanIngressEvent::Telemetry {
                source_kind: TelemetrySourceKind::CanAxis,
                frame_count,
                ..
            } => Some(*frame_count),
            _ => None,
        })
    }

    #[test]
    fn can_core_keeps_frame_count_order_per_channel() {
        let mut core = CanIngressCore::new(SentFilterConfig::default());
        let config = CanTransportConfig::default();
        let now = SystemTime::UNIX_EPOCH;

        let first_ch0 = core.handle_frame(&config, axis_frame(0, 10), now);
        let first_ch1 = core.handle_frame(&config, axis_frame(1, 20), now);
        let second_ch0 = core.handle_frame(&config, axis_frame(0, 30), now);

        assert_eq!(axis_frame_count(&first_ch0), Some(1));
        assert_eq!(axis_frame_count(&first_ch1), Some(1));
        assert_eq!(axis_frame_count(&second_ch0), Some(2));
    }

    #[test]
    fn explicit_tc1016_probes_its_four_physical_channels() {
        assert_eq!(HW_SUBTYPE_TC1016, 11);

        let config = CanTransportConfig::default();
        let candidates = candidate_can_configs(&config);

        assert_eq!(candidates.len(), 4);
        assert!(candidates.iter().all(|candidate| {
            candidate.hardware_name == "TC1016" && candidate.hardware_subtype == HW_SUBTYPE_TC1016
        }));
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.channels[0].index)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn auto_hardware_name_pairs_models_with_their_subtypes() {
        let mut config = CanTransportConfig::default();
        config.hardware_name = "auto".to_string();
        let candidates = candidate_can_configs(&config);

        assert!(candidates.iter().any(|candidate| {
            candidate.hardware_name == "TC1016" && candidate.hardware_subtype == HW_SUBTYPE_TC1016
        }));
        assert!(candidates.iter().any(|candidate| {
            candidate.hardware_name == "TC1012" && candidate.hardware_subtype == HW_SUBTYPE_TC1012
        }));
    }
}
