use biquad::{Biquad, Coefficients, DirectForm1, ToHertz, Type};
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;

#[derive(Clone, Debug)]
pub struct RawSample {
    pub device_id: String,
    pub sensor_id: usize,
    pub t_sec: f64,
    pub value: f64,
    pub req_id: u64,
}

#[derive(Clone, Debug)]
pub struct SignalSample {
    pub signal_id: String,
    pub device_id: String,
    pub t_sec: f64,
    pub value: f64,
    pub req_id: u64,
}

#[derive(Clone, Debug)]
pub struct SignalSpec {
    pub id: String,
    pub name: String,
    pub unit: String,
    pub decimals: usize,
    pub kind: SignalKind,
}

#[derive(Clone, Debug)]
pub enum SignalKind {
    SourceSensor { sensor_id: usize },
    Derived { formula: DerivedFormula },
}

#[derive(Clone, Debug)]
pub enum DerivedFormula {
    ScaleOffset {
        input_signal_id: String,
        scale: f64,
        offset: f64,
    },
    OffsetScale {
        input_signal_id: String,
        input_offset: f64,
        scale: f64,
    },
}

pub struct SignalProcessor {
    specs: Vec<SignalSpec>,
    latest_values: HashMap<(String, String), SignalSample>,
}

impl SignalProcessor {
    pub fn new(specs: Vec<SignalSpec>) -> Self {
        Self {
            specs,
            latest_values: HashMap::new(),
        }
    }

    pub fn specs(&self) -> &[SignalSpec] {
        &self.specs
    }

    pub fn spec(&self, signal_id: &str) -> Option<&SignalSpec> {
        self.specs.iter().find(|spec| spec.id == signal_id)
    }

    pub fn ingest_raw(&mut self, raw: RawSample) -> Vec<SignalSample> {
        let mut out = Vec::new();

        for spec in &self.specs {
            if let SignalKind::SourceSensor { sensor_id } = spec.kind
                && sensor_id == raw.sensor_id
            {
                out.push(SignalSample {
                    signal_id: spec.id.clone(),
                    device_id: raw.device_id.clone(),
                    t_sec: raw.t_sec,
                    value: raw.value,
                    req_id: raw.req_id,
                });
            }
        }

        for sample in &out {
            self.latest_values.insert(
                (sample.device_id.clone(), sample.signal_id.clone()),
                sample.clone(),
            );
        }

        let mut derived = Vec::new();
        // Only recompute formulas whose input was updated by this raw sample.
        // Recomputing every derived signal for every sensor duplicates stale
        // values and multiplies UI series work at high frame rates.
        for source in &out {
            for spec in &self.specs {
                if let SignalKind::Derived { formula } = &spec.kind
                    && formula.input_signal_id() == source.signal_id
                    && let Some(sample) = self.compute_derived(
                        formula,
                        &spec.id,
                        &raw.device_id,
                        raw.t_sec,
                        raw.req_id,
                    )
                {
                    derived.push(sample);
                }
            }
        }

        for sample in &derived {
            self.latest_values.insert(
                (sample.device_id.clone(), sample.signal_id.clone()),
                sample.clone(),
            );
        }

        out.extend(derived);
        out
    }

    fn compute_derived(
        &self,
        formula: &DerivedFormula,
        signal_id: &str,
        device_id: &str,
        t_sec: f64,
        req_id: u64,
    ) -> Option<SignalSample> {
        match formula {
            DerivedFormula::ScaleOffset {
                input_signal_id,
                scale,
                offset,
            } => {
                let input = self
                    .latest_values
                    .get(&(device_id.to_string(), input_signal_id.clone()))?;
                Some(SignalSample {
                    signal_id: signal_id.to_string(),
                    device_id: device_id.to_string(),
                    t_sec,
                    value: input.value * *scale + *offset,
                    req_id,
                })
            }
            DerivedFormula::OffsetScale {
                input_signal_id,
                input_offset,
                scale,
            } => {
                let input = self
                    .latest_values
                    .get(&(device_id.to_string(), input_signal_id.clone()))?;
                Some(SignalSample {
                    signal_id: signal_id.to_string(),
                    device_id: device_id.to_string(),
                    t_sec,
                    value: (input.value - *input_offset) * *scale,
                    req_id,
                })
            }
        }
    }
}

