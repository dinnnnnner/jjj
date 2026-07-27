use demo2::app::{AlarmService, SentFrameGapAlarmConfig, SentJumpAlarmConfig};
use demo2::bus::{AppEvent, DeviceEvent, EventBus, Store, TelemetrySourceKind};
use demo2::config::{CollectorConfig, collector_can_channels, env_flag, load_collector_config};
use demo2::db::SCHEMA_SQL;
use demo2::db::writer::{
    DbCmd, TelemetryRow, flush_telemetry_batch, insert_alarm_event, insert_system_event,
};
use demo2::domain::telemetry::axis_name;
use demo2::feed::{TelemetryMsg, UiFeedMsg, encode_feed_msg};
use demo2::ingress::can::{
    CanSignalWatchdogConfig, CanSignalWatchdogTarget, SentFilterConfig, run_can_ingress,
};
use demo2::ingress::serial::{
    SerialIngressMode, parse_serial_mode, publish_status, run_auto_serial_ingress,
    run_serial_ingress,
};
use demo2::ingress::tcp::run_tcp_ingress;
use demo2::protocol::SimpleFrameCodec;
use demo2::session::{DeviceSession, DeviceSessionHandle, SessionConfig};
use demo2::signal::{ButterworthConfig, KeyedButterworthFilter};
use demo2::transport::SerialTransport;
use demo2::transport::can::{CanTransportConfig, HW_SUBTYPE_TC1012, HW_SUBTYPE_TC1016};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tokio_postgres::NoTls;
use tracing::{info, warn};

const DB_CMD_CHANNEL_CAPACITY: usize = 50_000;
const TELEMETRY_BATCH_SIZE: usize = 256;
const TELEMETRY_FLUSH_INTERVAL: Duration = Duration::from_millis(25);
const DEMO_UI_STRIDE: u64 = 8;

#[derive(Default)]
struct CollectorStats {
    ingress_ready: AtomicBool,
    ui_ready: AtomicBool,
    ingress_connections: AtomicU64,
    ui_clients: AtomicU64,
    samples_rx: AtomicU64,
    ui_drop: AtomicU64,
    db_drop: AtomicU64,
    db_write_fail: AtomicU64,
    last_db_error: Mutex<Option<String>>,
}

fn telemetry_row_from_msg(msg: TelemetryMsg) -> anyhow::Result<TelemetryRow> {
    Ok(TelemetryRow {
        device_id: msg.device_id,
        sensor_id: i32::try_from(msg.sensor_id).map_err(|_| {
            anyhow::anyhow!(
                "sensor_id out of PostgreSQL INTEGER range: {}",
                msg.sensor_id
            )
        })?,
        axis: msg.axis,
        alarm_bit: msg.alarm_bit,
        t_sec: msg.t_sec,
        value: msg.value,
        request_id: i64::try_from(msg.request_id).map_err(|_| {
            anyhow::anyhow!(
                "request_id out of PostgreSQL BIGINT range: {}",
                msg.request_id
            )
        })?,
    })
}

