use crate::domain::AlarmEvent;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct TelemetryRow {
    pub device_id: String,
    pub sensor_id: usize,
    pub axis: String,
    pub alarm_bit: bool,
    pub t_sec: f64,
    pub value: f64,
    pub request_id: u64,
}

pub enum DbCmd {
    Telemetry(TelemetryRow),
    Alarm(AlarmEvent),
    System { level: String, message: String },
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub async fn ensure_telemetry_partitions(
    client: &tokio_postgres::Client,
    ts_values: &[i64],
) -> anyhow::Result<()> {
    let mut days = ts_values
        .iter()
        .map(|ts_ms| ts_ms.div_euclid(86_400_000))
        .collect::<Vec<_>>();
    days.sort_unstable();
    days.dedup();

    for day in days {
        let day_start_ms = (day * 86_400_000) as f64;
        client
            .execute(
                "SELECT ensure_telemetry_partition_for_day(to_timestamp($1::double precision / 1000.0)::date)",
                &[&day_start_ms],
            )
            .await
            .map_err(|err| anyhow::anyhow!("ensure telemetry partition failed: {err}"))?;
    }

    Ok(())
}

pub async fn flush_telemetry_batch(
    client: &tokio_postgres::Client,
    batch: &mut Vec<TelemetryRow>,
) -> anyhow::Result<()> {
    if batch.is_empty() {
        return Ok(());
    }

    let mut ts_ms = Vec::with_capacity(batch.len());
    let mut device_ids = Vec::with_capacity(batch.len());
    let mut sensor_ids = Vec::with_capacity(batch.len());
    let mut axes = Vec::with_capacity(batch.len());
    let mut alarm_bits = Vec::with_capacity(batch.len());
    let mut t_secs = Vec::with_capacity(batch.len());
    let mut values = Vec::with_capacity(batch.len());
    let mut request_ids = Vec::with_capacity(batch.len());

    for item in batch.drain(..) {
        let row_ts_ms = now_ms();
        ts_ms.push(row_ts_ms);
        device_ids.push(item.device_id);
        sensor_ids.push(item.sensor_id as i32);
        axes.push(item.axis);
        alarm_bits.push(item.alarm_bit);
        t_secs.push(item.t_sec);
        values.push(item.value);
        request_ids.push(item.request_id as i64);
    }

    ensure_telemetry_partitions(client, &ts_ms).await?;

    client
        .execute(
            "INSERT INTO telemetry_samples (ts_ms, created_at, device_id, sensor_id, axis, alarm_bit, t_sec, value, request_id)
             SELECT
                unnest_ts_ms,
                to_timestamp(unnest_ts_ms::double precision / 1000.0),
                unnest_device_id,
                unnest_sensor_id,
                unnest_axis,
                unnest_alarm_bit,
                unnest_t_sec,
                unnest_value,
                unnest_request_id
             FROM UNNEST(
                $1::BIGINT[],
                $2::TEXT[],
                $3::INTEGER[],
                $4::TEXT[],
                $5::BOOLEAN[],
                $6::DOUBLE PRECISION[],
                $7::DOUBLE PRECISION[],
                $8::BIGINT[]
             ) AS t(
                unnest_ts_ms,
                unnest_device_id,
                unnest_sensor_id,
                unnest_axis,
                unnest_alarm_bit,
                unnest_t_sec,
                unnest_value,
                unnest_request_id
             )",
            &[
                &ts_ms,
                &device_ids,
                &sensor_ids,
                &axes,
                &alarm_bits,
                &t_secs,
                &values,
                &request_ids,
            ],
        )
        .await
        .map_err(|err| anyhow::anyhow!("telemetry batch insert failed: {err}"))?;

    Ok(())
}

pub async fn insert_alarm_event(
    client: &tokio_postgres::Client,
    alarm: &AlarmEvent,
) -> anyhow::Result<()> {
    let ts_ms = now_ms();
    client
        .execute(
            "INSERT INTO alarm_events (ts_ms, device_id, alarm_id, level, message, cleared)
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[
                &ts_ms,
                &alarm.device_id,
                &alarm.alarm_id,
                &format!("{:?}", alarm.level),
                &alarm.message,
                &alarm.cleared,
            ],
        )
        .await
        .map_err(|err| anyhow::anyhow!("alarm insert failed: {err}"))?;
    Ok(())
}

pub async fn insert_system_event(
    client: &tokio_postgres::Client,
    level: &str,
    message: &str,
) -> anyhow::Result<()> {
    let ts_ms = now_ms();
    client
        .execute(
            "INSERT INTO system_events (ts_ms, level, message)
             VALUES ($1, $2, $3)",
            &[&ts_ms, &level, &message],
        )
        .await
        .map_err(|err| anyhow::anyhow!("system insert failed: {err}"))?;
    Ok(())
}