impl DerivedFormula {
    fn input_signal_id(&self) -> &str {
        match self {
            Self::ScaleOffset {
                input_signal_id, ..
            }
            | Self::OffsetScale {
                input_signal_id, ..
            } => input_signal_id,
        }
    }
}

pub fn default_signal_specs(sensor_count: usize) -> Vec<SignalSpec> {
    let mut specs = Vec::new();

    for sensor_id in 0..sensor_count {
        let (name, unit) = match sensor_id {
            0 => ("sent_t1_angle".to_string(), "deg".to_string()),
            1 => ("sent_t1_torque".to_string(), "Nm".to_string()),
            2 => ("sent_t2_angle".to_string(), "deg".to_string()),
            3 => ("sent_t2_torque".to_string(), "Nm".to_string()),
            4 => ("sent_s_angle".to_string(), "deg".to_string()),
            _ => (format!("Sensor {}", sensor_id), "raw".to_string()),
        };

        specs.push(SignalSpec {
            id: format!("sensor_{sensor_id}_raw"),
            name,
            unit,
            decimals: 3,
            kind: SignalKind::SourceSensor { sensor_id },
        });
    }

    specs.push(SignalSpec {
        id: "sensor_0_angle".to_string(),
        name: "sensor_P1_angle".to_string(),
        unit: "deg".to_string(),
        decimals: 3,
        kind: SignalKind::Derived {
            formula: DerivedFormula::OffsetScale {
                input_signal_id: "sensor_0_raw".to_string(),
                input_offset: 2048.0,
                scale: 40.0 / 4092.0,
            },
        },
    });
    specs.push(SignalSpec {
        id: "sensor_1_angle".to_string(),
        name: "sensor_T1_angle".to_string(),
        unit: "deg".to_string(),
        decimals: 3,
        kind: SignalKind::Derived {
            formula: DerivedFormula::OffsetScale {
                input_signal_id: "sensor_1_raw".to_string(),
                input_offset: 2047.5,
                scale: 12.0 / 4079.0,
            },
        },
    });
    specs.push(SignalSpec {
        id: "sensor_2_angle".to_string(),
        name: "sensor_P2_angle".to_string(),
        unit: "deg".to_string(),
        decimals: 3,
        kind: SignalKind::Derived {
            formula: DerivedFormula::OffsetScale {
                input_signal_id: "sensor_2_raw".to_string(),
                input_offset: 2048.0,
                scale: -40.0 / 4092.0,
            },
        },
    });
    specs.push(SignalSpec {
        id: "sensor_3_angle".to_string(),
        name: "sensor_T2_angle".to_string(),
        unit: "deg".to_string(),
        decimals: 3,
        kind: SignalKind::Derived {
            formula: DerivedFormula::OffsetScale {
                input_signal_id: "sensor_3_raw".to_string(),
                input_offset: 2047.5,
                scale: -12.0 / 4079.0,
            },
        },
    });

    specs
}

pub struct SentMovingAverage {
    window_size: usize,
    windows: [VecDeque<f64>; 5],
}

impl SentMovingAverage {
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size: window_size.max(1),
            windows: std::array::from_fn(|_| VecDeque::with_capacity(window_size.max(1))),
        }
    }

    pub fn apply(&mut self, values: [(usize, f64); 5]) -> [(usize, f64); 5] {
        values.map(|(sensor_id, value)| (sensor_id, self.apply_sample(sensor_id, value)))
    }

    /// Update only the signal present in this frame; other windows stay intact.
    pub fn apply_sample(&mut self, sensor_id: usize, value: f64) -> f64 {
        let Some(window) = self.windows.get_mut(sensor_id) else {
            return value;
        };
        if !value.is_finite() {
            return value;
        }
        window.push_back(value);
        while window.len() > self.window_size {
            window.pop_front();
        }
        if matches!(sensor_id, 0 | 2 | 4) {
            circular_mean_degrees(window, value)
        } else {
            window.iter().sum::<f64>() / window.len() as f64
        }
    }
}

pub fn circular_mean_degrees(samples: &VecDeque<f64>, reference: f64) -> f64 {
    if samples.is_empty() {
        return reference;
    }

    let (sin_sum, cos_sum) = samples
        .iter()
        .fold((0.0, 0.0), |(sin_sum, cos_sum), value| {
            let radians = value.to_radians();
            (sin_sum + radians.sin(), cos_sum + radians.cos())
        });

    let mut mean = sin_sum.atan2(cos_sum).to_degrees();
    while mean - reference > 180.0 {
        mean -= 360.0;
    }
    while reference - mean > 180.0 {
        mean += 360.0;
    }
    mean
}

