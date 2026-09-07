//! Bounded, in-memory at-least-once persistence with connection recovery.
use super::{
    SCHEMA_SQL,
    writer::{DbCmd, TelemetryRow, flush_telemetry_batch, insert_alarm_event, insert_system_event},
};
use async_trait::async_trait;
use std::time::Duration;

pub enum WriteBatch {
    Telemetry(Vec<TelemetryRow>),
    Event(DbCmd),
}

#[async_trait]
pub trait WriteSink: Send {
    async fn write(&mut self, batch: &WriteBatch) -> anyhow::Result<()>;
}

pub struct PgSink {
    dsn: String,
    client: Option<tokio_postgres::Client>,
}

impl PgSink {
    pub fn new(dsn: String) -> Self {
        Self { dsn, client: None }
    }

    async fn try_write(&mut self, batch: &WriteBatch) -> anyhow::Result<()> {
        if self.client.is_none() {
            let (client, connection) =
                tokio_postgres::connect(&self.dsn, tokio_postgres::NoTls).await?;
            tokio::spawn(async move {
                let _ = connection.await;
            });
            client.batch_execute(SCHEMA_SQL).await?;
            self.client = Some(client);
        }
        let client = self.client.as_ref().expect("connected above");
        match batch {
            WriteBatch::Telemetry(rows) => flush_telemetry_batch(client, rows).await,
            WriteBatch::Event(DbCmd::Alarm(alarm)) => insert_alarm_event(client, alarm).await,
            WriteBatch::Event(DbCmd::System { level, message }) => {
                insert_system_event(client, level, message).await
            }
            WriteBatch::Event(DbCmd::Telemetry(row)) => {
                flush_telemetry_batch(client, std::slice::from_ref(row)).await
            }
        }
    }
}

#[async_trait]
impl WriteSink for PgSink {
    async fn write(&mut self, batch: &WriteBatch) -> anyhow::Result<()> {
        let result =
            match tokio::time::timeout(Duration::from_secs(10), self.try_write(batch)).await {
                Ok(result) => result,
                Err(_) => Err(anyhow::anyhow!("postgres operation timed out")),
            };
        if result.is_err() {
            self.client = None;
        }
        result
    }
}

pub async fn retry_write(
    sink: &mut impl WriteSink,
    batch: &WriteBatch,
    base_delay: Duration,
    mut on_error: impl FnMut(anyhow::Error),
) {
    let mut delay = base_delay.clamp(Duration::from_millis(1), Duration::from_secs(30));
    loop {
        match sink.write(batch).await {
            Ok(()) => return,
            Err(err) => on_error(err),
        }
        tokio::time::sleep(delay).await;
        delay = delay.saturating_mul(2).min(Duration::from_secs(30));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct RecoveringSink {
        failures: usize,
        seen: Vec<(i64, i64)>,
    }
    #[async_trait]
    impl WriteSink for RecoveringSink {
        async fn write(&mut self, batch: &WriteBatch) -> anyhow::Result<()> {
            let WriteBatch::Telemetry(rows) = batch else {
                panic!("expected telemetry")
            };
            self.seen.push((rows[0].captured_at_ms, rows[0].request_id));
            if self.failures > 0 {
                self.failures -= 1;
                anyhow::bail!("simulated disconnected database");
            }
            Ok(())
        }
    }
    #[tokio::test]
    async fn outage_retries_the_same_sample_without_changing_capture_time() {
        let mut sink = RecoveringSink {
            failures: 2,
            seen: vec![],
        };
        let batch = WriteBatch::Telemetry(vec![TelemetryRow {
            captured_at_ms: 1234,
            request_id: 42,
            device_id: "can://test:ch0".into(),
            sensor_id: 0,
            axis: "x".into(),
            alarm_bit: false,
            t_sec: 0.25,
            value: 17.0,
        }]);
        let mut failures = 0;
        retry_write(&mut sink, &batch, Duration::from_millis(1), |_| {
            failures += 1
        })
        .await;
        assert_eq!(failures, 2);
        assert_eq!(sink.seen, vec![(1234, 42); 3]);
    }
}
