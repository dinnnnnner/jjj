use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug)]
pub(super) struct FrameTime {
    pub captured_at: SystemTime,
    pub t_sec: f64,
}

/// Hardware timestamps are relative microseconds, not Unix timestamps.
/// Anchor each channel at callback reception, before the ingress queue.
/// Absolute time includes the first callback's delivery latency. Callbacks must
/// be ordered per channel: a decreasing timestamp is treated as a clock reset.
#[derive(Default)]
pub(super) struct CanClock {
    origin: Option<SystemTime>,
    anchor: Option<(u64, SystemTime)>,
    last_timestamp_us: Option<u64>,
}

impl CanClock {
    pub fn capture(&mut self, timestamp_us: u64, received_at: SystemTime) -> FrameTime {
        let origin = *self.origin.get_or_insert(received_at);
        let captured_at = if timestamp_us == 0 {
            // A missing hardware timestamp must not acquire ingress queue delay.
            received_at
        } else {
            if self.anchor.is_none()
                || self
                    .last_timestamp_us
                    .is_some_and(|last| timestamp_us < last)
            {
                // A device clock restart begins a new mapping to host time.
                self.anchor = Some((timestamp_us, received_at));
            }
            self.last_timestamp_us = Some(timestamp_us);
            let (anchor_us, anchor_time) = self.anchor.expect("initialized above");
            anchor_time
                .checked_add(Duration::from_micros(timestamp_us - anchor_us))
                .unwrap_or(received_at)
        };
        FrameTime {
            captured_at,
            t_sec: captured_at
                .duration_since(origin)
                .unwrap_or_default()
                .as_secs_f64(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_spacing_survives_late_callbacks_and_clock_resets() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = CanClock::default();
        assert_eq!(clock.capture(1_000_000, start).captured_at, start);
        let next = clock.capture(1_002_000, start + Duration::from_secs(5));
        assert_eq!(next.captured_at, start + Duration::from_millis(2));
        assert_eq!(next.t_sec, 0.002);
        let reset = clock.capture(10, start + Duration::from_secs(6));
        assert_eq!(reset.captured_at, start + Duration::from_secs(6));
        assert_eq!(reset.t_sec, 6.0);
        let next = clock.capture(1_010, start + Duration::from_secs(7));
        assert_eq!(next.captured_at, start + Duration::from_millis(6001));
    }

    #[test]
    fn unavailable_clock_uses_callback_time_without_resetting_valid_anchor() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = CanClock::default();
        assert_eq!(clock.capture(0, start).captured_at, start);
        clock.capture(100, start + Duration::from_secs(1));
        assert_eq!(clock.capture(0, start + Duration::from_secs(2)).t_sec, 2.0);
        assert_eq!(
            clock.capture(1_100, start + Duration::from_secs(3)).t_sec,
            1.001
        );
    }
}
