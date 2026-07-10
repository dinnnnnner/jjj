use crate::bus::TelemetrySourceKind;
use crate::bus::{AppEvent, DeviceEvent, EventBus, Store};
use crate::domain::{Command, ConnState, DeviceId, DeviceSnapshot, Response};
use crate::protocol::{Frame, FrameCodec};
use crate::transport::{Transport, TransportError};
use bytes::BytesMut;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("session closed")]
    Closed,
    #[error("command timeout")]
    Timeout,
    #[error("transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("codec error: {0}")]
    Codec(String),
    #[error("command channel full")]
    Busy,
}

#[derive(Clone)]
pub struct SessionConfig {
    pub heartbeat_interval: Duration,
    pub heartbeat_timeout: Duration,
    pub reconnect_base: Duration,
    pub reconnect_max: Duration,
    pub enable_heartbeat: bool,
    pub reconnect_enabled: bool,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(2),
            heartbeat_timeout: Duration::from_secs(8),
            reconnect_base: Duration::from_millis(200),
            reconnect_max: Duration::from_secs(5),
            enable_heartbeat: true,
            reconnect_enabled: true,
        }
    }
}

enum SessionMsg {
    Call(Command, oneshot::Sender<Result<Response, SessionError>>),
    Stop,
}

struct PendingResponder {
    responder: oneshot::Sender<Result<Response, SessionError>>,
}

struct PendingCommand {
    cmd: Command,
    deadline: Instant,
    attempts: u32,
}

enum SessionCoreEvent {
    Transmit(Frame),
    CommandSucceeded(Response),
    CommandTimedOut(u64),
    TelemetryReceived {
        request_id: u64,
        sensor_id: usize,
        value: f64,
        ack: Frame,
    },
    InvalidTelemetry,
    UnsolicitedFrame(u8),
    HeartbeatTimedOut,
}

struct SessionCore {
    config: SessionConfig,
    pending: HashMap<u64, PendingCommand>,
    last_rx: Instant,
}

impl SessionCore {
    fn new(config: SessionConfig, now: Instant) -> Self {
        Self {
            config,
            pending: HashMap::new(),
            last_rx: now,
        }
    }

    fn handle_command(&mut self, cmd: Command, now: Instant) -> Vec<SessionCoreEvent> {
        let frame = command_frame(&cmd);
        self.pending.insert(
            cmd.request_id,
            PendingCommand {
                deadline: now + cmd.timeout,
                attempts: 1,
                cmd,
            },
        );
        vec![SessionCoreEvent::Transmit(frame)]
    }

    fn handle_heartbeat_tick(&mut self, now: Instant) -> Vec<SessionCoreEvent> {
        if !self.config.enable_heartbeat {
            return Vec::new();
        }

        let mut events = vec![SessionCoreEvent::Transmit(Frame {
            request_id: 0,
            kind: 0xF0,
            payload: Vec::new(),
        })];

        if now.duration_since(self.last_rx) > self.config.heartbeat_timeout {
            events.push(SessionCoreEvent::HeartbeatTimedOut);
        }

        events
    }

    fn handle_timeout_tick(&mut self, now: Instant) -> Vec<SessionCoreEvent> {
        let timed_out: Vec<_> = self
            .pending
            .iter()
            .filter_map(|(id, pending)| (now >= pending.deadline).then_some(*id))
            .collect();
        let mut events = Vec::new();

        for id in timed_out {
            let Some(mut pending) = self.pending.remove(&id) else {
                continue;
            };

            if pending.cmd.idempotent && pending.attempts < pending.cmd.retry.max_attempts {
                pending.attempts += 1;
                pending.deadline = now + pending.cmd.timeout;
                let frame = command_frame(&pending.cmd);
                self.pending.insert(id, pending);
                events.push(SessionCoreEvent::Transmit(frame));
            } else {
                events.push(SessionCoreEvent::CommandTimedOut(id));
            }
        }

        events
    }

    fn handle_frame(
        &mut self,
        frame: Frame,
        now: Instant,
        system_now: SystemTime,
    ) -> Vec<SessionCoreEvent> {
        self.last_rx = now;

        if frame.kind == 0xF1 {
            return Vec::new();
        }

        if self.pending.remove(&frame.request_id).is_some() {
            return vec![SessionCoreEvent::CommandSucceeded(Response {
                request_id: frame.request_id,
                code: frame.kind,
                payload: frame.payload,
                ts: system_now,
            })];
        }

        if frame.kind == 0x34 {
            if let Some((sensor_id, value)) = parse_sensor_value(&frame.payload) {
                return vec![SessionCoreEvent::TelemetryReceived {
                    request_id: frame.request_id,
                    sensor_id,
                    value,
                    ack: Frame {
                        request_id: frame.request_id,
                        kind: 0x90,
                        payload: b"demo2-ack".to_vec(),
                    },
                }];
            }

            return vec![SessionCoreEvent::InvalidTelemetry];
        }

        vec![SessionCoreEvent::UnsolicitedFrame(frame.kind)]
    }

