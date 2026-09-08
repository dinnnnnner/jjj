# demo2 API 文档

本文档描述当前源码实际暴露的进程间接口、帧协议、消息格式和数据库表结构。默认地址来自 `CollectorConfig::default()` 和 `config.toml.example`。

## 1. 端口总览

| 接口 | 默认地址 | 协议 | 说明 |
| --- | --- | --- | --- |
| TCP ingress | `127.0.0.1:19010` | 自定义二进制帧/TCP | 设备或测试 sender 上传遥测帧。 |
| UI feed | `127.0.0.1:19011` | 长度前缀二进制帧/TCP | collector 向 UI 推送遥测、告警和状态消息。 |
| Health | `127.0.0.1:19012` | HTTP/1.1 | 健康检查和 ready 检查。 |
| Control | `127.0.0.1:19013` | JSON Lines/TCP | UI 或工具下发 collector 控制命令。 |

## 2. TCP Ingress 帧协议

帧结构：

```text
magic:      u16 = 0xAA55
body_len:   u16
request_id: u64
kind:       u8
payload:    [u8; N]
crc16:      u16
```

`body_len` 覆盖 `request_id + kind + payload`。CRC16 的计算范围也是 `request_id + kind + payload`。`SimpleFrameCodec` 会处理粘包、拆包、错位恢复、长度上限和 CRC 校验。

常用 `kind`：

| kind | 方向 | 说明 |
| --- | --- | --- |
| `0x34` | 设备 -> collector | 遥测上报。 |
| `0x90` | collector -> 设备 | ACK，payload 当前为 `demo2-ack`。 |
| `0xF0` | 任意 | 心跳请求。 |
| `0xF1` | 任意 | 心跳响应。 |

legacy 遥测 payload 是 UTF-8 文本：

```text
sid=<sensor_id>,value=<float>
```

示例：

```text
sid=3,value=47.381
```

collector 收到合法遥测后会发布 `TelemetrySample`，并回发 `kind = 0x90` 的 ACK。

## 3. UI Feed 二进制协议

UI feed 是 TCP 长连接，不是 HTTP 或 JSON Lines。客户端连接 `ui_feed_addr` 后，collector 连续发送长度前缀帧：

```text
wire_len:         u32，大端，表示后续 frame 的字节数
frame_magic:      [u8; 4] = "JJJF"
protocol_version: u16，大端，当前为 3
payload:          bincode(UiFeedMsg)，固定宽度整数编码
```

`wire_len` 自身不计入 frame 长度。`frame_magic + protocol_version + payload` 最大为 1 MiB（1,048,576 字节），因此 bincode payload 最大为 1,048,570 字节。collector 和 UI 客户端共用 `src/feed.rs` 中的 `MAX_FEED_FRAME_LEN`、`encode_feed_msg` 和 `decode_feed_msg`。

解码器会拒绝超过上限的帧、截断的协议头、错误魔数、未知版本、无效 payload 和尾随字节。协议没有保留旧 JSON Lines 的兼容路径；collector 与 UI 客户端应使用相同版本的共享类型。

payload 的逻辑消息类型为：

```rust
enum UiFeedMsg {
    Telemetry(TelemetryMsg),
    Alarm(AlarmUpdate),
    Status(String),
    AlarmSnapshot(AlarmSnapshot),
}
```

### 3.1 telemetry

v3 保留 v2 的 `captured_at_ms: i64`（采集端生成的 Unix 毫秒），写库及重试不重新生成时间；告警消息改为带版本的结构，collector 与 UI 必须同时升级。

`AlarmUpdate { epoch: Uuid, revision: u64, event: AlarmEvent }` 和 `AlarmSnapshot { epoch: Uuid, revision: u64, alarms: Vec<AlarmEvent> }` 使用同一告警状态版本。采集服务启动时生成 `epoch`，每次触发或恢复原子递增 `revision`。连接时及每秒请求快照。增量与快照在同一告警状态锁内入队，由单一 UI 告警转发任务发送，快照不会越过队列中更早的增量。UI 拒绝不晚于已接收快照或同一告警最新增量的旧事件，应用较旧快照时保留更晚的触发和恢复。新 `epoch` 的快照重置活动状态，旧 `epoch` 的增量被忽略。快照不增加历史计数，不补齐丢失的历史事件。

