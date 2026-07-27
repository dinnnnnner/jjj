use crate::bus::{AppEvent, DeviceEvent, EventBus, TelemetrySourceKind};
use crate::domain::{AlarmEvent, AlarmLevel};
use crate::protocol::demo_serial::{Packet as SerialPacket, SerialDemoCodec};
use crate::protocol::{FrameCodec, SentFrameCodec, SimpleFrameCodec};
use crate::transport::{SerialTransport, Transport, TransportError, available_serial_port_names};
use bytes::BytesMut;
use std::io;
use std::time::{Duration, Instant, SystemTime};

const DEMO_GROUP_RATE_HZ: f64 = 6250.0;
const AUTO_DETECT_PROBE_TIME: Duration = Duration::from_millis(800);
const AUTO_DETECT_RETRY_TIME: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SerialIngressMode {
    Legacy,
    Demo,
    Sent,
}

pub fn parse_serial_mode(text: &str) -> Option<SerialIngressMode> {
    match text.trim().to_ascii_lowercase().as_str() {
        "legacy" => Some(SerialIngressMode::Legacy),
        "demo" => Some(SerialIngressMode::Demo),
        "sent" | "sent1" | "sent2" | "sent3" => Some(SerialIngressMode::Sent),
        _ => None,
    }
}

pub fn parse_sensor_value(payload: &[u8]) -> Option<(usize, f64)> {
    let s = std::str::from_utf8(payload).ok()?.trim();
    let mut sid = None;
    let mut value = None;

    for part in s.split(',') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        if k.eq_ignore_ascii_case("sid") {
            sid = v.parse::<usize>().ok();
        } else if k.eq_ignore_ascii_case("value") {
            value = v.parse::<f64>().ok();
        }
    }

    Some((sid?, value?))
}

pub fn publish_status(bus: &EventBus, msg: impl Into<String>) {
    let msg = msg.into();
    tracing::info!(status = %msg);
    bus.publish(AppEvent::System(msg));
}

pub fn publish_telemetry_sample(
    bus: &EventBus,
    device_id: &str,
    sensor_id: usize,
    t_sec: f64,
    value: f64,
    req_id: u64,
    alarm_bit: bool,
    source_kind: TelemetrySourceKind,
) {
    bus.publish(AppEvent::Device(DeviceEvent::TelemetrySample {
        device_id: device_id.to_string(),
        sensor_id,
        t_sec,
        value,
        req_id,
        alarm_bit,
        source_kind,
    }));
}

fn contains_valid_frame(mode: SerialIngressMode, buf: &mut BytesMut) -> bool {
    match mode {
        SerialIngressMode::Legacy => {
            let codec = SimpleFrameCodec::new(4096);
            loop {
                match codec.try_decode(buf) {
                    Ok(Some(frame)) if parse_sensor_value(&frame.payload).is_some() => return true,
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return false,
                }
            }
        }
        SerialIngressMode::Demo => {
            let codec = SerialDemoCodec;
            matches!(codec.try_decode(buf), Ok(Some(_)))
        }
        SerialIngressMode::Sent => {
            let codec = SentFrameCodec;
            loop {
                match codec.try_decode(buf) {
                    Ok(Some(frame)) if matches!(frame.pause, 0x1 | 0x6 | 0xB) => return true,
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => return false,
                }
            }
        }
    }
}

async fn probe_serial_port(port_name: &str, baud_rate: u32, mode: SerialIngressMode) -> bool {
    let mut transport = SerialTransport::new(port_name, baud_rate);
    if transport.connect().await.is_err() {
        return false;
    }

    let deadline = Instant::now() + AUTO_DETECT_PROBE_TIME;
    let mut buf = BytesMut::with_capacity(8192);
    let mut detected = false;
    while Instant::now() < deadline {
        match transport.read(&mut buf).await {
            Ok(_) if contains_valid_frame(mode, &mut buf) => {
                detected = true;
                break;
            }
            Ok(_) => {}
            Err(TransportError::Io(err)) if err.kind() == io::ErrorKind::TimedOut => {}
            Err(_) => break,
        }
    }
    let _ = transport.close().await;
    detected
}

