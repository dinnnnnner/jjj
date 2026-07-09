use crate::DISPLAY_TZ_OFFSET_SECS;
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn format_datetime_input(ts_ms: i64) -> String {
    let total_seconds = ts_ms.div_euclid(1000) + DISPLAY_TZ_OFFSET_SECS;
    let days = total_seconds.div_euclid(86_400);
    let secs = total_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs / 3600;
    let minute = (secs % 3600) / 60;
    let second = secs % 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

pub(crate) fn format_datetime_for_filename(ts_ms: i64) -> String {
    let total_seconds = ts_ms.div_euclid(1000) + DISPLAY_TZ_OFFSET_SECS;
    let days = total_seconds.div_euclid(86_400);
    let secs = total_seconds.rem_euclid(86_400);
    let (_year, month, day) = civil_from_days(days);
    let hour = secs / 3600;
    let minute = (secs % 3600) / 60;
    let second = secs % 60;

    format!("{month:02}-{day:02}_{hour:02}-{minute:02}-{second:02}")
}

pub(crate) fn format_alarm_datetime(time: SystemTime) -> String {
    let ts_ms = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let total_seconds = ts_ms.div_euclid(1000) + DISPLAY_TZ_OFFSET_SECS;
    let days = total_seconds.div_euclid(86_400);
    let secs = total_seconds.rem_euclid(86_400);
    let (_year, month, day) = civil_from_days(days);
    let hour = secs / 3600;
    let minute = (secs % 3600) / 60;
    let second = secs % 60;
    format!("{month:02}/{day:02} {hour:02}:{minute:02}:{second:02}")
}

pub(crate) fn default_can_export_filename(
    start_ts_ms: i64,
    end_ts_ms: i64,
    now_ts_ms: i64,
) -> String {
    format!(
        "can_export_{}_{}_{}.txt",
        format_datetime_for_filename(now_ts_ms),
        format_datetime_for_filename(start_ts_ms),
        format_datetime_for_filename(end_ts_ms)
    )
}

pub(crate) fn parse_time_input(text: &str) -> Option<i64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(ts_ms) = trimmed.parse::<i64>() {
        return Some(ts_ms);
    }

    let normalized = trimmed.replace('/', "-").replace('T', " ");
    let mut parts = normalized.split_whitespace();
    let date_part = parts.next()?;
    let time_part = parts.next().unwrap_or("00:00:00");
    if parts.next().is_some() {
        return None;
    }

    let mut date = date_part.split('-');
    let year = date.next()?.parse::<i64>().ok()?;
    let month = date.next()?.parse::<i64>().ok()?;
    let day = date.next()?.parse::<i64>().ok()?;
    if date.next().is_some() {
        return None;
    }

    let mut time = time_part.split(':');
    let hour = time.next()?.parse::<i64>().ok()?;
    let minute = time.next().unwrap_or("0").parse::<i64>().ok()?;
    let second = time.next().unwrap_or("0").parse::<i64>().ok()?;
    if time.next().is_some() {
        return None;
    }

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let local_seconds = days * 86_400 + hour * 3600 + minute * 60 + second;
    Some((local_seconds - DISPLAY_TZ_OFFSET_SECS) * 1000)
}

pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}