CAN 以每通道首个有效硬件微秒时间戳和回调入口主机时间建立映射，后续采样沿用硬件时间差，同帧信号共用时间。零时间戳回退到回调时间，下一个有效时间戳重新校准；硬件时间倒退也按时钟重置重新校准。输出时间被限制为不小于上一帧，主机时钟倒退时也不会打乱曲线点的顺序。这依赖每通道回调有序；绝对时间包含初次回调延迟，不保证跨设备时钟同步。

```text
TelemetryMsg {
  captured_at_ms: 1700000000123,
  device_id: "tcp://127.0.0.1:54321",
  sensor_id: 0,
  axis: "",
  alarm_bit: false,
  t_sec: 1.23,
  value: 47.381,
  request_id: 100,
  source_kind: TcpFrame,
}
```

字段说明：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `device_id` | string | 设备标识，例如 `tcp://...`、`serial://COM3` 或 CAN 设备 ID。 |
| `sensor_id` | integer | 传感器编号。 |
| `axis` | string | collector 根据 `source_kind` 和 `sensor_id` 推导出的轴名。 |
| `alarm_bit` | boolean | 上游协议携带的告警位。 |
| `t_sec` | number | 样本相对时间，单位秒。 |
| `value` | number | 样本值。 |
| `request_id` | integer | 原始请求 ID 或入口生成的序号。 |
| `source_kind` | string | 遥测来源。 |

`source_kind` 当前包括：

```text
Unknown
SerialDemo
SerialSent1
SerialSent2
SerialSent3
CanAxis
CanSent
TcpFrame
FrameStream
```

`axis` 推导规则：

| 来源 | sensor_id | axis |
| --- | --- | --- |
| `SerialDemo` | `0/1/2` | `x/y/z` |
| `CanAxis` | `0/1/2` | `x/y/z` |
| `CanSent` | `0/1/2/3/4` | `t1_angle/t1_torque/t2_angle/t2_torque/s_angle` |
| 其他 | 任意 | 空字符串 |

### 3.2 alarm

```text
AlarmEvent {
  device_id: "can://TC1016",
  alarm_id: "sent_torque_jump_t1",
  level: Critical,
  message: "T1 torque jump=0.350, warn=0.200, red=0.300, purple=0.400",
  raised_at: ...,
  cleared: false,
}
```

`level` 来自 `AlarmLevel`，当前代码使用 `Info`、`Warning`、`Critical`、`Purple`。`raised_at` 是 Rust `SystemTime` 通过 bincode 编码的时间字段；`cleared = true` 表示告警恢复事件。

### 3.3 status

```text
Status("collector serial legacy session started")
```

`status` 用于运行状态提示和系统事件转发。

## 4. Control JSON Lines

control 接口是 TCP JSON 行协议。客户端连接 `control_addr`，发送一行 JSON 命令后读取一行 JSON 响应，连接可关闭。

### 4.1 设置 SENT 跳变阈值

请求：

```json
{
  "type": "set_sent_jump_thresholds",
  "torque_warn": 0.2,
  "torque_red": 0.3,
  "torque_purple": 0.4,
  "angle_t1_red": 0.2,
  "angle_t2_red": 0.2,
  "angle_s_red": 1.0
}
```

成功响应：

```json
{"ok":true}
```

失败响应：

```json
{"ok":false,"error":"SENT jump thresholds must be finite numbers"}
```

阈值会被规范化：扭矩阈值取绝对值并保证 `warn <= red <= purple`，T1、T2 和 S 的角度阈值分别取绝对值。旧客户端仍可发送 `angle_t_red`，collector 会将它同时用于 T1 和 T2。更新成功后 collector 会发布一条 `status` 消息到 UI feed。

## 5. Health HTTP API

