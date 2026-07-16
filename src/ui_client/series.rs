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

/// Reduce a time-ordered series to roughly `max_points` while retaining each
/// bucket's extrema. This keeps short spikes visible and greatly reduces egui
/// tessellation work for high-rate feeds.
pub(crate) fn downsample_for_chart(
    points: &VecDeque<[f64; 2]>,
    max_points: usize,
) -> Vec<[f64; 2]> {
    if max_points == 0 || points.is_empty() {
        return Vec::new();
    }
    if points.len() <= max_points || max_points < 4 {
        return points.iter().copied().collect();
    }

    let interior_len = points.len() - 2;
    let bucket_count = ((max_points - 2) / 2).max(1);
    let bucket_size = interior_len.div_ceil(bucket_count);
    let mut sampled = Vec::with_capacity(max_points);
    sampled.push(points[0]);

    for bucket_start in (1..points.len() - 1).step_by(bucket_size) {
        let bucket_end = (bucket_start + bucket_size).min(points.len() - 1);
        let mut min_idx = bucket_start;
        let mut max_idx = bucket_start;
        for idx in bucket_start + 1..bucket_end {
            if points[idx][1] < points[min_idx][1] {
                min_idx = idx;
            }
            if points[idx][1] > points[max_idx][1] {
                max_idx = idx;
            }
        }

        match min_idx.cmp(&max_idx) {
            std::cmp::Ordering::Less => {
                sampled.push(points[min_idx]);
                sampled.push(points[max_idx]);
            }
            std::cmp::Ordering::Greater => {
                sampled.push(points[max_idx]);
                sampled.push(points[min_idx]);
            }
            std::cmp::Ordering::Equal => sampled.push(points[min_idx]),
        }
    }

    sampled.push(points[points.len() - 1]);
    sampled
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_downsampling_keeps_endpoints_and_spikes() {
        let mut points = (0..1_000).map(|x| [x as f64, 0.0]).collect::<VecDeque<_>>();
        points[321][1] = 100.0;
        points[654][1] = -80.0;

        let sampled = downsample_for_chart(&points, 100);

        assert!(sampled.len() <= 100);
        assert_eq!(sampled.first(), points.front());
        assert_eq!(sampled.last(), points.back());
        assert!(sampled.contains(&[321.0, 100.0]));
        assert!(sampled.contains(&[654.0, -80.0]));
        assert!(sampled.windows(2).all(|pair| pair[0][0] <= pair[1][0]));
    }

    #[test]
    fn chart_downsampling_leaves_small_series_unchanged() {
        let points = VecDeque::from([[1.0, 2.0], [2.0, 3.0], [3.0, 4.0]]);
        assert_eq!(
            downsample_for_chart(&points, 10),
            points.iter().copied().collect::<Vec<_>>()
        );
    }
}