pub struct ButterworthCascade {
    sections: Vec<DirectForm1<f32>>,
}

#[derive(Clone, Copy, Debug)]
pub struct ButterworthConfig {
    pub order: usize,
    pub sample_rate_hz: f32,
    pub cutoff_hz: f32,
}

impl ButterworthCascade {
    pub fn new(order: usize, sample_rate_hz: f32, cutoff_hz: f32) -> anyhow::Result<Self> {
        Self::from_config(ButterworthConfig {
            order,
            sample_rate_hz,
            cutoff_hz,
        })
    }

    pub fn from_config(config: ButterworthConfig) -> anyhow::Result<Self> {
        anyhow::ensure!(config.order >= 2, "filter order must be at least 2");
        anyhow::ensure!(
            config.order % 2 == 0,
            "filter order must be even for cascaded Butterworth sections"
        );
        anyhow::ensure!(
            config.sample_rate_hz > 0.0,
            "filter sample rate must be positive"
        );
        anyhow::ensure!(config.cutoff_hz > 0.0, "filter cutoff must be positive");
        anyhow::ensure!(
            config.cutoff_hz < config.sample_rate_hz * 0.5,
            "filter cutoff must be below Nyquist"
        );

        let section_count = config.order / 2;
        let mut sections = Vec::with_capacity(section_count);

        for index in 1..=section_count {
            let q = butterworth_section_q(config.order, index);
            let coeffs = Coefficients::<f32>::from_params(
                Type::LowPass,
                config.sample_rate_hz.hz(),
                config.cutoff_hz.hz(),
                q,
            )
            .map_err(|err| anyhow::anyhow!("build biquad coeffs failed: {err:?}"))?;
            sections.push(DirectForm1::<f32>::new(coeffs));
        }

        Ok(Self { sections })
    }

    pub fn run(&mut self, sample: f64) -> f64 {
        let mut x = sample as f32;
        for section in &mut self.sections {
            x = section.run(x);
        }
        x as f64
    }
}

pub struct KeyedButterworthFilter<K> {
    config: ButterworthConfig,
    filters: HashMap<K, ButterworthCascade>,
}

impl<K> KeyedButterworthFilter<K>
where
    K: Eq + Hash,
{
    pub fn new(config: ButterworthConfig) -> Self {
        Self {
            config,
            filters: HashMap::new(),
        }
    }

    pub fn apply(&mut self, key: K, value: f64) -> anyhow::Result<f64> {
        match self.filters.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                Ok(entry.get_mut().run(value))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut filter = ButterworthCascade::from_config(self.config)?;
                let filtered = filter.run(value);
                entry.insert(filter);
                Ok(filtered)
            }
        }
    }
}

