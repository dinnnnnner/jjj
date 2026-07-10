# demo2

面向上位机 V2 的 Rust MVP。项目把设备接入、协议解码、会话管理、事件分发、告警、持久化和 UI 展示拆成相对清晰的模块，当前推荐按“采集服务 + UI 客户端”两个进程运行。

## 当前能力

- TCP 设备接入：监听 `127.0.0.1:19010`，处理二进制帧、粘包拆包、CRC 校验和 ACK 回包。
- 串口接入：支持 legacy demo 帧，以及 `sent` / `sent1` / `sent2` / `sent3` 等 SENT 数据模式。
- CAN 接入：可通过 TSMaster / TC1012 相关配置启用，支持 SENT 数据滤波。
- UI 数据流：监听 `127.0.0.1:19011`，向 UI 客户端推送带版本头和长度前缀的 bincode 二进制帧。
- 健康检查：监听 `127.0.0.1:19012`，提供 `/health` 和 `/ready`。
- 控制接口：监听 `127.0.0.1:19013`，通过 TCP JSON 行命令更新 SENT 跳变阈值。
- 告警服务：按传感器阈值触发和恢复告警事件。
- PostgreSQL 持久化：写入遥测、告警和系统事件，遥测表按日期分区。
- UI 客户端：展示多传感器实时曲线、状态、告警记录和历史数据回放视图。

## 项目结构

```text
src/
  app/        告警服务和业务编排
  bus/        EventBus、Store、应用事件
  db/         PostgreSQL schema
  domain/     领域类型
  ingress/    TCP、串口、CAN 接入
  protocol/   帧协议、SENT 编解码
  session/    设备会话状态机、ACK、pending 请求
  signal/     信号处理
  transport/  TCP、串口、CAN 传输抽象
  bin/        collector、UI、sender、DB 工具
```

更细的架构说明见 `docs/architecture.md`，接口协议见 `docs/api.md`。

## 环境要求

- Rust stable，项目使用 edition 2024。
- Windows PowerShell，串口 / UI / 一键启动脚本主要按 Windows 环境编写。
- Docker Desktop，可选，用于启动本地 PostgreSQL。
- PostgreSQL 16，可选；未启用或不可用时，采集服务会降级为无持久化模式继续运行。

## 快速开始

1. 启动 PostgreSQL：

```powershell
docker compose up -d postgres
```

如果使用 Windows 本机安装的 PostgreSQL 18，而不是 Docker，请在管理员 PowerShell 中启动服务，并设置为开机自动启动：

```powershell
Start-Service -Name "postgresql-x64-18"
Set-Service -Name "postgresql-x64-18" -StartupType Automatic
Get-Service -Name "postgresql-x64-18"
```

不同 PostgreSQL 版本的服务名可能不同，可先运行 `Get-Service *postgres*` 查询实际名称。

`docker-compose.yml` 默认创建：

- 数据库：`demo2`
- 用户：`postgres`
- 密码：`123456`
- 端口：`5432`

2. 创建本地配置：

```powershell
Copy-Item config.toml.example config.toml
```

3. 初始化数据库和分区 schema：

```powershell
cargo run --bin init_db
```

4. 启动采集服务：

```powershell
cargo run --bin collector_service
```

5. 启动 UI 客户端：

```powershell
cargo run --bin ui_client
```

6. 发送测试 TCP 数据：

```powershell
cargo run --bin sender_stress_report
```

也可以使用 `sender_1min` 回放导出的 CAN 数据文件：

```powershell
$env:DEMO2_EXPORT_PATH = "D:\path\to\export.txt"
cargo run --bin sender_1min
```

## 一键启动

项目提供 Windows 启动脚本：

```powershell
.\scripts\run_demo2.ps1
```

常用参数：

```powershell
.\scripts\run_demo2.ps1 -WithDocker
.\scripts\run_demo2.ps1 -WithDocker -WithSender
```

- `-WithDocker`：先执行 `docker compose up -d postgres`。
- `-WithSender`：尝试启动相邻目录 `tcp_frame_sender` 中的 sender，默认 bin 为 `sender_1min`。
- `-SenderBin <name>`：指定外部 sender bin。

## 配置说明

`collector_service` 会读取项目根目录的 `config.toml`。未找到或解析失败时使用内置默认值。

常用配置：