/// Continuously scans serial ports until one produces a valid frame for `mode`.
/// If the selected device disconnects, scanning resumes automatically.
pub async fn run_auto_serial_ingress(baud_rate: u32, mode: SerialIngressMode, bus: EventBus) {
    let mut last_candidates: Option<Vec<String>> = None;

    loop {
        let candidates = match available_serial_port_names() {
            Ok(ports) => ports,
            Err(err) => {
                publish_status(&bus, format!("collector serial auto-detect failed: {err}"));
                tokio::time::sleep(AUTO_DETECT_RETRY_TIME).await;
                continue;
            }
        };

        if last_candidates.as_ref() != Some(&candidates) {
            if candidates.is_empty() {
                publish_status(&bus, "collector serial auto-detect: no ports found");
            } else {
                publish_status(
                    &bus,
                    format!(
                        "collector serial auto-detect probing: {}",
                        candidates.join(", ")
                    ),
                );
            }
            last_candidates = Some(candidates.clone());
        }

        let mut selected = None;
        for port_name in candidates {
            if probe_serial_port(&port_name, baud_rate, mode).await {
                selected = Some(port_name);
                break;
            }
        }

        if let Some(port_name) = selected {
            publish_status(
                &bus,
                format!("collector serial auto-detected {port_name} @ {baud_rate}"),
            );
            run_serial_ingress(port_name.clone(), baud_rate, mode, bus.clone()).await;
            publish_status(
                &bus,
                format!("collector serial auto-detected port {port_name} ended; rescanning"),
            );
            last_candidates = None;
        }

        tokio::time::sleep(AUTO_DETECT_RETRY_TIME).await;
    }
}

