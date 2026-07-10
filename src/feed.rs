use crate::bus::TelemetrySourceKind;
use crate::domain::AlarmEvent;
use serde::{Deserialize, Serialize};

pub const FEED_MAGIC: [u8; 4] = *b"JJJF";
pub const FEED_PROTOCOL_VERSION: u16 = 1;
const FEED_HEADER_LEN: usize = FEED_MAGIC.len() + size_of::<u16>();

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

#[derive(Debug, thiserror::Error)]
pub enum FeedDecodeError {
    #[error("feed frame is shorter than the protocol header")]
    TruncatedHeader,
    #[error("invalid feed protocol magic")]
    InvalidMagic,
    #[error("unsupported feed protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid feed payload: {0}")]
    InvalidPayload(#[from] bincode::Error),
}

pub fn encode_feed_msg(msg: &UiFeedMsg) -> bincode::Result<Vec<u8>> {
    let payload = bincode::serialize(msg)?;
    let mut frame = Vec::with_capacity(FEED_HEADER_LEN + payload.len());
    frame.extend_from_slice(&FEED_MAGIC);
    frame.extend_from_slice(&FEED_PROTOCOL_VERSION.to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_feed_msg(frame: &[u8]) -> Result<UiFeedMsg, FeedDecodeError> {
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

    Ok(bincode::deserialize(&frame[FEED_HEADER_LEN..])?)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Err(FeedDecodeError::UnsupportedVersion(2))
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
}