```toml
[collector]
ingress_addr = "127.0.0.1:19010"
ui_feed_addr = "127.0.0.1:19011"
control_addr = "127.0.0.1:19013"
health_addr = "127.0.0.1:19012"

pg_dsn = "host=127.0.0.1 port=5432 user=postgres password=123456 dbname=demo2"
pg_connect_max_retries = 20
pg_connect_retry_ms = 1000

serial_port = "COM3"
serial_baud = 2000000
serial_mode = "sent"

can_enabled = false
can_hardware_name = "TC1012"
can_channel = 0
can_baud_kbps = 500
can_data_baud_kbps = 2000
can_channels = [0, 1]

db_filter_enabled = false
db_filter_order = 10
db_filter_sample_rate_hz = 48000.0
db_filter_cutoff_hz = 4000.0

sent_filter_enabled = true
sent_filter_window = 10
max_payload = 4096
bus_capacity = 10000
ui_feed_capacity = 10000
```

### T1 / T2 / S 数据处理

CAN SENT 原始值在 `src/ingress/can.rs` 的 `decode_sent_values` 中解析为 `f32`：

- T1 angle：offset 25
- T1 torque：offset 29
- T2 angle：offset 1
- T2 torque：offset 5
- S angle：offset 49

解析后有两层可选处理：

- `sent_filter_enabled`：CAN SENT 入口处的移动平均滤波。`SentMovingAverage` 在 `src/ingress/can.rs`，角度信号使用圆周平均，扭矩信号使用算术平均。
- `db_filter_enabled`：raw event bus 到 processed event bus 转发阶段的 Butterworth 低通滤波。启用后，UI feed、告警和数据库写入看到的都是处理后的值。

因此如果需要完整保留原始上传值，应同时关闭 `sent_filter_enabled` 和 `db_filter_enabled`。

常用环境变量覆盖：

- `DEMO2_DISABLE_DB=1`：禁用 PostgreSQL 持久化。
- `DEMO2_COLLECTOR_SERIAL_PORT` / `DEMO2_COLLECTOR_SERIAL_BAUD` / `DEMO2_COLLECTOR_SERIAL_MODE`：覆盖串口接入。
- `DEMO2_COLLECTOR_CAN_ENABLED` / `DEMO2_COLLECTOR_CAN_CHANNEL` / `DEMO2_COLLECTOR_CAN_CHANNELS` / `DEMO2_COLLECTOR_CAN_HW_NAME`：覆盖 CAN 接入。
- `DEMO2_COLLECTOR_CAN_TSMASTER_BIN` / `DEMO2_COLLECTOR_CAN_AUTOSTART_TSMASTER`：配置 TSMaster 启动。
- `DEMO2_COLLECTOR_CAN_BAUD_KBPS` / `DEMO2_COLLECTOR_CAN_DATA_BAUD_KBPS`：覆盖 CAN 仲裁/数据波特率。
- `DEMO2_SENT_FILTER_ENABLED` / `DEMO2_SENT_FILTER_WINDOW`：覆盖 SENT 滤波配置。
- `DEMO2_UI_FEED_ADDR`：覆盖 UI 客户端连接的 feed 地址。
- `DEMO2_UI_EMBED_COLLECTOR=1`：让 `ui_client` 启动内嵌 collector，默认关闭。
- `DEMO2_COLLECTOR_CONTROL_ADDR`：覆盖 UI 客户端连接的控制接口地址。
- `DEMO2_PG_DSN`：覆盖 UI 查询历史数据使用的数据库连接串。

## 二进制入口

- `collector_service`：核心采集服务，负责 TCP / 串口 / CAN 接入、事件转发、告警、持久化和健康检查。
- `ui_client`：egui UI 客户端，连接采集服务的版本化二进制 feed；默认只连接外部 collector，如需单进程演示可设置 `DEMO2_UI_EMBED_COLLECTOR=1`。
- `serial_frame_sender`：串口测试发送端，支持 legacy 与 SENT 格式。
- `serial_sender_ui`：串口发送 UI。
- `sender_stress_report`：内置 TCP 压测发送端，逐档提升发送频率并输出 ACK / 丢包统计。
- `sender_1min`：从导出文件回放前 30 秒 y 轴数据到 TCP ingress。
- `init_db`：创建数据库并初始化 schema。
- `check_db_status`：打印表、分区和最近遥测记录。
- `check_db_partition`：打印遥测分区状态。