fn telemetry_msg_from_sample(
    device_id: String,
    sensor_id: usize,
    t_sec: f64,
    value: f64,
    req_id: u64,
    alarm_bit: bool,
    source_kind: TelemetrySourceKind,
) -> TelemetryMsg {
    TelemetryMsg {
        device_id,
        sensor_id,
        axis: axis_name(source_kind, sensor_id).to_string(),
        alarm_bit,
        t_sec,
        value,
        request_id: req_id,
        source_kind,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ControlCmd {
    SetSentJumpThresholds {
        torque_warn: f64,
        torque_red: f64,
        torque_purple: f64,
        angle_t1_red: Option<f64>,
        angle_t2_red: Option<f64>,
        angle_t_red: Option<f64>,
        angle_s_red: f64,
    },
    SetSentFrameGapThresholds {
        t1_us: u64,
        t2_us: u64,
        s_us: u64,
    },
}

#[derive(Clone, Copy)]
struct PersistenceFilterConfig {
    enabled: bool,
    filter: ButterworthConfig,
}

struct PersistenceFilter {
    config: PersistenceFilterConfig,
    filters: KeyedButterworthFilter<(String, usize, String)>,
    init_error_logged: bool,
}

impl PersistenceFilter {
    fn new(config: PersistenceFilterConfig) -> Self {
        Self {
            filters: KeyedButterworthFilter::new(config.filter),
            config,
            init_error_logged: false,
        }
    }

    fn apply(&mut self, mut msg: TelemetryMsg) -> TelemetryMsg {
        if !self.config.enabled {
            return msg;
        }

        let key = (msg.device_id.clone(), msg.sensor_id, msg.axis.clone());
        match self.filters.apply(key, msg.value) {
            Ok(filtered) => {
                msg.value = filtered;
            }
            Err(err) => {
                if !self.init_error_logged {
                    warn!(error = %err, "db Butterworth filter init failed, bypassing filter");
                    self.init_error_logged = true;
                }
            }
        }

        msg
    }
}

fn record_db_error(stats: &CollectorStats, message: String) {
    if let Ok(mut guard) = stats.last_db_error.lock() {
        *guard = Some(message);
    }
}

fn record_db_write_error(stats: &CollectorStats, op: &'static str, err: anyhow::Error) {
    stats.db_write_fail.fetch_add(1, Ordering::Relaxed);
    record_db_error(stats, format!("{op}: {err}"));
    warn!(error = %err, op, "postgres write failed");
}

async fn flush_db_telemetry_batch(
    client: &tokio_postgres::Client,
    batch: &mut Vec<TelemetryRow>,
    stats: &CollectorStats,
) -> bool {
    match flush_telemetry_batch(client, batch).await {
        Ok(()) => true,
        Err(err) => {
            record_db_write_error(stats, "telemetry_batch", err);
            false
        }
    }
}

async fn start_pg_writer(
    cfg: &CollectorConfig,
    stats: Arc<CollectorStats>,
) -> anyhow::Result<mpsc::Sender<DbCmd>> {
    let mut attempt = 0_u32;
    let (client, connection) = loop {
        attempt = attempt.saturating_add(1);
        match tokio_postgres::connect(&cfg.pg_dsn, NoTls).await {
            Ok(ok) => {
                info!("postgres connected on attempt {}", attempt);
                break ok;
            }
            Err(err) => {
                if attempt >= cfg.pg_connect_max_retries {
                    return Err(anyhow::anyhow!(
                        "postgres connect failed after {} attempts: {}",
                        attempt,
                        err
                    ));
                }
                warn!(
                    attempt,
                    max_attempts = cfg.pg_connect_max_retries,
                    retry_ms = cfg.pg_connect_retry_ms,
                    error = %err,
                    "postgres connect failed, retrying"
                );
                tokio::time::sleep(std::time::Duration::from_millis(cfg.pg_connect_retry_ms)).await;
            }
        }
    };

    tokio::spawn(async move {
        if let Err(err) = connection.await {
            warn!(error = %err, "postgres connection task ended");
        }
    });

    client
        .batch_execute(SCHEMA_SQL)
        .await
        .map_err(|err| anyhow::anyhow!("postgres schema init failed: {err}"))?;

    let (tx, mut rx) = mpsc::channel::<DbCmd>(DB_CMD_CHANNEL_CAPACITY);
    tokio::spawn(async move {
        let mut telemetry_batch = Vec::with_capacity(TELEMETRY_BATCH_SIZE);
        let mut flush_tick = tokio::time::interval(TELEMETRY_FLUSH_INTERVAL);
        flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else {
                        break;
                    };
                    match cmd {
                        DbCmd::Telemetry(t) => {
                            if telemetry_batch.len() >= TELEMETRY_BATCH_SIZE
                                && !flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await
                            {
                                stats.db_drop.fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                            telemetry_batch.push(t);
                            if telemetry_batch.len() >= TELEMETRY_BATCH_SIZE {
                                let _ = flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await;
                            }
                        }
                        DbCmd::Alarm(a) => {
                            let _ = flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await;
                            if let Err(err) = insert_alarm_event(&client, &a).await {
                                record_db_write_error(&stats, "alarm", err);
                            }
                        }
                        DbCmd::System { level, message } => {
                            let _ = flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await;
                            if let Err(err) = insert_system_event(&client, &level, &message).await {
                                record_db_write_error(&stats, "system", err);
                            }
                        }
                    }
                }
                _ = flush_tick.tick() => {
                    let _ = flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await;
                }
                else => break,
            }
        }

        let _ = flush_db_telemetry_batch(&client, &mut telemetry_batch, &stats).await;
    });

    Ok(tx)
}