pub async fn run_serial_ingress(
    port_name: String,
    baud_rate: u32,
    mode: SerialIngressMode,
    bus: EventBus,
) {
    let device_id = format!("serial://{}", port_name);
    let mut transport = SerialTransport::new(&port_name, baud_rate);
    if let Err(err) = transport.connect().await {
        publish_status(
            &bus,
            format!("collector serial open failed {port_name}@{baud_rate}: {err}"),
        );
        return;
    }
    publish_status(
        &bus,
        format!(
            "collector serial listening: {} @ {} ({})",
            port_name,
            baud_rate,
            match mode {
                SerialIngressMode::Legacy => "legacy",
                SerialIngressMode::Demo => "demo",
                SerialIngressMode::Sent => "sent",
            }
        ),
    );

    let start = Instant::now();
    let legacy_codec = SimpleFrameCodec::new(4096);
    let demo_codec = SerialDemoCodec;
    let sent_codec = SentFrameCodec;
    let mut legacy_buf = BytesMut::with_capacity(4096);
    let mut demo_buf = BytesMut::with_capacity(8192);
    let mut sent_buf = BytesMut::with_capacity(4096);
    let mut req_id = 1_u64;
    let mut demo_alarm_state: Option<bool> = None;

    loop {
        let target_buf = match mode {
            SerialIngressMode::Legacy => &mut legacy_buf,
            SerialIngressMode::Demo => &mut demo_buf,
            SerialIngressMode::Sent => &mut sent_buf,
        };

        match transport.read(target_buf).await {
            Ok(0) => {}
            Ok(_) => match mode {
                SerialIngressMode::Legacy => loop {
                    let frame = match legacy_codec.try_decode(&mut legacy_buf) {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break,
                        Err(err) => {
                            publish_status(
                                &bus,
                                format!("collector serial legacy decode error: {err}"),
                            );
                            break;
                        }
                    };

                    if let Some((sensor_id, value)) = parse_sensor_value(&frame.payload) {
                        publish_telemetry_sample(
                            &bus,
                            &device_id,
                            sensor_id,
                            start.elapsed().as_secs_f64(),
                            value,
                            frame.request_id,
                            false,
                            TelemetrySourceKind::FrameStream,
                        );
                    }
                },
                SerialIngressMode::Demo => loop {
                    let packet = match demo_codec.try_decode(&mut demo_buf) {
                        Ok(Some(packet)) => packet,
                        Ok(None) => break,
                        Err(err) => {
                            publish_status(
                                &bus,
                                format!("collector serial demo decode error: {err}"),
                            );
                            break;
                        }
                    };

                    match packet {
                        SerialPacket::Stream(stream) => {
                            for (group_idx, group) in stream.groups().enumerate() {
                                let t = start.elapsed().as_secs_f64()
                                    + (group_idx as f64 / DEMO_GROUP_RATE_HZ);
                                if demo_alarm_state != Some(group.ch1.alarm) {
                                    demo_alarm_state = Some(group.ch1.alarm);
                                    let alarm_event = AlarmEvent {
                                        device_id: device_id.clone(),
                                        alarm_id: "demo_alarm_bit".to_string(),
                                        level: AlarmLevel::Warning,
                                        message: format!(
                                            "demo alarm bit {}",
                                            if group.ch1.alarm {
                                                "asserted"
                                            } else {
                                                "cleared"
                                            }
                                        ),
                                        raised_at: SystemTime::now(),
                                        cleared: !group.ch1.alarm,
                                    };
                                    if group.ch1.alarm {
                                        bus.publish(AppEvent::Device(DeviceEvent::AlarmRaised(
                                            alarm_event,
                                        )));
                                    } else {
                                        bus.publish(AppEvent::Device(DeviceEvent::AlarmCleared(
                                            alarm_event,
                                        )));
                                    }
                                    publish_status(
                                        &bus,
                                        format!(
                                            "collector serial demo alarm {}",
                                            if group.ch1.alarm {
                                                "asserted"
                                            } else {
                                                "cleared"
                                            }
                                        ),
                                    );
                                }
                                let samples = [
                                    (0usize, group.ch1.x as f64),
                                    (1usize, group.ch1.y as f64),
                                    (2usize, group.ch1.z as f64),
                                ];
                                for (sensor_id, value) in samples {
                                    publish_telemetry_sample(
                                        &bus,
                                        &device_id,
                                        sensor_id,
                                        t,
                                        value,
                                        req_id,
                                        group.ch1.alarm,
                                        TelemetrySourceKind::SerialDemo,
                                    );
                                }
                                req_id = req_id.saturating_add(1);
                            }
                        }
                        SerialPacket::Response(resp) => {
                            publish_status(
                                &bus,
                                format!(
                                    "collector serial response cmd=0x{:02X} status=0x{:02X} payload_len={}",
                                    resp.cmd,
                                    resp.status,
                                    resp.payload.len()
                                ),
                            );
                        }
                    }
                },
                SerialIngressMode::Sent => loop {
                    let frame = match sent_codec.try_decode(&mut sent_buf) {
                        Ok(Some(frame)) => frame,
                        Ok(None) => break,
                        Err(err) => {
                            publish_status(
                                &bus,
                                format!("collector serial sent decode error: {err}"),
                            );
                            break;
                        }
                    };
                    let t = start.elapsed().as_secs_f64();

                    match frame.pause {
                        0x1 => {
                            publish_telemetry_sample(
                                &bus,
                                &device_id,
                                0,
                                t,
                                frame.channel_1 as f64,
                                req_id,
                                false,
                                TelemetrySourceKind::SerialSent1,
                            );
                            publish_telemetry_sample(
                                &bus,
                                &device_id,
                                1,
                                t,
                                frame.channel_2 as f64,
                                req_id,
                                false,
                                TelemetrySourceKind::SerialSent1,
                            );
                        }
                        0x6 => {
                            publish_telemetry_sample(
                                &bus,
                                &device_id,
                                2,
                                t,
                                frame.channel_1 as f64,
                                req_id,
                                false,
                                TelemetrySourceKind::SerialSent2,
                            );
                            publish_telemetry_sample(
                                &bus,
                                &device_id,
                                3,
                                t,
                                frame.channel_2 as f64,
                                req_id,
                                false,
                                TelemetrySourceKind::SerialSent2,
                            );
                        }
                        0xB => {
                            let secondary_angle = frame.channel_1 as f64;
                            publish_telemetry_sample(
                                &bus,
                                &device_id,
                                4,
                                t,
                                secondary_angle,
                                req_id,
                                false,
                                TelemetrySourceKind::SerialSent3,
                            );
                        }
                        other => {
                            publish_status(
                                &bus,
                                format!(
                                    "collector serial sent unsupported pause=0x{other:X} status=0x{:X}",
                                    frame.status
                                ),
                            );
                        }
                    }
                    req_id = req_id.saturating_add(1);
                },
            },
            Err(TransportError::Io(err)) if err.kind() == io::ErrorKind::TimedOut => {}
            Err(err) => {
                publish_status(
                    &bus,
                    format!("collector serial read error {port_name}: {err}"),
                );
                break;
            }
        }
    }

    let _ = transport.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Frame, SentFrameCodec};

    #[test]
    fn auto_detect_accepts_a_valid_sent_frame_after_noise() {
        let sent = SentFrameCodec::with_crc(0x2, 0x456, 0x789, 0x6);
        let mut raw = BytesMut::from(&b"noise"[..]);
        raw.extend_from_slice(&SentFrameCodec::encode_frame(sent));

        assert!(contains_valid_frame(SerialIngressMode::Sent, &mut raw));
    }

    #[test]
    fn auto_detect_rejects_sent_frame_with_unsupported_pause() {
        let sent = SentFrameCodec::with_crc(0x2, 0x456, 0x789, 0x2);
        let mut raw = BytesMut::from(&SentFrameCodec::encode_frame(sent)[..]);

        assert!(!contains_valid_frame(SerialIngressMode::Sent, &mut raw));
    }

    #[test]
    fn auto_detect_accepts_a_valid_legacy_sensor_frame() {
        let codec = SimpleFrameCodec::new(4096);
        let frame = Frame {
            request_id: 7,
            kind: 0x34,
            payload: b"sid=2,value=12.5".to_vec(),
        };
        let mut raw = BytesMut::new();
        codec.encode(&frame, &mut raw).unwrap();

        assert!(contains_valid_frame(SerialIngressMode::Legacy, &mut raw));
    }
}