fn butterworth_section_q(order: usize, section_index: usize) -> f32 {
    let theta = ((2 * section_index - 1) as f32 * std::f32::consts::PI) / (2.0 * order as f32);
    1.0 / (2.0 * theta.cos())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn estimate_amplitude(samples: &[f64], sample_rate_hz: f64, freq_hz: f64) -> f64 {
        let n = samples.len() as f64;
        let mut sin_acc = 0.0;
        let mut cos_acc = 0.0;

        for (idx, sample) in samples.iter().enumerate() {
            let phase = 2.0 * std::f64::consts::PI * freq_hz * idx as f64 / sample_rate_hz;
            sin_acc += sample * phase.sin();
            cos_acc += sample * phase.cos();
        }

        2.0 * (sin_acc.hypot(cos_acc)) / n
    }

    #[test]
    fn derived_signal_is_only_emitted_when_its_source_changes() {
        let mut processor = SignalProcessor::new(default_signal_specs(2));

        let sensor_zero = processor.ingest_raw(RawSample {
            device_id: "test".to_string(),
            sensor_id: 0,
            t_sec: 1.0,
            value: 2048.0,
            req_id: 1,
        });
        assert!(
            sensor_zero
                .iter()
                .any(|sample| sample.signal_id == "sensor_0_angle")
        );

        let sensor_one = processor.ingest_raw(RawSample {
            device_id: "test".to_string(),
            sensor_id: 1,
            t_sec: 2.0,
            value: 2047.5,
            req_id: 2,
        });
        assert!(
            sensor_one
                .iter()
                .all(|sample| sample.signal_id != "sensor_0_angle")
        );
        assert!(
            sensor_one
                .iter()
                .any(|sample| sample.signal_id == "sensor_1_angle")
        );
    }

    #[test]
    fn sent_angle_filter_wraps_across_zero_degrees() {
        let mut filter = SentMovingAverage::new(3);

        let first = filter.apply([(0, 359.0), (1, 10.0), (2, 0.0), (3, 20.0), (4, 0.0)]);
        assert!((first[0].1 - 359.0).abs() < 0.001);

        let second = filter.apply([(0, 0.0), (1, 20.0), (2, 0.0), (3, 40.0), (4, 0.0)]);
        assert!(
            second[0].1 > 359.0 || second[0].1 < 1.0,
            "angle average should stay near wrap boundary, got {}",
            second[0].1
        );

        let third = filter.apply([(0, 1.0), (1, 30.0), (2, 0.0), (3, 60.0), (4, 0.0)]);
        assert!(
            third[0].1 > -1.0 && third[0].1 < 2.0,
            "359/0/1 should average near 0 degrees, got {}",
            third[0].1
        );
    }

    #[test]
    fn sent_torque_filter_uses_arithmetic_average() {
        let mut filter = SentMovingAverage::new(3);

        let _ = filter.apply([(0, 0.0), (1, 10.0), (2, 0.0), (3, 20.0), (4, 0.0)]);
        let _ = filter.apply([(0, 0.0), (1, 20.0), (2, 0.0), (3, 40.0), (4, 0.0)]);
        let third = filter.apply([(0, 0.0), (1, 30.0), (2, 0.0), (3, 60.0), (4, 0.0)]);

        assert!((third[1].1 - 20.0).abs() < 0.001);
        assert!((third[3].1 - 40.0).abs() < 0.001);
    }

    #[test]
    fn butterworth_lowpass_keeps_low_freq_and_reduces_high_freq() {
        let sample_rate_hz = 48_000.0;
        let low_freq_hz = 1_000.0;
        let high_freq_hz = 10_000.0;
        let mut filter = ButterworthCascade::new(10, sample_rate_hz as f32, 4_000.0).unwrap();

        let total_samples = 12_000usize;
        let mut output = Vec::with_capacity(total_samples);

        for idx in 0..total_samples {
            let t = idx as f64 / sample_rate_hz;
            let input = (2.0 * std::f64::consts::PI * low_freq_hz * t).sin()
                + (2.0 * std::f64::consts::PI * high_freq_hz * t).sin();
            output.push(filter.run(input));
        }

        let steady_state = &output[4_000..];
        let low_amp = estimate_amplitude(steady_state, sample_rate_hz, low_freq_hz);
        let high_amp = estimate_amplitude(steady_state, sample_rate_hz, high_freq_hz);

        assert!(
            low_amp > 0.7,
            "expected low-frequency component to remain visible, got amplitude {low_amp}"
        );
        assert!(
            high_amp < 0.1,
            "expected high-frequency component to be attenuated, got amplitude {high_amp}"
        );
        assert!(
            low_amp > high_amp * 8.0,
            "expected low-frequency component to dominate after filtering, low={low_amp}, high={high_amp}"
        );
    }

    #[test]
    fn butterworth_lowpass_accepts_manual_input_samples() {
        let mut filter = ButterworthCascade::new(10, 48_000.0, 4_000.0).unwrap();
        let mut input = Vec::new();
        input.extend(std::iter::repeat_n(0.0, 80));
        input.extend(std::iter::repeat_n(100.0, 160));
        input.extend(std::iter::repeat_n(0.0, 160));

        let output = input
            .iter()
            .map(|sample| filter.run(*sample))
            .collect::<Vec<_>>();

        assert_eq!(output.len(), input.len());
    }

    #[test]
    fn keyed_butterworth_filter_keeps_state_per_key() {
        let config = ButterworthConfig {
            order: 2,
            sample_rate_hz: 100.0,
            cutoff_hz: 10.0,
        };
        let mut filter = KeyedButterworthFilter::new(config);

        let a_first = filter.apply("a", 100.0).unwrap();
        let b_first = filter.apply("b", 100.0).unwrap();
        let a_second = filter.apply("a", 100.0).unwrap();

        assert_eq!(a_first, b_first);
        assert_ne!(a_first, a_second);
    }
}
