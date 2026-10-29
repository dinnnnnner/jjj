//! Run explicitly against a disposable PostgreSQL instance using a key/value DSN.
use demo2::db::retry::{PgSink, WriteBatch, WriteSink, retry_write};
use demo2::db::writer::TelemetryRow;
use futures_util::FutureExt;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

async fn in_test_database<F, Fut>(test: F)
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = ()>,
{
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
    let result = AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(45), test(db_dsn)))
        .catch_unwind()
        .await;
    // Clean up even if an assertion fails or a writer keeps retrying forever.
    admin
        .batch_execute(&format!("DROP DATABASE {db_name} WITH (FORCE)"))
        .await
        .unwrap();
    match result {
        Ok(result) => result.expect("database integration test timed out"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
#[ignore = "requires DEMO2_TEST_PG_DSN pointing to a disposable PostgreSQL instance"]
async fn reconnect_preserves_capture_times_and_cross_midnight_partitions() {
    in_test_database(|db_dsn| async move {
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
        let killed = query
            .query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
        WHERE datname = current_database() AND application_name = 'jjj_retry_test'",
                &[],
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
    })
    .await;
}

#[tokio::test]
#[ignore = "requires DEMO2_TEST_PG_DSN pointing to a disposable PostgreSQL instance"]
async fn concurrent_startup_and_batches_preserve_samples_and_alarm_events() {
    use demo2::db::writer::DbCmd;
    use demo2::domain::{AlarmEvent, AlarmLevel};
    const WRITERS: i64 = 4;
    const ROWS: i64 = 256;
    const START: i64 = 1_704_153_599_900;
    in_test_database(|db_dsn| async move {
        let writes = (0..WRITERS).map(|writer| {
            let db_dsn = &db_dsn;
            async move {
                let mut sink = PgSink::new(format!("{db_dsn} options='-c timezone=UTC'"));
                let samples = (0..ROWS).map(|sample| TelemetryRow {
                    captured_at_ms: START + sample,
                    device_id: format!("test-{writer}"),
                    sensor_id: 0,
                    axis: "x".into(),
                    alarm_bit: sample % 2 == 0,
                    t_sec: sample as f64 / 1000.0,
                    value: sample as f64,
                    request_id: writer * ROWS + sample,
                }).collect();
                retry_write(&mut sink, &WriteBatch::Telemetry(samples), Duration::from_millis(10), |_| {}).await;
                for cleared in [false, true] {
                    let event = AlarmEvent {
                        device_id: format!("test-{writer}"),
                        alarm_id: "test_alarm".into(),
                        level: AlarmLevel::Warning,
                        message: "concurrent persistence".into(),
                        raised_at: std::time::UNIX_EPOCH + Duration::from_millis(START as u64),
                        cleared,
                    };
                    retry_write(&mut sink, &WriteBatch::Event(DbCmd::Alarm(event)), Duration::from_millis(10), |_| {}).await;
                }
                retry_write(&mut sink, &WriteBatch::Event(DbCmd::System {
                    level: "info".into(), message: format!("writer {writer} completed"),
                }), Duration::from_millis(10), |_| {}).await;
            }
        });
        futures_util::future::join_all(writes).await;
        let (query, connection) = tokio_postgres::connect(&db_dsn, tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });
        let rows = query.query("SELECT ts_ms, request_id, value, alarm_bit, (extract(epoch FROM created_at)*1000)::bigint FROM telemetry_samples ORDER BY request_id", &[]).await.unwrap();
        assert_eq!(rows.len(), (WRITERS * ROWS) as usize);
        for (id, row) in rows.iter().enumerate() {
            let sample = id as i64 % ROWS;
            assert_eq!(row.get::<_, i64>(0), START + sample);
            assert_eq!(row.get::<_, i64>(1), id as i64);
            assert_eq!(row.get::<_, f64>(2), sample as f64);
            assert_eq!(row.get::<_, bool>(3), sample % 2 == 0);
            assert_eq!(row.get::<_, i64>(4), START + sample);
        }
        let alarms = query.query("SELECT device_id, cleared, ts_ms FROM alarm_events ORDER BY device_id, id", &[]).await.unwrap();
        assert_eq!(alarms.len(), (WRITERS * 2) as usize);
        for (i, alarm) in alarms.iter().enumerate() {
            assert_eq!(alarm.get::<_, String>(0), format!("test-{}", i / 2));
            assert_eq!(alarm.get::<_, bool>(1), i % 2 == 1);
            assert_eq!(alarm.get::<_, i64>(2), START);
        }
        let systems = query.query_one("SELECT count(*) FROM system_events", &[]).await.unwrap();
        assert_eq!(systems.get::<_, i64>(0), WRITERS);
    }).await;
}

struct CollectorProcess {
    child: Option<std::process::Child>,
    directory: std::path::PathBuf,
}

#[tokio::test]
#[ignore = "requires DEMO2_TEST_PG_DSN pointing to a disposable PostgreSQL instance"]
async fn unicode_system_and_alarm_messages_round_trip_without_corruption() {
    use demo2::db::writer::DbCmd;
    use demo2::domain::{AlarmEvent, AlarmLevel};
    in_test_database(|db_dsn| async move {
        let message = "数据库写入正常；CAN 自检结果：温度 25°C，扭矩 −12.5 N·m ✅";
        let mut sink = PgSink::new(db_dsn.clone());
        sink.write(&WriteBatch::Event(DbCmd::System {
            level: "info".into(),
            message: message.into(),
        }))
        .await
        .unwrap();
        sink.write(&WriteBatch::Event(DbCmd::Alarm(AlarmEvent {
            device_id: "测试设备".into(),
            alarm_id: "中文报警".into(),
            level: AlarmLevel::Warning,
            message: message.into(),
            raised_at: std::time::SystemTime::now(),
            cleared: false,
        })))
        .await
        .unwrap();
        let (query, connection) = tokio_postgres::connect(&db_dsn, tokio_postgres::NoTls)
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let row = query.query_one("SHOW client_encoding", &[]).await.unwrap();
        assert_eq!(row.get::<_, String>(0), "UTF8");
        let rows = query
            .query(
                "SELECT message FROM system_events UNION ALL SELECT message FROM alarm_events",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(row.get::<_, String>(0).as_bytes(), message.as_bytes());
        }
        let row = query
            .query_one("SELECT device_id, alarm_id FROM alarm_events", &[])
            .await
            .unwrap();
        assert_eq!(row.get::<_, String>(0), "测试设备");
        assert_eq!(row.get::<_, String>(1), "中文报警");
    })
    .await;
}

impl Drop for CollectorProcess {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        // This directory contains only the configuration created by this test.
        let _ = std::fs::remove_file(self.directory.join("config.toml"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[tokio::test]
#[ignore = "requires DEMO2_TEST_PG_DSN pointing to a disposable PostgreSQL instance"]
async fn collector_process_recovers_database_connection_without_losing_feed_samples() {
    use bytes::BytesMut;
    use demo2::feed::{UiFeedMsg, decode_feed_msg};
    use demo2::protocol::{Frame, FrameCodec, SimpleFrameCodec};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    in_test_database(|db_dsn| async move {
        let directory = std::env::temp_dir().join(format!("jjj_e2e_{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&directory).unwrap();
        let mut process = CollectorProcess { child: None, directory };
        let reservations: Vec<_> = (0..4).map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap()).collect();
        let ports: Vec<_> = reservations.iter().map(|listener| listener.local_addr().unwrap()).collect();
        let config = serde_json::json!({"collector": {
            "ingress_addr": ports[0].to_string(), "ui_feed_addr": ports[1].to_string(),
            "control_addr": ports[2].to_string(), "health_addr": ports[3].to_string(),
            "can_enabled": false, "serial_auto_detect": false, "pg_connect_retry_ms": 10,
            "pg_dsn": format!("{db_dsn} application_name=jjj_collector_e2e"),
        }});
        std::fs::write(process.directory.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
        drop(reservations);
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_collector_service"));
        command.current_dir(&process.directory)
            .env("DEMO2_DISABLE_DB", "0").env("DEMO2_COLLECTOR_CAN_ENABLED", "0")
            .env("DEMO2_COLLECTOR_SERIAL_AUTO_DETECT", "0")
            .env_remove("DEMO2_COLLECTOR_SERIAL_PORT")
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)] {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        process.child = Some(command.spawn().unwrap());
        let mut feed = loop {
            assert!(process.child.as_mut().unwrap().try_wait().unwrap().is_none(), "collector exited before readiness");
            if let Ok(socket) = TcpStream::connect(ports[1]).await { break socket; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        let (query, connection) = tokio_postgres::connect(&db_dsn, tokio_postgres::NoTls).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });
        let codec = SimpleFrameCodec::new(4096);
        let mut observed = std::collections::BTreeMap::new();
        for cycle in 0..2_u64 {
            let mut device = loop {
                if let Ok(socket) = TcpStream::connect(ports[0]).await { break socket; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            let mut batch = BytesMut::new();
            for id in cycle * 256 + 1..=cycle * 256 + 256 {
                codec.encode(&Frame { request_id: id, kind: 0x34,
                    payload: format!("sid=0,value={id}.5").into_bytes(),
                }, &mut batch).unwrap();
            }
            device.write_all(&batch[..5]).await.unwrap();
            device.write_all(&batch[5..]).await.unwrap();
            let mut received = BytesMut::new();
            for id in cycle * 256 + 1..=cycle * 256 + 256 {
                loop {
                    if let Some(ack) = codec.try_decode(&mut received).unwrap() {
                        assert_eq!((ack.request_id, ack.kind), (id, 0x90));
                        break;
                    }
                    assert!(device.read_buf(&mut received).await.unwrap() > 0);
                }
            }
            drop(device);
            let expected = ((cycle + 1) * 256) as usize;
            while observed.len() < expected {
                let length = feed.read_u32().await.unwrap() as usize;
                assert!(length <= demo2::feed::MAX_FEED_FRAME_LEN);
                let mut frame = vec![0; length];
                feed.read_exact(&mut frame).await.unwrap();
                if let UiFeedMsg::Telemetry(sample) = decode_feed_msg(&frame).unwrap() {
                    assert!(observed.insert(sample.request_id, (sample.captured_at_ms, sample.value)).is_none());
                }
            }
            loop {
                if let Ok(row) = query.query_one("SELECT count(*) FROM telemetry_samples", &[]).await
                    && row.get::<_, i64>(0) == expected as i64 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if cycle == 0 {
                let killed = query.query("SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=current_database() AND application_name='jjj_collector_e2e'", &[]).await.unwrap();
                assert_eq!(killed.len(), 1);
                assert!(killed[0].get::<_, bool>(0));
            }
        }
        let rows = query.query("SELECT request_id, ts_ms, value FROM telemetry_samples ORDER BY request_id", &[]).await.unwrap();
        assert_eq!(rows.len(), 512);
        for row in rows {
            let id = row.get::<_, i64>(0) as u64;
            let actual = (row.get::<_, i64>(1), row.get::<_, f64>(2));
            assert_eq!(actual, observed[&id]);
            assert_eq!(actual.1, id as f64 + 0.5);
        }
    }).await;
}
