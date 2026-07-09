# demo2 架构文档

本文档面向当前仓库代码，说明 collector、UI、接入协议、事件总线和持久化之间的关系。接口字段和示例见 `docs/api.md`。

## 1. 项目定位

`demo2` 是一个 Rust 上位机 MVP，负责设备数据接入、协议解码、事件分发、告警计算、PostgreSQL 持久化和 egui 桌面展示。

当前主要运行形态：

- `collector_service`：采集服务，负责 TCP / 串口 / CAN 接入、UI feed、control、health 和数据库写入。
- `ui_client`：桌面 UI，连接 UI feed，并可通过 control 接口更新 SENT 跳变告警阈值。

注意：当前 `ui_client` 默认只连接外部 collector。设置 `DEMO2_UI_EMBED_COLLECTOR=1` 后会额外启动一份内嵌 collector，适合单进程演示。

## 2. 进程与端口

| 组件 | 默认地址 | 说明 |
| --- | --- | --- |
| TCP ingress | `127.0.0.1:19010` | 设备或 sender 上传二进制帧。 |
| UI feed | `127.0.0.1:19011` | collector 向 UI 推送 JSON Lines。 |
| Health | `127.0.0.1:19012` | HTTP `/health` 和 `/ready`。 |
| Control | `127.0.0.1:19013` | TCP JSON Lines 控制命令。 |

## 3. 代码分层

```text
src/
  transport/  TCP、串口、CAN 字节流抽象
  protocol/   SimpleFrame、demo serial、SENT 编解码
  session/    设备会话、pending 请求、ACK、心跳和连接状态
  ingress/    TCP、串口、CAN 接入，把外部数据转换为 AppEvent
  bus/        EventBus 与最新设备快照 Store
  app/        AlarmService 和业务服务
  signal/     信号规格、派生信号和滤波
  db/         PostgreSQL schema 与写入器
  ui_client/  UI feed、状态、事件处理、回放、记录和视图
  bin/        可执行入口
```

### 3.1 transport

`Transport` trait 只处理字节流 I/O：

```rust
async fn connect(&mut self) -> Result<(), TransportError>;
async fn read(&mut self, dst: &mut BytesMut) -> Result<usize, TransportError>;
async fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError>;
async fn close(&mut self) -> Result<(), TransportError>;
```

实现包括 `TcpTransport`、`ConnectedTcpTransport`、`SerialTransport`、`ConnectedSerialTransport` 和 TSMaster CAN 封装。业务字段、告警和 UI 展示不放在这一层。

### 3.2 protocol

协议层提供：

- `SimpleFrameCodec`：TCP / legacy 串口二进制帧，支持粘包拆包、CRC、错位恢复和长度上限。
- `SerialDemoCodec`：demo 串口数据流。
- `SentFrameCodec`：10 字节 SENT 帧，校验 nibble CRC。

### 3.3 session

`DeviceSession` 负责连接生命周期、请求响应匹配、心跳、重连、遥测解析和 ACK 回包。TCP ingress 接受被动连接后会为每个连接创建一个 session，并关闭 session 内部心跳和重连：

```text
enable_heartbeat = false
reconnect_enabled = false
```

断开后等待设备端重新连接。

### 3.4 ingress

接入层把外部输入统一转换为 `AppEvent`：

- TCP：监听 `ingress_addr`，每个连接进入 `DeviceSession`。
- 串口：支持 `legacy`、`demo`、`sent`、`sent1`、`sent2`、`sent3`。
- CAN：通过 TSMaster / TC1012 读取 CAN / CAN FD，解析普通三轴样本、SENT over CAN 和 SENT error。

### 3.5 bus

`EventBus` 基于 `tokio::broadcast`，是有界发布订阅通道。慢消费者会收到 `Lagged`，旧消息可能被跳过。

核心事件：

- `TelemetrySample`
- `AlarmRaised`
- `AlarmCleared`
- `ConnStateChanged`
- `CommandResult`
- `Log`
- `System`