### 5.1 `GET /health`

返回 HTTP 200 和 collector 运行状态：

```json
{
  "ingress_ready": true,
  "ui_ready": true,
  "ingress_connections": 1,
  "ui_clients": 1,
  "samples_rx": 12345,
  "ui_drop": 0,
  "db_drop": 0,
  "db_write_fail": 0,
  "last_db_error": null
}
```

`ui_drop` 统计 UI 转发或客户端队列溢出，以及告警 feed 编码失败。没有 UI 订阅者时的发送失败不计入丢包，空闲期间的定时快照不会增加此指标。

### 5.2 `GET /ready`

当 TCP ingress 和 UI feed 都已就绪时返回：

```json
{"ready":true}
```

否则返回 HTTP 503：

```json
{"ready":false}
```

## 6. 串口协议

collector 的 `serial_mode` 支持：

```text
legacy
demo
sent
sent1
sent2
sent3
```

`sent1`、`sent2`、`sent3` 在 collector 配置解析时按 SENT 模式处理，具体语义由 SENT 帧 `pause` 字段区分。

SENT 帧固定 10 字节：

```text
sync marker: 0xF0
status:      4-bit
channel_1:   12-bit
channel_2:   12-bit
crc:         4-bit
pause:       4-bit
```

`pause` 映射：

| pause | 来源 | 发布 sensor |
| --- | --- | --- |
| `0x1` | SENT1 | `0/1` |
| `0x6` | SENT2 | `2/3` |
| `0xB` | SENT3 | `4` |

## 7. CAN 数据映射

普通 CAN 三轴样本：

| CAN ID | sensor_id | axis |
| --- | --- | --- |
| `0x100` | `0` | `x` |
| `0x102` | `1` | `y` |
| `0x104` | `2` | `z` |

SENT over CAN 按 ID 互斥选择格式。ID 1 使用 offset 4/6/8 的小端 `i16`，分别发布 S angle（4）、T2 angle（2）、T2 torque（3）；ID 2 使用 offset 6/8，发布 T1 angle（0）、T1 torque（1）。这两类帧要求至少 10 字节，填充到 64 字节也不进入浮点解码器。

非 TX、非 ID 1/2/3 且至少 53 字节的帧使用以下 `f32` 小端偏移：

| sensor_id | 信号 | offset |
| --- | --- | --- |
| `0` | T1 angle | `25` |
| `1` | T1 torque | `29` |
| `2` | T2 angle | `1` |
| `3` | T2 torque | `5` |
| `4` | S angle | `49` |

`identifier = 3` 表示 SENT error 帧，会被转换为告警事件。`sent_filter_enabled` 对上述三种数据格式都生效，按设备、通道、信号维护独立窗口，只更新当前帧含有的信号。

## 8. 数据库表

collector 使用 PostgreSQL，schema 定义在 `src/db/mod.rs`。

### 8.1 telemetry_samples

```text
id BIGINT GENERATED ALWAYS AS IDENTITY
ts_ms BIGINT NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
device_id TEXT NOT NULL
sensor_id INTEGER NOT NULL
axis TEXT NOT NULL DEFAULT ''
alarm_bit BOOLEAN NOT NULL DEFAULT FALSE
t_sec DOUBLE PRECISION NOT NULL
value DOUBLE PRECISION NOT NULL
request_id BIGINT NOT NULL
PRIMARY KEY (created_at, id)
```

该表按 `created_at` 日期范围分区。写入前会调用 `ensure_telemetry_partition_for_day(target_day DATE)` 确保当天分区存在。

### 8.2 alarm_events

```text
id BIGSERIAL PRIMARY KEY
ts_ms BIGINT NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
device_id TEXT NOT NULL
alarm_id TEXT NOT NULL
level TEXT NOT NULL
message TEXT NOT NULL
cleared BOOLEAN NOT NULL
```

### 8.3 system_events

```text
id BIGSERIAL PRIMARY KEY
ts_ms BIGINT NOT NULL
created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP
level TEXT NOT NULL
message TEXT NOT NULL
```