    fn close(&mut self) -> Vec<u64> {
        self.pending.drain().map(|(id, _)| id).collect()
    }
}

fn command_frame(cmd: &Command) -> Frame {
    Frame {
        request_id: cmd.request_id,
        kind: cmd.kind.code(),
        payload: cmd.payload.clone(),
    }
}

#[derive(Clone)]
pub struct DeviceSessionHandle {
    device_id: DeviceId,
    tx: mpsc::Sender<SessionMsg>,
}

impl DeviceSessionHandle {
    pub async fn call(&self, cmd: Command) -> Result<Response, SessionError> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(SessionMsg::Call(cmd, tx))
            .await
            .map_err(|_| SessionError::Closed)?;
        rx.await.map_err(|_| SessionError::Closed)?
    }

    pub async fn stop(&self) {
        let _ = self.tx.send(SessionMsg::Stop).await;
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

pub struct DeviceSession {
    device_id: DeviceId,
    state: ConnState,
    transport: Box<dyn Transport>,
    codec: Arc<dyn FrameCodec>,
    bus: EventBus,
    store: Store,
    config: SessionConfig,
    rx: mpsc::Receiver<SessionMsg>,
}

impl DeviceSession {
    pub fn spawn(
        device_id: DeviceId,
        transport: Box<dyn Transport>,
        codec: Arc<dyn FrameCodec>,
        bus: EventBus,
        store: Store,
        config: SessionConfig,
    ) -> DeviceSessionHandle {
        Self::spawn_with_join(device_id, transport, codec, bus, store, config).0
    }

    pub fn spawn_with_join(
        device_id: DeviceId,
        transport: Box<dyn Transport>,
        codec: Arc<dyn FrameCodec>,
        bus: EventBus,
        store: Store,
        config: SessionConfig,
    ) -> (DeviceSessionHandle, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(256);
        let mut session = Self {
            device_id: device_id.clone(),
            state: ConnState::Disconnected,
            transport,
            codec,
            bus,
            store,
            config,
            rx,
        };

        let join = tokio::spawn(async move {
            session.run().await;
        });

        (DeviceSessionHandle { device_id, tx }, join)
    }

    async fn run(&mut self) {
        let mut backoff = self.config.reconnect_base;

        loop {
            self.set_state(ConnState::Connecting).await;
            match self.transport.connect().await {
                Ok(()) => {
                    self.set_state(ConnState::Handshaking).await;
                    self.set_state(ConnState::Ready).await;
                    info!(device_id = %self.device_id, "session connected");

                    if self.connection_loop().await.is_ok() {
                        self.set_state(ConnState::Disconnected).await;
                        let _ = self.transport.close().await;
                        break;
                    }
                }
                Err(err) => {
                    warn!(device_id = %self.device_id, error = %err, "connect failed");
                }
            }

            if !self.config.reconnect_enabled {
                self.set_state(ConnState::Disconnected).await;
                let _ = self.transport.close().await;
                break;
            }
            self.set_state(ConnState::Reconnecting).await;
            let _ = self.transport.close().await;
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(self.config.reconnect_max);
        }
    }

    async fn connection_loop(&mut self) -> Result<(), SessionError> {
        let connection_start = Instant::now();
        let mut in_buf = BytesMut::with_capacity(8192);
        let mut core = SessionCore::new(self.config.clone(), Instant::now());
        let mut responders: HashMap<u64, PendingResponder> = HashMap::new();
        let mut heartbeat_tick = tokio::time::interval(self.config.heartbeat_interval);
        let mut timeout_tick = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                msg = self.rx.recv() => {
                    match msg {
                        Some(SessionMsg::Call(cmd, responder)) => {
                            let request_id = cmd.request_id;
                            responders.insert(request_id, PendingResponder {
                                responder,
                            });
                            let events = core.handle_command(cmd, Instant::now());
                            if let Err(err) = self.handle_core_events(events, &mut responders, connection_start).await {
                                if let Some(pending) = responders.remove(&request_id) {
                                    let _ = pending.responder.send(Err(err));
                                }
                                return Err(SessionError::Closed);
                            }
                        }
                        Some(SessionMsg::Stop) | None => {
                            for id in core.close() {
                                if let Some(pending) = responders.remove(&id) {
                                    let _ = pending.responder.send(Err(SessionError::Closed));
                                }
                            }
                            for (_, p) in responders.drain() {
                                let _ = p.responder.send(Err(SessionError::Closed));
                            }
                            return Ok(());
                        }
                    }
                }
                _ = heartbeat_tick.tick() => {
                    let events = core.handle_heartbeat_tick(Instant::now());
                    self.handle_core_events(events, &mut responders, connection_start).await?;
                }
                _ = timeout_tick.tick() => {
                    let events = core.handle_timeout_tick(Instant::now());
                    self.handle_core_events(events, &mut responders, connection_start).await?;
                }
                read_res = self.transport.read(&mut in_buf) => {
                    let n = read_res?;
                    if n == 0 {
                        return Err(SessionError::Closed);
                    }
                    if self.state == ConnState::Degraded {
                        self.set_state(ConnState::Ready).await;
                    }

                    loop {
                        let maybe = self
                            .codec
                            .try_decode(&mut in_buf)
                            .map_err(|e| SessionError::Codec(e.to_string()))?;
                        let Some(frame) = maybe else { break; };

                        let events = core.handle_frame(frame, Instant::now(), SystemTime::now());
                        self.handle_core_events(events, &mut responders, connection_start).await?;
                    }
                }
            }
        }
    }

    async fn handle_core_events(
        &mut self,
        events: Vec<SessionCoreEvent>,
        responders: &mut HashMap<u64, PendingResponder>,
        connection_start: Instant,
    ) -> Result<(), SessionError> {
        for event in events {
            match event {
                SessionCoreEvent::Transmit(frame) => {
                    let mut out = BytesMut::with_capacity(64 + frame.payload.len());
                    self.codec
                        .encode(&frame, &mut out)
                        .map_err(|e| SessionError::Codec(e.to_string()))?;
                    self.transport.write_all(&out).await?;
                }
                SessionCoreEvent::CommandSucceeded(response) => {
                    let request_id = response.request_id;
                    if let Some(pending) = responders.remove(&request_id) {
                        let _ = pending.responder.send(Ok(response));
                    }
                    self.bus
                        .publish(AppEvent::Device(DeviceEvent::CommandResult {
                            device_id: self.device_id.clone(),
                            request_id,
                            ok: true,
                        }));
                }
                SessionCoreEvent::CommandTimedOut(request_id) => {
                    if let Some(pending) = responders.remove(&request_id) {
                        let _ = pending.responder.send(Err(SessionError::Timeout));
                    }
                    self.bus
                        .publish(AppEvent::Device(DeviceEvent::CommandResult {
                            device_id: self.device_id.clone(),
                            request_id,
                            ok: false,
                        }));
                }
                SessionCoreEvent::TelemetryReceived {
                    request_id,
                    sensor_id,
                    value,
                    ack,
                } => {
                    self.publish_telemetry(request_id, sensor_id, value, connection_start)
                        .await;
                    let mut out = BytesMut::new();
                    self.codec
                        .encode(&ack, &mut out)
                        .map_err(|e| SessionError::Codec(e.to_string()))?;
                    self.transport.write_all(&out).await?;
                }
                SessionCoreEvent::InvalidTelemetry => {
                    self.bus.publish(AppEvent::Device(DeviceEvent::Log {
                        device_id: self.device_id.clone(),
                        level: "warn",
                        msg: "invalid telemetry payload for kind=0x34".to_string(),
                    }));
                }
                SessionCoreEvent::UnsolicitedFrame(kind) => {
                    self.bus.publish(AppEvent::Device(DeviceEvent::Log {
                        device_id: self.device_id.clone(),
                        level: "warn",
                        msg: format!("unsolicited frame kind={kind}"),
                    }));
                }
                SessionCoreEvent::HeartbeatTimedOut => {
                    self.set_state(ConnState::Degraded).await;
                    return Err(SessionError::Timeout);
                }
            }
        }

        Ok(())
    }

    async fn publish_telemetry(
        &mut self,
        request_id: u64,
        sensor_id: usize,
        value: f64,
        connection_start: Instant,
    ) {
        let mut snapshot = self
            .store
            .snapshot(&self.device_id)
            .await
            .unwrap_or_else(|| DeviceSnapshot {
                device_id: self.device_id.clone(),
                ..DeviceSnapshot::default()
            });
        snapshot.last_seen = Some(SystemTime::now());
        snapshot
            .telemetry
            .insert(format!("sensor_{sensor_id}"), json!(value));
        self.store.upsert_snapshot(snapshot).await;

        self.bus
            .publish(AppEvent::Device(DeviceEvent::TelemetrySample {
                device_id: self.device_id.clone(),
                sensor_id,
                t_sec: connection_start.elapsed().as_secs_f64(),
                value,
                req_id: request_id,
                alarm_bit: false,
                source_kind: if self.device_id.starts_with("tcp://") {
                    TelemetrySourceKind::TcpFrame
                } else {
                    TelemetrySourceKind::FrameStream
                },
            }));
    }

    async fn set_state(&mut self, next: ConnState) {
        if self.state == next {
            return;
        }

        let prev = self.state;
        self.state = next;

        self.bus
            .publish(AppEvent::Device(DeviceEvent::ConnStateChanged {
                device_id: self.device_id.clone(),
                from: prev,
                to: next,
            }));

        let mut snapshot = self
            .store
            .snapshot(&self.device_id)
            .await
            .unwrap_or_else(|| DeviceSnapshot {
                device_id: self.device_id.clone(),
                ..DeviceSnapshot::default()
            });
        snapshot.conn_state = next;
        snapshot.last_seen = Some(SystemTime::now());
        self.store.upsert_snapshot(snapshot.clone()).await;

        self.bus
            .publish(AppEvent::Device(DeviceEvent::TelemetryUpdated {
                device_id: self.device_id.clone(),
                snapshot,
            }));

        info!(device_id = %self.device_id, state = ?next, "session state changed");
    }
}

