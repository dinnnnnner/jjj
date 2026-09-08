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
    last_captured_at: Option<SystemTime>,
}

impl CanClock {
    pub fn capture(&mut self, timestamp_us: u64, received_at: SystemTime) -> FrameTime {
        let origin = *self.origin.get_or_insert(received_at);
        let captured_at = if timestamp_us == 0 {
            // Resume from a fresh mapping after fallback, rather than jumping
            // back to the old hardware timeline on the next valid timestamp.
            self.anchor = None;
            self.last_timestamp_us = None;
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
        // Host time can move backward too. Keep points ordered and re-anchor
        // valid hardware time so subsequent frames retain their spacing.
        let captured_at = self
            .last_captured_at
            .map_or(captured_at, |last| last.max(captured_at));
        if timestamp_us != 0 {
            self.anchor = Some((timestamp_us, captured_at));
        }
        self.last_captured_at = Some(captured_at);
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
    fn unavailable_clock_reanchors_the_next_valid_timestamp() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = CanClock::default();
        assert_eq!(clock.capture(0, start).captured_at, start);
        clock.capture(100, start + Duration::from_secs(1));
        assert_eq!(clock.capture(0, start + Duration::from_secs(2)).t_sec, 2.0);
        assert_eq!(
            clock.capture(1_100, start + Duration::from_secs(3)).t_sec,
            3.0
        );
        assert_eq!(
            clock.capture(2_100, start + Duration::from_secs(4)).t_sec,
            3.001
        );
    }

    #[test]
    fn repeated_missing_timestamps_and_host_clock_rollback_stay_ordered() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut clock = CanClock::default();
        let inputs = [(100, 0), (0, 5), (0, 4), (1_100, 3), (2_100, 6), (10, 2)];
        let expected = [0.0, 5.0, 5.0, 5.0, 5.001, 5.001];
        let mut previous = start;
        for ((hardware, received_secs), t_sec) in inputs.into_iter().zip(expected) {
            let frame = clock.capture(hardware, start + Duration::from_secs(received_secs));
            assert!(frame.captured_at >= previous);
            assert_eq!(frame.t_sec, t_sec);
            previous = frame.captured_at;
        }
    }
}
