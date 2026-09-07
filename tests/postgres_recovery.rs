//! Run explicitly against a disposable PostgreSQL instance using a key/value DSN.
use demo2::db::retry::{PgSink, WriteBatch, WriteSink, retry_write};
use demo2::db::writer::TelemetryRow;
use std::time::Duration;

#[tokio::test]
#[ignore = "requires DEMO2_TEST_PG_DSN pointing to a disposable PostgreSQL instance"]
async fn reconnect_preserves_capture_times_and_cross_midnight_partitions() {
    let dsn = std::env::var("DEMO2_TEST_PG_DSN").expect("disposable PostgreSQL DSN required");
    let (admin, connection) = tokio_postgres::connect(&dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let db_name = format!("jjj_recovery_{}", uuid::Uuid::new_v4().simple());
    admin
        .batch_execute(&format!("CREATE DATABASE {db_name}"))
        .await
        .unwrap();
    let db_dsn = format!("{dsn} dbname={db_name}");
    let (query, connection) = tokio_postgres::connect(&db_dsn, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let mut sink = PgSink::new(format!(
        "{db_dsn} application_name=jjj_retry_test options='-c timezone=America/New_York'"
    ));
    let times = query
        .query_one(
            "SELECT
        (extract(epoch FROM '2024-01-01 23:59:59-05'::timestamptz)*1000)::bigint,
        (extract(epoch FROM '2024-01-02 00:00:01-05'::timestamptz)*1000)::bigint",
            &[],
        )
        .await
        .unwrap();
    let first: i64 = times.get(0);
    let second: i64 = times.get(1);
    let row = |captured_at_ms, request_id| TelemetryRow {
        captured_at_ms,
        device_id: "can://test:ch0".into(),
        sensor_id: 0,
        axis: "x".into(),
        alarm_bit: false,
        t_sec: request_id as f64,
        value: 42.0,
        request_id,
    };
    sink.write(&WriteBatch::Telemetry(vec![row(first, 1), row(second, 2)]))
        .await
        .unwrap();
    // Terminate only the writer connection in the database created by this test.
    let killed = admin
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
        WHERE datname = $1 AND application_name = 'jjj_retry_test'",
            &[&db_name],
        )
        .await
        .unwrap();
    assert_eq!(killed.len(), 1);
    let mut failures = 0;
    tokio::time::timeout(
        Duration::from_secs(20),
        retry_write(
            &mut sink,
            &WriteBatch::Telemetry(vec![row(second + 1000, 3)]),
            Duration::from_millis(10),
            |_| failures += 1,
        ),
    )
    .await
    .unwrap();
    assert!(failures >= 1);
    let rows = query
        .query(
            "SELECT ts_ms, request_id FROM telemetry_samples ORDER BY ts_ms, id",
            &[],
        )
        .await
        .unwrap();
    let actual: Vec<(i64, i64)> = rows.iter().map(|r| (r.get(0), r.get(1))).collect();
    assert_eq!(actual, vec![(first, 1), (second, 2), (second + 1000, 3)]);
    drop(sink);
    drop(query);
    admin
        .batch_execute(&format!("DROP DATABASE {db_name} WITH (FORCE)"))
        .await
        .unwrap();
}