## UI 客户端代码结构

`ui_client` 的二进制入口仍在 `src/bin/ui_client.rs`，当前入口只负责调用运行器；运行器会启动 UI feed 线程和 egui UI，并在 `DEMO2_UI_EMBED_COLLECTOR=1` 时额外启动内嵌 collector：

- `alarm.rs`：处理告警状态、CAN 阈值告警、SENT 跳变告警和样本落入曲线缓存。
- `config.rs`：读取 UI feed 地址和 PostgreSQL DSN。
- `events.rs`：处理 UI 队列消息、实时样本、状态更新、CAN 回放结果和报警记录查询结果。
- `feed.rs`：维护 TCP feed 连接、重连、长度前缀读取、版本化二进制帧解码、队列投递和 feed 统计。
- `fonts.rs`：配置 egui 中文字体。
- `messages.rs`：重新导出 collector 与 UI 共用的 `TelemetryMsg` / `UiFeedMsg`。
- `models.rs`：定义 UI 视图、告警、CAN 回放和报警记录相关状态结构。
- `records.rs`：查询和绘制报警记录窗口。
- `replay.rs`：加载、绘制和导出 CAN/SENT 历史回放数据。
- `runtime.rs`：组织“先启动 collector，再启动 UI”的入口流程。
- `series.rs`：维护每路传感器的滑窗曲线数据。
- `settings.rs`：集中 UI 客户端常量。
- `state.rs`：承载 `UiClientApp` 的主 UI 状态，`UiClientApp` 自身保留通道、feed 统计、连接地址和本地 DB writer 等外壳资源。
- `time.rs`：时间输入解析和显示/导出文件名格式化。
- `view.rs`：绘制顶部栏、告警面板、阈值面板、动态图表窗口和主 egui update。

## 串口联调

1. 在 `config.toml` 中配置采集端串口：

```toml
[collector]
serial_port = "COM3"
serial_baud = 2000000
serial_mode = "sent"
```

2. 启动采集服务和 UI：

```powershell
cargo run --bin collector_service
cargo run --bin ui_client
```

3. 启动串口发送端：

```powershell
cargo run --bin serial_frame_sender -- --port COM4 --baud 2000000 --format sent1
```

可选参数：

```text
--format legacy | sent1 | sent2 | sent3
--sensors <n>
--interval-ms <n>
--duration-secs <n>
```

## 健康检查与数据库检查

```powershell
Invoke-RestMethod http://127.0.0.1:19012/health
Invoke-RestMethod http://127.0.0.1:19012/ready
cargo run --bin check_db_status
cargo run --bin check_db_partition
```

`/health` 返回采集、UI、样本数、丢弃计数、数据库写入失败和最近数据库错误等状态；`/ready` 在 ingress 与 UI feed 均就绪时返回 200。

## 协议概要

完整接口说明见 `docs/api.md`。以下是 TCP legacy 上传协议概要。

TCP legacy 帧格式：

```text
magic:      u16 = 0xAA55
body_len:   u16
request_id: u64
kind:       u8
payload:    [u8; N]
crc16:      u16
```

常用 `kind`：

- `0x34`：遥测上报。
- `0x90`：ACK。
- `0xF0`：心跳请求。
- `0xF1`：心跳响应。

legacy 遥测 payload 示例：

```text
sid=3,value=47.381
```

## 常见问题

- `os error 10061`：目标端口没有服务在监听。先确认 `collector_service` 已启动，sender 连接 `19010`，UI 连接 `19011`。
- UI 无数据：检查 `/health` 的 `samples_rx` 是否增长，再检查 sender 是否成功连接并收到 ACK。
- 数据库无数据：先运行 `Get-Service *postgres*` 确认 Windows PostgreSQL 服务处于 `Running`；PostgreSQL 18 可在管理员 PowerShell 中运行 `Start-Service -Name "postgresql-x64-18"`，并用 `Set-Service -Name "postgresql-x64-18" -StartupType Automatic` 设置自动启动。然后确认 `pg_dsn` 密码为 `123456`；如不需要持久化，可设置 `DEMO2_DISABLE_DB=1`。
- `config.toml` 不生效：确认文件位于项目根目录，并且 `[collector]` 表名正确。

## 测试

```powershell
cargo test
cargo check
```
