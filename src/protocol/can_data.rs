#[derive(Clone, Copy, Debug)]
pub struct SentCanError {
    pub error_type: u8,
    pub t1_error: bool,
    pub t2_error: bool,
    pub s_error: bool,
}

pub fn decode_sent_values(is_tx: bool, identifier: u32, data: &[u8]) -> Option<[(usize, f64); 5]> {
    if is_tx || matches!(identifier, 1 | 2 | 3) {
        return None;
    }
    if data.len() < 53 {
        return None;
    }

    Some([
        (0, read_f32_le(data, 25)? as f64), // T1 angle
        (1, read_f32_le(data, 29)? as f64), // T1 torque
        (2, read_f32_le(data, 1)? as f64),  // T2 angle
        (3, read_f32_le(data, 5)? as f64),  // T2 torque
        (4, read_f32_le(data, 49)? as f64), // S angle
    ])
}

pub fn decode_sent_1(is_tx: bool, identifier: u32, data: &[u8]) -> Option<Vec<(usize, f64)>> {
    if is_tx || identifier != 1 {
        return None;
    }

    let flag: u8 = data.first().copied()?;
    match flag {
        1 => {
            let t2_angle = read_i16_le(data, 6)?;
            let t2_torque = read_i16_le(data, 8)?;

            let angle = (2047.0 - t2_angle as f64) * 10.0 / 1023.0;
            let torque = (4095.0 / 2.0 - t2_torque as f64) * 12.0 / 4079.0;

            Some(vec![(2, angle), (3, torque)])
        }
        2 => {
            let s = read_i16_le(data, 4)?;
            let value = (s as f64 - 4089.0 / 2.0) * 296.0 / 4087.0;

            Some(vec![(4, value)])
        }
        _ => None,
    }
}

pub fn decode_sent_2(is_tx: bool, identifier: u32, data: &[u8]) -> Option<Vec<(usize, f64)>> {
    if is_tx || identifier != 2 {
        return None;
    }

    let t1_angle: i16 = read_i16_le(data, 6)?;
    let t1_torque: i16 = read_i16_le(data, 8)?;

    let t1_angle_f64 = (-2047.0 + t1_angle as f64) * 10.0 / 1023.0;
    let t1_torque_f64 = (-4095.0 / 2.0 + t1_torque as f64) * 12.0 / 4079.0;

    Some(vec![(0, t1_angle_f64), (1, t1_torque_f64)])
}

pub fn decode_sent_error(is_tx: bool, identifier: u32, data: &[u8]) -> Option<SentCanError> {
    if is_tx || identifier != 3 {
        return None;
    }
    if data.len() < 4 {
        return None;
    }
    Some(SentCanError {
        error_type: data[0],
        t1_error: data[1] != 0,
        t2_error: data[2] != 0,
        s_error: data[3] != 0,
    })
}

pub fn decode_axis_sample(identifier: u32, data: &[u8]) -> Option<(usize, f64)> {
    let sensor_id = match identifier {
        0x100 => 0,
        0x102 => 1,
        0x104 => 2,
        _ => return None,
    };
    if data.len() < 4 {
        return None;
    }

    let raw = i32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    Some((sensor_id, raw as f64))
}

pub fn decode_self_test_response(
    is_tx: bool,
    identifier: u32,
    data: &[u8],
) -> Option<(bool, bool, bool)> {
    if is_tx || identifier != 0x123 {
        return None;
    }
    if data.len() < 3 {
        return None;
    }
    Some((data[0] == 1, data[1] == 1, data[2] == 1))
}

fn read_f32_le(data: &[u8], offset: usize) -> Option<f32> {
    let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(f32::from_le_bytes(bytes))
}

fn read_i16_le(data: &[u8], offset: usize) -> Option<i16> {
    let bytes: [u8; 2] = data.get(offset..offset + 2)?.try_into().ok()?;
    Some(i16::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sent_1_flag_selects_t2_or_s_values() {
        let mut data = [0u8; 10];
        data[0] = 1;
        data[6..8].copy_from_slice(&i16::MIN.to_le_bytes());
        data[8..10].copy_from_slice(&i16::MAX.to_le_bytes());
        assert_eq!(
            decode_sent_1(false, 1, &data),
            Some(vec![(2, 340.3225806451613), (3, -90.37362098553567)])
        );

        data[0] = 2;
        data[4..6].copy_from_slice(&0x1234_i16.to_le_bytes());
        assert_eq!(
            decode_sent_1(false, 1, &data),
            Some(vec![(4, 189.42696354294102)])
        );

        data[0] = 3;
        assert!(decode_sent_1(false, 1, &data).is_none());
    }

    #[test]
    fn specialized_sent_ids_skip_the_generic_decoder() {
        let data = [0u8; 64];
        for identifier in [1, 2, 3] {
            assert!(decode_sent_values(false, identifier, &data).is_none());
        }
    }

    #[test]
    fn axis_sample_reads_big_endian_i32() {
        let data = 123_i32.to_be_bytes();
        assert_eq!(decode_axis_sample(0x102, &data), Some((1, 123.0)));
    }

    #[test]
    fn sent_error_ignores_tx_frames() {
        assert!(decode_sent_error(true, 3, &[1, 1, 0, 0]).is_none());
        let error = decode_sent_error(false, 3, &[2, 1, 0, 1]).unwrap();
        assert_eq!(error.error_type, 2);
        assert!(error.t1_error);
        assert!(!error.t2_error);
        assert!(error.s_error);
    }
}