async fn serve_ui_client(
    mut socket: TcpStream,
    mut rx: broadcast::Receiver<Vec<u8>>,
    stats: Arc<CollectorStats>,
) {
    stats.ui_clients.fetch_add(1, Ordering::Relaxed);
    loop {
        let line = match rx.recv().await {
            Ok(line) => line,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "ui feed lagged, skipping stale messages");
                stats.ui_drop.fetch_add(skipped as u64, Ordering::Relaxed);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        };

        let Ok(len) = u32::try_from(line.len()) else {
            stats.ui_drop.fetch_add(1, Ordering::Relaxed);
            continue;
        };
        if socket.write_all(&len.to_be_bytes()).await.is_err() {
            break;
        }
        if socket.write_all(&line).await.is_err() {
            break;
        }
    }
    stats.ui_clients.fetch_sub(1, Ordering::Relaxed);
}

async fn run_ui_forwarder(
    mut sub: tokio::sync::broadcast::Receiver<AppEvent>,
    ui_tx: broadcast::Sender<Vec<u8>>,
    stats: Arc<CollectorStats>,
) {
    let mut demo_emit_counters: HashMap<(String, usize), u64> = HashMap::new();
    loop {
        match sub.recv().await {
            Ok(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id,
                sensor_id,
                t_sec,
                value,
                req_id,
                alarm_bit,
                source_kind,
            })) => {
                let msg = telemetry_msg_from_sample(
                    device_id,
                    sensor_id,
                    t_sec,
                    value,
                    req_id,
                    alarm_bit,
                    source_kind,
                );
                if msg.source_kind == TelemetrySourceKind::SerialDemo {
                    let key = (msg.device_id.clone(), msg.sensor_id);
                    let counter = demo_emit_counters.entry(key).or_insert(0);
                    *counter = counter.saturating_add(1);
                    if *counter % DEMO_UI_STRIDE != 0 {
                        continue;
                    }
                }
                if let Ok(frame) = encode_feed_msg(&UiFeedMsg::Telemetry(msg)) {
                    if ui_tx.send(frame).is_err() {
                        stats.ui_drop.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(AppEvent::Device(DeviceEvent::AlarmRaised(alarm)))
            | Ok(AppEvent::Device(DeviceEvent::AlarmCleared(alarm))) => {
                if let Ok(frame) = encode_feed_msg(&UiFeedMsg::Alarm(alarm)) {
                    if ui_tx.send(frame).is_err() {
                        stats.ui_drop.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(AppEvent::System(msg)) => {
                if let Ok(frame) = encode_feed_msg(&UiFeedMsg::Status(msg)) {
                    if ui_tx.send(frame).is_err() {
                        stats.ui_drop.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "ui forwarder lagged, skipping stale messages");
                stats.ui_drop.fetch_add(skipped as u64, Ordering::Relaxed);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn run_alarm_forwarder(
    mut sub: tokio::sync::broadcast::Receiver<AppEvent>,
    alarm_service: AlarmService,
    stats: Arc<CollectorStats>,
) {
    let mut active_sessions: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        match sub.recv().await {
            Ok(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id,
                sensor_id,
                t_sec,
                value,
                source_kind,
                ..
            })) => {
                stats.samples_rx.fetch_add(1, Ordering::Relaxed);
                alarm_service.evaluate_sample(&device_id, sensor_id, t_sec, value, source_kind);
            }
            Ok(AppEvent::Device(DeviceEvent::ConnStateChanged { device_id, to, .. })) => {
                match to {
                    demo2::domain::ConnState::Ready => {
                        active_sessions.insert(device_id);
                    }
                    demo2::domain::ConnState::Disconnected => {
                        active_sessions.remove(&device_id);
                    }
                    _ => {}
                }
                stats
                    .ingress_connections
                    .store(active_sessions.len() as u64, Ordering::Relaxed);
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "alarm forwarder lagged, skipping stale messages");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn run_control_server(listener: TcpListener, alarm_service: AlarmService, bus: EventBus) {
    loop {
        match listener.accept().await {
            Ok((socket, _)) => {
                let service = alarm_service.clone();
                let bus = bus.clone();
                tokio::spawn(async move {
                    if let Err(err) = serve_control_client(socket, service, bus).await {
                        warn!(error = %err, "collector control request failed");
                    }
                });
            }
            Err(err) => warn!(error = %err, "collector control accept failed"),
        }
    }
}

async fn serve_control_client(
    socket: TcpStream,
    alarm_service: AlarmService,
    bus: EventBus,
) -> anyhow::Result<()> {
    let mut reader = BufReader::new(socket);
    match handle_control_client(&mut reader, alarm_service, bus).await {
        Ok(()) => reader.get_mut().write_all(b"{\"ok\":true}\n").await?,
        Err(err) => {
            let response = serde_json::json!({
                "ok": false,
                "error": err.to_string(),
            });
            reader
                .get_mut()
                .write_all(format!("{response}\n").as_bytes())
                .await?;
            return Err(err);
        }
    }
    Ok(())
}

async fn handle_control_client(
    reader: &mut BufReader<TcpStream>,
    alarm_service: AlarmService,
    bus: EventBus,
) -> anyhow::Result<()> {
    let mut line = String::new();
    reader.read_line(&mut line).await?;
    let cmd = serde_json::from_str::<ControlCmd>(line.trim())?;
    match cmd {
        ControlCmd::SetSentJumpThresholds {
            torque_warn,
            torque_red,
            torque_purple,
            angle_t1_red,
            angle_t2_red,
            angle_t_red,
            angle_s_red,
        } => {
            let angle_t1_red = angle_t1_red
                .or(angle_t_red)
                .ok_or_else(|| anyhow::anyhow!("missing angle_t1_red threshold"))?;
            let angle_t2_red = angle_t2_red
                .or(angle_t_red)
                .ok_or_else(|| anyhow::anyhow!("missing angle_t2_red threshold"))?;
            alarm_service
                .set_sent_jump_config(SentJumpAlarmConfig {
                    torque_warn,
                    torque_red,
                    torque_purple,
                    angle_t1_red,
                    angle_t2_red,
                    angle_s_red,
                })
                .map_err(anyhow::Error::msg)?;
            let config = alarm_service.sent_jump_config();
            bus.publish(AppEvent::System(format!(
                "SENT jump thresholds updated: torque warn={:.3}, red={:.3}, purple={:.3}; angle T1={:.3}, T2={:.3}, S={:.3}",
                config.torque_warn,
                config.torque_red,
                config.torque_purple,
                config.angle_t1_red,
                config.angle_t2_red,
                config.angle_s_red
            )));
        }
        ControlCmd::SetSentFrameGapThresholds { t1_us, t2_us, s_us } => {
            alarm_service
                .set_sent_frame_gap_config(SentFrameGapAlarmConfig { t1_us, t2_us, s_us })
                .map_err(anyhow::Error::msg)?;
            let config = alarm_service.sent_frame_gap_config();
            bus.publish(AppEvent::System(format!(
                "SENT frame gap thresholds updated: T1={}us, T2={}us, S={}us",
                config.t1_us, config.t2_us, config.s_us
            )));
        }
    }
    Ok(())
}

async fn run_persistence_forwarder(
    mut sub: tokio::sync::broadcast::Receiver<AppEvent>,
    db_tx: mpsc::Sender<DbCmd>,
    stats: Arc<CollectorStats>,
) {
    loop {
        match sub.recv().await {
            Ok(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id,
                sensor_id,
                t_sec,
                value,
                req_id,
                alarm_bit,
                source_kind,
            })) => {
                let msg = telemetry_msg_from_sample(
                    device_id,
                    sensor_id,
                    t_sec,
                    value,
                    req_id,
                    alarm_bit,
                    source_kind,
                );
                match telemetry_row_from_msg(msg) {
                    Ok(row) => {
                        let _ = db_tx.send(DbCmd::Telemetry(row)).await;
                    }
                    Err(err) => {
                        stats.db_drop.fetch_add(1, Ordering::Relaxed);
                        record_db_write_error(&stats, "telemetry_validation", err);
                    }
                }
            }
            Ok(AppEvent::Device(DeviceEvent::AlarmRaised(alarm))) => {
                let _ = db_tx.send(DbCmd::Alarm(alarm)).await;
            }
            Ok(AppEvent::Device(DeviceEvent::AlarmCleared(alarm))) => {
                let _ = db_tx.send(DbCmd::Alarm(alarm)).await;
            }
            Ok(AppEvent::System(msg)) => {
                let _ = db_tx
                    .send(DbCmd::System {
                        level: "info".to_string(),
                        message: msg,
                    })
                    .await;
            }
            Ok(AppEvent::Device(DeviceEvent::Log { level, msg, .. })) => {
                let _ = db_tx
                    .send(DbCmd::System {
                        level: level.to_string(),
                        message: msg,
                    })
                    .await;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "persistence lagged, skipping stale messages");
                stats.db_drop.fetch_add(skipped as u64, Ordering::Relaxed);
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn run_filtered_event_forwarder(
    mut sub: tokio::sync::broadcast::Receiver<AppEvent>,
    processed_bus: EventBus,
    filter_config: PersistenceFilterConfig,
) {
    let mut filter = PersistenceFilter::new(filter_config);
    loop {
        match sub.recv().await {
            Ok(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id,
                sensor_id,
                t_sec,
                value,
                req_id,
                alarm_bit,
                source_kind,
            })) => {
                let filtered = filter.apply(telemetry_msg_from_sample(
                    device_id,
                    sensor_id,
                    t_sec,
                    value,
                    req_id,
                    alarm_bit,
                    source_kind,
                ));
                processed_bus.publish(AppEvent::Device(DeviceEvent::TelemetrySample {
                    device_id: filtered.device_id,
                    sensor_id: filtered.sensor_id,
                    t_sec: filtered.t_sec,
                    value: filtered.value,
                    req_id: filtered.request_id,
                    alarm_bit: filtered.alarm_bit,
                    source_kind: filtered.source_kind,
                }));
            }
            Ok(other) => processed_bus.publish(other),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                warn!(skipped, "filter forwarder lagged, skipping stale messages");
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}

fn health_json(stats: &CollectorStats) -> String {
    let last_db_error = stats
        .last_db_error
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    serde_json::json!({
        "ingress_ready": stats.ingress_ready.load(Ordering::Relaxed),
        "ui_ready": stats.ui_ready.load(Ordering::Relaxed),
        "ingress_connections": stats.ingress_connections.load(Ordering::Relaxed),
        "ui_clients": stats.ui_clients.load(Ordering::Relaxed),
        "samples_rx": stats.samples_rx.load(Ordering::Relaxed),
        "ui_drop": stats.ui_drop.load(Ordering::Relaxed),
        "db_drop": stats.db_drop.load(Ordering::Relaxed),
        "db_write_fail": stats.db_write_fail.load(Ordering::Relaxed),
        "last_db_error": last_db_error,
    })
    .to_string()
}

async fn run_health_server(listener: TcpListener, stats: Arc<CollectorStats>) -> io::Result<()> {
    loop {
        let (mut socket, _) = listener.accept().await?;
        let stats = stats.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let n = match socket.read(&mut buf).await {
                Ok(n) => n,
                Err(_) => return,
            };
            let req = String::from_utf8_lossy(&buf[..n]);
            let first = req.lines().next().unwrap_or("");

            let (code, body) = if first.starts_with("GET /ready") {
                let ready = stats.ingress_ready.load(Ordering::Relaxed)
                    && stats.ui_ready.load(Ordering::Relaxed);
                if ready {
                    ("200 OK", "{\"ready\":true}".to_string())
                } else {
                    ("503 Service Unavailable", "{\"ready\":false}".to_string())
                }
            } else {
                ("200 OK", health_json(&stats))
            };

            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                code,
                body.len(),
                body
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });
    }
}

pub async fn run() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let cfg = load_collector_config();
    info!(?cfg, "collector config loaded");

    let stats = Arc::new(CollectorStats::default());
    let raw_bus = EventBus::new(cfg.bus_capacity);
    let processed_bus = EventBus::new(cfg.bus_capacity);
    let store = Store::default();
    let sessions: Arc<tokio::sync::RwLock<HashMap<String, DeviceSessionHandle>>> =
        Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let filter_config = PersistenceFilterConfig {
        enabled: cfg.db_filter_enabled,
        filter: ButterworthConfig {
            order: cfg.db_filter_order,
            sample_rate_hz: cfg.db_filter_sample_rate_hz,
            cutoff_hz: cfg.db_filter_cutoff_hz,
        },
    };
    tokio::spawn(run_filtered_event_forwarder(
        raw_bus.subscribe(),
        processed_bus.clone(),
        filter_config,
    ));

    let disable_db = std::env::var("DEMO2_DISABLE_DB")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let (ui_tx, _) = broadcast::channel::<Vec<u8>>(cfg.ui_feed_capacity);
    let alarm_service = AlarmService::new(processed_bus.clone());
    let control_listener = TcpListener::bind(&cfg.control_addr).await?;
    let health_listener = TcpListener::bind(&cfg.health_addr).await?;
    tokio::spawn(run_ui_forwarder(
        processed_bus.subscribe(),
        ui_tx.clone(),
        stats.clone(),
    ));
    tokio::spawn(run_alarm_forwarder(
        processed_bus.subscribe(),
        alarm_service.clone(),
        stats.clone(),
    ));
    tokio::spawn(run_control_server(
        control_listener,
        alarm_service,
        processed_bus.clone(),
    ));

    if disable_db {
        warn!("DEMO2_DISABLE_DB is enabled, persistence is disabled");
    } else {
        match start_pg_writer(&cfg, stats.clone()).await {
            Ok(db_tx) => {
                tokio::spawn(run_persistence_forwarder(
                    processed_bus.subscribe(),
                    db_tx,
                    stats.clone(),
                ));
                info!("postgres persistence enabled");
            }
            Err(err) => {
                warn!(error = %err, "postgres unavailable, running without persistence");
            }
        }
    }

    tokio::spawn(run_health_server(health_listener, stats.clone()));

    let can_enabled = env_flag("DEMO2_COLLECTOR_CAN_ENABLED").unwrap_or(cfg.can_enabled);
    if can_enabled {
        let env_hardware_name = std::env::var("DEMO2_COLLECTOR_CAN_HW_NAME").ok();
        let hardware_name = env_hardware_name
            .clone()
            .unwrap_or_else(|| cfg.can_hardware_name.clone());
        let hardware_subtype = std::env::var("DEMO2_COLLECTOR_CAN_HW_SUBTYPE")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .or_else(|| {
                if env_hardware_name.is_some() {
                    None
                } else {
                    cfg.can_hardware_subtype
                }
            })
            .unwrap_or_else(
                || match hardware_name.trim().to_ascii_uppercase().as_str() {
                    "TC1012" => HW_SUBTYPE_TC1012,
                    "TC1016" => HW_SUBTYPE_TC1016,
                    _ => CanTransportConfig::default().hardware_subtype,
                },
            );
        let can_config = CanTransportConfig {
            tsmaster_bin: std::env::var("DEMO2_COLLECTOR_CAN_TSMASTER_BIN")
                .ok()
                .or_else(|| cfg.can_tsmaster_bin.clone())
                .map(Into::into),
            autostart_tsmaster: env_flag("DEMO2_COLLECTOR_CAN_AUTOSTART_TSMASTER")
                .unwrap_or(cfg.can_autostart_tsmaster),
            hardware_name,
            hardware_subtype,
            channels: collector_can_channels(&cfg),
            ..CanTransportConfig::default()
        };
        let sent_filter_config = SentFilterConfig {
            enabled: env_flag("DEMO2_SENT_FILTER_ENABLED").unwrap_or(cfg.sent_filter_enabled),
            window_size: std::env::var("DEMO2_SENT_FILTER_WINDOW")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(cfg.sent_filter_window)
                .max(1),
        };
        let watchdog_config = CanSignalWatchdogConfig {
            enabled: env_flag("DEMO2_CAN_SIGNAL_WATCHDOG_ENABLED")
                .unwrap_or(cfg.can_signal_watchdog_enabled),
            timeout: Duration::from_millis(
                std::env::var("DEMO2_CAN_SIGNAL_TIMEOUT_MS")
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(cfg.can_signal_timeout_ms)
                    .max(1),
            ),
            targets: cfg
                .can_signal_watchdogs
                .iter()
                .map(|target| CanSignalWatchdogTarget {
                    channel: target.channel,
                    identifier: target.can_id,
                    label: target.label.clone().unwrap_or_else(|| {
                        target
                            .can_id
                            .map(|identifier| format!("CAN 0x{identifier:X}"))
                            .unwrap_or_else(|| format!("Channel {} signal", target.channel))
                    }),
                })
                .collect(),
        };
        let bus_for_can = raw_bus.clone();
        tokio::spawn(async move {
            run_can_ingress(can_config, sent_filter_config, watchdog_config, bus_for_can).await;
        });
    }

    let configured_serial_port = std::env::var("DEMO2_COLLECTOR_SERIAL_PORT")
        .ok()
        .or_else(|| cfg.serial_port.clone());
    let serial_auto_detect =
        env_flag("DEMO2_COLLECTOR_SERIAL_AUTO_DETECT").unwrap_or(cfg.serial_auto_detect);
    let serial_baud = std::env::var("DEMO2_COLLECTOR_SERIAL_BAUD")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(cfg.serial_baud);
    let serial_mode_text =
        std::env::var("DEMO2_COLLECTOR_SERIAL_MODE").unwrap_or_else(|_| cfg.serial_mode.clone());
    let serial_mode = parse_serial_mode(&serial_mode_text).unwrap_or(SerialIngressMode::Sent);

    if let Some(port_name) = configured_serial_port {
        match serial_mode {
            SerialIngressMode::Legacy => {
                let device_id = format!("serial://{}", port_name);
                let handle = DeviceSession::spawn(
                    device_id.clone(),
                    Box::new(SerialTransport::new(port_name, serial_baud)),
                    Arc::new(SimpleFrameCodec::new(cfg.max_payload)),
                    raw_bus.clone(),
                    store.clone(),
                    SessionConfig {
                        enable_heartbeat: false,
                        reconnect_enabled: false,
                        ..SessionConfig::default()
                    },
                );
                sessions.write().await.insert(device_id, handle);
                publish_status(&raw_bus, "collector serial legacy session started");
            }
            _ => {
                let bus_for_serial = raw_bus.clone();
                tokio::spawn(async move {
                    run_serial_ingress(port_name, serial_baud, serial_mode, bus_for_serial).await;
                });
            }
        }
    } else if serial_auto_detect {
        let bus_for_serial = raw_bus.clone();
        tokio::spawn(async move {
            run_auto_serial_ingress(serial_baud, serial_mode, bus_for_serial).await;
        });
    }

    let ui_listener = TcpListener::bind(&cfg.ui_feed_addr).await?;
    stats.ui_ready.store(true, Ordering::Relaxed);
    let stats_for_ui = stats.clone();
    tokio::spawn(async move {
        loop {
            if let Ok((socket, _)) = ui_listener.accept().await {
                let rx = ui_tx.subscribe();
                tokio::spawn(serve_ui_client(socket, rx, stats_for_ui.clone()));
            }
        }
    });

    stats.ingress_ready.store(true, Ordering::Relaxed);

    println!("collector ingress listening on {}", cfg.ingress_addr);
    println!("collector ui feed listening on {}", cfg.ui_feed_addr);
    println!("collector health listening on {}", cfg.health_addr);
    println!("collector control listening on {}", cfg.control_addr);

    run_tcp_ingress(
        &cfg.ingress_addr,
        cfg.max_payload,
        raw_bus.clone(),
        store.clone(),
        sessions.clone(),
    )
    .await?;

    Ok(())
}

#[allow(dead_code)]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    run().await
}
