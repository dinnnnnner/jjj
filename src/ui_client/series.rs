use super::messages::TelemetryMsg;
use crate::{MAX_POINTS_PER_SERIES, WINDOW_SECS};
use std::collections::VecDeque;

pub struct SensorSeries {
    pub points: VecDeque<[f64; 2]>,
    pub latest: Option<f64>,
    pub device_id: String,
}

impl SensorSeries {
    pub fn new() -> Self {
        Self {
            points: VecDeque::with_capacity(1024),
            latest: None,
            device_id: String::new(),
        }
    }

    pub fn push(&mut self, msg: &TelemetryMsg) {
        self.points.push_back([msg.t_sec, msg.value]);
        self.latest = Some(msg.value);
        self.device_id = msg.device_id.clone();

        let min_t = msg.t_sec - WINDOW_SECS;
        while let Some(front) = self.points.front() {
            if front[0] < min_t {
                let _ = self.points.pop_front();
            } else {
                break;
            }
        }
        while self.points.len() > MAX_POINTS_PER_SERIES {
            let _ = self.points.pop_front();
        }
    }
}