fn parse_sensor_value(payload: &[u8]) -> Option<(usize, f64)> {
    let s = std::str::from_utf8(payload).ok()?.trim();
    let mut sid = None;
    let mut value = None;
    for part in s.split(',') {
        let mut kv = part.splitn(2, '=');
        let k = kv.next()?.trim();
        let v = kv.next()?.trim();
        if k.eq_ignore_ascii_case("sid") {
            sid = v.parse::<usize>().ok();
        } else if k.eq_ignore_ascii_case("value") {
            value = v.parse::<f64>().ok();
        }
    }
    Some((sid?, value?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{CommandKind, RetryPolicy};

    fn command(request_id: u64, timeout: Duration, max_attempts: u32, idempotent: bool) -> Command {
        Command {
            request_id,
            kind: CommandKind::ReadParam,
            payload: b"read".to_vec(),
            timeout,
            retry: RetryPolicy {
                max_attempts,
                base_delay: Duration::from_millis(10),
            },
            idempotent,
        }
    }

    #[test]
    fn command_produces_transmit_and_response_completes_pending() {
        let now = Instant::now();
        let mut core = SessionCore::new(SessionConfig::default(), now);

        let events = core.handle_command(command(7, Duration::from_secs(1), 1, true), now);
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::Transmit(frame) => {
                assert_eq!(frame.request_id, 7);
                assert_eq!(frame.kind, 0x01);
                assert_eq!(frame.payload, b"read");
            }
            _ => panic!("expected transmit event"),
        }

        let events = core.handle_frame(
            Frame {
                request_id: 7,
                kind: 0x80,
                payload: b"ok".to_vec(),
            },
            now,
            SystemTime::UNIX_EPOCH,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::CommandSucceeded(response) => {
                assert_eq!(response.request_id, 7);
                assert_eq!(response.code, 0x80);
                assert_eq!(response.payload, b"ok");
            }
            _ => panic!("expected command success event"),
        }
    }

    #[test]
    fn idempotent_command_retries_then_times_out() {
        let now = Instant::now();
        let mut core = SessionCore::new(SessionConfig::default(), now);

        let _ = core.handle_command(command(8, Duration::from_millis(100), 2, true), now);

        let events = core.handle_timeout_tick(now + Duration::from_millis(100));
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::Transmit(frame) => assert_eq!(frame.request_id, 8),
            _ => panic!("expected retry transmit event"),
        }

        let events = core.handle_timeout_tick(now + Duration::from_millis(200));
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::CommandTimedOut(request_id) => assert_eq!(*request_id, 8),
            _ => panic!("expected timeout event"),
        }
    }

    #[test]
    fn non_idempotent_command_does_not_retry() {
        let now = Instant::now();
        let mut core = SessionCore::new(SessionConfig::default(), now);

        let _ = core.handle_command(command(9, Duration::from_millis(100), 3, false), now);

        let events = core.handle_timeout_tick(now + Duration::from_millis(100));
        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::CommandTimedOut(request_id) => assert_eq!(*request_id, 9),
            _ => panic!("expected timeout event"),
        }
    }

    #[test]
    fn telemetry_frame_produces_ack_event() {
        let now = Instant::now();
        let mut core = SessionCore::new(SessionConfig::default(), now);

        let events = core.handle_frame(
            Frame {
                request_id: 11,
                kind: 0x34,
                payload: b"sid=2,value=42.5".to_vec(),
            },
            now,
            SystemTime::UNIX_EPOCH,
        );

        assert_eq!(events.len(), 1);
        match &events[0] {
            SessionCoreEvent::TelemetryReceived {
                request_id,
                sensor_id,
                value,
                ack,
            } => {
                assert_eq!(*request_id, 11);
                assert_eq!(*sensor_id, 2);
                assert_eq!(*value, 42.5);
                assert_eq!(ack.request_id, 11);
                assert_eq!(ack.kind, 0x90);
                assert_eq!(ack.payload, b"demo2-ack");
            }
            _ => panic!("expected telemetry event"),
        }
    }
}
