use crate::bus::TelemetrySourceKind;
use crate::domain::AlarmEvent;
use bincode::Options;
use serde::{Deserialize, Serialize};

pub const FEED_MAGIC: [u8; 4] = *b"JJJF";
pub const FEED_PROTOCOL_VERSION: u16 = 2;
pub const MAX_FEED_FRAME_LEN: usize = 1024 * 1024;
const FEED_HEADER_LEN: usize = FEED_MAGIC.len() + size_of::<u16>();
const MAX_FEED_PAYLOAD_LEN: u64 = (MAX_FEED_FRAME_LEN - FEED_HEADER_LEN) as u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetryMsg {
    pub captured_at_ms: i64,
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
    AlarmSnapshot(Vec<AlarmEvent>),
}

#[derive(Debug, thiserror::Error)]
pub enum FeedDecodeError {
    #[error("feed frame is shorter than the protocol header")]
    TruncatedHeader,
    #[error("feed frame length {actual} exceeds the {max}-byte limit")]
    FrameTooLarge { actual: usize, max: usize },
    #[error("invalid feed protocol magic")]
    InvalidMagic,
    #[error("unsupported feed protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid feed payload: {0}")]
    InvalidPayload(#[from] bincode::Error),
}

fn feed_options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_FEED_PAYLOAD_LEN)
        .reject_trailing_bytes()
}

pub fn encode_feed_msg(msg: &UiFeedMsg) -> bincode::Result<Vec<u8>> {
    let payload = feed_options().serialize(msg)?;
    let mut frame = Vec::with_capacity(FEED_HEADER_LEN + payload.len());
    frame.extend_from_slice(&FEED_MAGIC);
    frame.extend_from_slice(&FEED_PROTOCOL_VERSION.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_feed_msg(frame: &[u8]) -> Result<UiFeedMsg, FeedDecodeError> {
    if frame.len() > MAX_FEED_FRAME_LEN {
        return Err(FeedDecodeError::FrameTooLarge {
            actual: frame.len(),
            max: MAX_FEED_FRAME_LEN,
        });
    }
    if frame.len() < FEED_HEADER_LEN {
        return Err(FeedDecodeError::TruncatedHeader);
    }
    if frame[..FEED_MAGIC.len()] != FEED_MAGIC {
        return Err(FeedDecodeError::InvalidMagic);
    }

    let version = u16::from_be_bytes([frame[4], frame[5]]);
    if version != FEED_PROTOCOL_VERSION {
        return Err(FeedDecodeError::UnsupportedVersion(version));
    }

    Ok(feed_options().deserialize(&frame[FEED_HEADER_LEN..])?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_time_and_alarm_snapshots_roundtrip() {
        let msg = UiFeedMsg::Telemetry(TelemetryMsg {
            captured_at_ms: 123456789,
            device_id: "can://test:ch0".into(),
            sensor_id: 0,
            axis: "x".into(),
            alarm_bit: false,
            t_sec: 0.123,
            value: 1.5,
            request_id: 7,
            source_kind: TelemetrySourceKind::CanAxis,
        });
        let UiFeedMsg::Telemetry(decoded) =
            decode_feed_msg(&encode_feed_msg(&msg).unwrap()).unwrap()
        else {
            panic!()
        };
        assert_eq!(decoded.captured_at_ms, 123456789);
        assert_eq!(decoded.t_sec, 0.123);
        assert!(
            matches!(decode_feed_msg(&encode_feed_msg(&UiFeedMsg::AlarmSnapshot(vec![])).unwrap()).unwrap(),
            UiFeedMsg::AlarmSnapshot(alarms) if alarms.is_empty())
        );
    }

    #[test]
    fn feed_frame_roundtrips_with_versioned_header() {
        let frame = encode_feed_msg(&UiFeedMsg::Status("ready".to_string())).unwrap();

        assert_eq!(&frame[..4], &FEED_MAGIC);
        assert_eq!(
            u16::from_be_bytes([frame[4], frame[5]]),
            FEED_PROTOCOL_VERSION
        );
        assert!(matches!(
            decode_feed_msg(&frame).unwrap(),
            UiFeedMsg::Status(status) if status == "ready"
        ));
    }

    #[test]
    fn feed_frame_rejects_unknown_version() {
        let mut frame = encode_feed_msg(&UiFeedMsg::Status("ready".to_string())).unwrap();
        frame[4..6].copy_from_slice(&(FEED_PROTOCOL_VERSION + 1).to_be_bytes());

        assert!(matches!(
            decode_feed_msg(&frame),
            Err(FeedDecodeError::UnsupportedVersion(version)) if version == FEED_PROTOCOL_VERSION + 1
        ));
    }

    #[test]
    fn feed_frame_rejects_invalid_magic_and_truncated_header() {
        let mut frame = encode_feed_msg(&UiFeedMsg::Status("ready".to_string())).unwrap();
        frame[0] ^= 0xff;

        assert!(matches!(
            decode_feed_msg(&frame),
            Err(FeedDecodeError::InvalidMagic)
        ));
        assert!(matches!(
            decode_feed_msg(&frame[..5]),
            Err(FeedDecodeError::TruncatedHeader)
        ));
    }

    #[test]
    fn feed_frame_enforces_the_shared_size_limit() {
        let oversized = UiFeedMsg::Status("x".repeat(MAX_FEED_FRAME_LEN));

        assert!(matches!(
            encode_feed_msg(&oversized),
            Err(err) if matches!(*err, bincode::ErrorKind::SizeLimit)
        ));

        let oversized_frame = vec![0; MAX_FEED_FRAME_LEN + 1];

        let result = decode_feed_msg(&oversized_frame);
        assert!(
            matches!(
                result,
                Err(FeedDecodeError::FrameTooLarge { actual, max })
                    if actual == MAX_FEED_FRAME_LEN + 1 && max == MAX_FEED_FRAME_LEN
            ),
            "unexpected decode result: {result:?}"
        );
    }

    #[test]
    fn feed_frame_rejects_trailing_bytes() {
        let mut frame = encode_feed_msg(&UiFeedMsg::Status("ready".to_string())).unwrap();
        frame.push(0);

        assert!(matches!(
            decode_feed_msg(&frame),
            Err(FeedDecodeError::InvalidPayload(_))
        ));
    }
}