`Store` 只保存每个设备的最新快照，不保存全量历史。

### 3.6 app

`AlarmService` 负责两类告警：

- 通用 sensor 阈值规则：通过 `set_rule(sensor_id, AlarmRule)` 注册，按 high/low 与 clear 阈值触发和恢复。
- SENT 跳变告警：默认阈值在 `SentJumpAlarmConfig` 中，可通过 control 接口更新。

告警输出为 `AlarmRaised` / `AlarmCleared`，下游 UI feed 和数据库写入器都消费同一类事件。

### 3.7 db

PostgreSQL schema 定义在 `src/db/mod.rs`：

- `telemetry_samples`：按 `created_at` 日期范围分区。
- `alarm_events`
- `system_events`

写入器批量写入遥测，批大小为 256。写入前会确保当天分区存在。

### 3.8 ui_client

`src/ui_client/` 承载 UI 逻辑：

- `feed.rs`：连接 UI feed、重连、JSON 行解码。
- `messages.rs`：UI feed 消息结构。
- `events.rs`：处理 UI 队列消息。
- `state.rs` / `view.rs`：主状态和 egui 绘制。
- `alarm.rs`：UI 侧告警展示、CAN 阈值和 SENT 跳变相关交互。
- `records.rs` / `replay.rs`：历史告警和 CAN/SENT 回放。
- `runtime.rs`：启动 feed 线程和 eframe 窗口，并按需启动内嵌 collector。

## 4. 数据流

### 4.1 TCP sender 到 UI

```text
sender_stress_report / sender_1min
  -> TCP ingress 19010
  -> run_tcp_ingress
  -> DeviceSession
  -> SimpleFrameCodec
  -> AppEvent::Device(TelemetrySample)
  -> raw_bus
  -> run_filtered_event_forwarder
  -> processed_bus
  -> run_ui_forwarder
  -> UI feed JSON line on 19011
  -> ui_client feed thread
  -> egui 曲线和状态
```

### 4.2 CAN SENT 到 UI/DB

```text
TSMaster callback
  -> CanTransport
  -> run_can_ingress
  -> decode_sent_values
  -> optional SentMovingAverage
  -> TelemetrySample(source_kind=CanSent)
  -> raw_bus
  -> optional Butterworth filter
  -> processed_bus
  -> UI feed / AlarmService / PostgreSQL
```

### 4.3 告警到 UI/DB

```text
processed_bus TelemetrySample
  -> run_alarm_forwarder
  -> AlarmService::evaluate_sample
  -> AlarmRaised / AlarmCleared
  -> processed_bus
  -> UI feed / PostgreSQL
```

### 4.4 持久化

```text
processed_bus
  -> run_persistence_forwarder
  -> DbCmd channel
  -> PostgreSQL writer
  -> telemetry_samples / alarm_events / system_events
```

## 5. 滤波位置

当前有两层可选处理：

- CAN SENT 入口移动平均：`sent_filter_enabled` / `sent_filter_window`，发生在 `run_can_ingress` 发布事件之前。
- Butterworth 低通滤波：`db_filter_enabled` 等配置，发生在 raw bus 到 processed bus 的转发阶段。

启用 Butterworth 后，UI feed、告警和数据库看到的是 processed bus 中的滤波值。

## 6. 背压与丢弃

- `EventBus` 是有界 broadcast channel，慢订阅者可能跳过旧消息。
- UI feed 也是有界 broadcast channel，发送失败或 lag 会增加 `ui_drop`。
- DB forwarder lag 会增加 `db_drop`，数据库写入失败会增加 `db_write_fail` 并记录 `last_db_error`。

这些指标可通过 `/health` 查看。

## 7. 推荐开发验证

```powershell
cargo check
cargo test
cargo run --bin collector_service
Invoke-RestMethod http://127.0.0.1:19012/health
cargo run --bin sender_stress_report
```

涉及数据库 schema 或写入路径时，再运行：

```powershell
cargo run --bin init_db
cargo run --bin check_db_status
cargo run --bin check_db_partition
```
