//! Optional Windows display telemetry. UI timing is never used as a fallback.
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const STALE_AFTER: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub(crate) struct DisplayStats {
    pub(crate) fps: Option<f64>,
    pub(crate) status: String,
}

impl DisplayStats {
    fn unavailable(status: impl Into<String>) -> Self {
        Self {
            fps: None,
            status: status.into(),
        }
    }
}

pub(crate) struct DisplayMonitor {
    latest: Arc<Mutex<DisplayStats>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DisplayMonitor {
    pub(crate) fn start() -> Self {
        // Constructing the app in ordinary unit tests must not start ETW sessions.
        if cfg!(test) {
            return Self {
                latest: Arc::new(Mutex::new(DisplayStats::unavailable("单元测试未启用采集"))),
                stop: Arc::new(AtomicBool::new(false)),
                worker: None,
            };
        }
        Self::start_capture()
    }

    fn start_capture() -> Self {
        let latest = Arc::new(Mutex::new(DisplayStats::unavailable("正在启动 PresentMon")));
        let stop = Arc::new(AtomicBool::new(false));
        let state = latest.clone();
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            if let Err(error) = capture(&state, &stopping) {
                *state.lock().unwrap() = DisplayStats::unavailable(error);
            }
        });
        Self {
            latest,
            stop,
            worker: Some(worker),
        }
    }

    pub(crate) fn snapshot(&self) -> DisplayStats {
        self.latest.lock().unwrap().clone()
    }
}

impl Drop for DisplayMonitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn executable() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DEMO2_PRESENTMON_PATH") {
        let path = PathBuf::from(path);
        return path
            .is_file()
            .then_some(path)
            .ok_or_else(|| "DEMO2_PRESENTMON_PATH 指定的文件不存在".into());
    }
    let name = "PresentMon-2.3.1-x64.exe";
    let mut paths = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tools/presentmon")
            .join(name),
    ];
    if let Ok(app) = std::env::current_exe() {
        if let Some(dir) = app.parent() {
            paths.insert(0, dir.join(name));
            paths.insert(1, dir.join("tools/presentmon").join(name));
        }
    }
    paths
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "未找到 PresentMon；请运行 scripts/install_presentmon.ps1，或设置 DEMO2_PRESENTMON_PATH"
                .into()
        })
}

#[derive(Debug)]
struct DisplayFrame {
    chain: String,
    displayed_at_ms: f64,
}

struct CsvSchema {
    pid: usize,
    chain: usize,
    start: usize,
    latency: usize,
    duration: usize,
}

impl CsvSchema {
    fn parse(line: &str) -> Option<Self> {
        let fields = csv_fields(line)?;
        let column = |name| fields.iter().position(|field| field == name);
        Some(Self {
            pid: column("ProcessID")?,
            chain: column("SwapChainAddress")?,
            start: column("CPUStartTime")?,
            latency: column("DisplayLatency")?,
            duration: column("DisplayedTime")?,
        })
    }

    fn frame(&self, line: &str, target_pid: u32) -> Option<DisplayFrame> {
        let fields = csv_fields(line)?;
        if fields.get(self.pid)?.parse::<u32>().ok()? != target_pid {
            return None;
        }
        let number = |index: usize| {
            fields
                .get(index)?
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite() && *n >= 0.0)
        };
        // NA means dropped / display timing unavailable, never a displayed frame.
        if number(self.duration)? <= 0.0 {
            return None;
        }
        let displayed_at_ms = number(self.start)? + number(self.latency)?;
        if !displayed_at_ms.is_finite() {
            return None;
        }
        Some(DisplayFrame {
            chain: fields.get(self.chain)?.clone(),
            displayed_at_ms,
        })
    }
}

// PresentMon emits one CSV record per line; support quoted executable names.
fn csv_fields(line: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut chars = line
        .trim_end_matches(['\r', '\n'])
        .trim_start_matches('\u{feff}')
        .chars()
        .peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        match ch {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(ch),
        }
    }
    if quoted {
        return None;
    }
    fields.push(field);
    Some(fields)
}

struct ChainWindow {
    times: VecDeque<f64>,
    last_received: Instant,
}

#[derive(Default)]
struct FrameTracker {
    chains: HashMap<String, ChainWindow>,
}

impl FrameTracker {
    fn push(&mut self, frame: DisplayFrame, received: Instant) {
        self.chains
            .retain(|_, chain| received.duration_since(chain.last_received) <= STALE_AFTER);
        let chain = self
            .chains
            .entry(frame.chain)
            .or_insert_with(|| ChainWindow {
                times: VecDeque::new(),
                last_received: received,
            });
        // Repeated / out-of-order display events must not inflate FPS.
        if chain
            .times
            .back()
            .is_some_and(|last| frame.displayed_at_ms <= *last)
        {
            return;
        }
        chain.last_received = received;
        chain.times.push_back(frame.displayed_at_ms);
        let cutoff = frame.displayed_at_ms - 1000.0;
        // Retain one timestamp at/before the window boundary for full intervals.
        while chain.times.len() > 2 && chain.times[1] <= cutoff {
            chain.times.pop_front();
        }
        while chain.times.len() > 4096 {
            chain.times.pop_front();
        }
    }

    fn snapshot(&self, now: Instant) -> DisplayStats {
        // This UI has one native viewport. If additional chains appear, select
        // the most active recent chain rather than adding unrelated FPS values.
        let candidate = self
            .chains
            .iter()
            .filter(|(_, chain)| now.duration_since(chain.last_received) <= STALE_AFTER)
            .filter_map(|(id, chain)| {
                let span = chain.times.back()? - chain.times.front()?;
                (span >= 500.0).then(|| (id, (chain.times.len() - 1) as f64 * 1000.0 / span))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match candidate {
            Some((id, fps)) => DisplayStats {
                fps: Some(fps),
                status: format!(
                    "PresentMon 显示事件；约 1 秒窗口；呈现链 {id}。ETW 数据有延迟；多个呈现链取最活跃的一条。"
                ),
            },
            None => DisplayStats::unavailable(
                "等待有效显示事件（可能尚未采集、窗口不可见或驱动未提供数据）；超过 3 秒无事件时不保留旧帧率",
            ),
        }
    }
}

#[cfg(not(windows))]
fn capture(_: &Arc<Mutex<DisplayStats>>, _: &AtomicBool) -> Result<(), String> {
    Err("Displayed FPS 仅支持 Windows PresentMon".into())
}

#[cfg(windows)]
fn capture(latest: &Arc<Mutex<DisplayStats>>, stop: &AtomicBool) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let path = executable()?;
    let pid = std::process::id();
    let session = format!("demo2-display-{pid}");
    let mut child = Command::new(&path)
        .args([
            "--process_id",
            &pid.to_string(),
            "--session_name",
            &session,
            "--output_stdout",
            "--no_console_stats",
            "--v2_metrics",
            "--no_track_gpu",
            "--no_track_input",
            "--terminate_on_proc_exit",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 PresentMon: {error}"))?;

    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(2048);
    let reader = thread::spawn(move || {
        for line in BufReader::new(output).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    // Always drain stderr so a warning cannot block the capture process.
    let diagnostic = Arc::new(Mutex::new(String::new()));
    let diagnostic_writer = diagnostic.clone();
    let error_reader = thread::spawn(move || {
        for line in BufReader::new(errors).lines().map_while(Result::ok) {
            let mut text = diagnostic_writer.lock().unwrap();
            if text.len() < 8192 {
                text.push_str(&line);
                text.push('\n');
            }
        }
    });

    let mut schema = None;
    let mut tracker = FrameTracker::default();
    let mut last_publish = Instant::now();
    let mut result = Ok(());
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if schema.is_none() {
                    schema = CsvSchema::parse(&line);
                    if schema.is_none() && line.contains("ProcessID") {
                        result = Err(
                            "PresentMon CSV 格式不兼容；请使用安装脚本提供的 2.3.1 版本".into()
                        );
                        break;
                    }
                } else if let Some(frame) = schema.as_ref().unwrap().frame(&line, pid) {
                    tracker.push(frame, Instant::now());
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                result = Err("PresentMon 已停止采集".into());
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_publish.elapsed() >= Duration::from_millis(250) {
            *latest.lock().unwrap() = tracker.snapshot(Instant::now());
            last_publish = Instant::now();
        }
    }

    // Release the bounded reader before stopping the child. Stop only our ETW
    // session, allowing PresentMon to close it normally; never touch other tools.
    drop(rx);
    if child.try_wait().ok().flatten().is_none() {
        if let Ok(mut terminator) = Command::new(&path)
            .args(["--session_name", &session, "--terminate_existing_session"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let deadline = Instant::now() + Duration::from_secs(2);
            while terminator.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(25));
            }
            let _ = terminator.kill();
            let _ = terminator.wait();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(25));
        }
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = reader.join();
    let _ = error_reader.join();
    if result.is_err() {
        let detail = diagnostic.lock().unwrap();
        if detail.to_lowercase().contains("access denied") {
            return Err("PresentMon 权限不足：请以管理员身份运行 UI，或加入 Performance Log Users 用户组后重新登录。".into());
        }
        if !detail.is_empty() {
            return Err(format!("{}：{}", result.unwrap_err(), detail.trim()));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    const HEADER: &str =
        "Application,ProcessID,SwapChainAddress,CPUStartTime,DisplayLatency,DisplayedTime";

    #[test]
    fn filters_dropped_wrong_pid_and_invalid_display_data() {
        let schema = CsvSchema::parse(HEADER).unwrap();
        for row in [
            "ui,7,A,100,5,NA",
            "ui,8,A,100,5,16",
            "ui,7,A,100,NaN,16",
            "ui,7,A,100,5,0",
            "ui,7,A,100,-5,16",
            "ui,7,A",
        ] {
            assert!(schema.frame(row, 7).is_none(), "{row}");
        }
        let frame = schema.frame("\"ui, name.exe\",7,A,100,5,16", 7).unwrap();
        assert_eq!(frame.displayed_at_ms, 105.0);
        assert_eq!(frame.chain, "A");
    }

    #[test]
    fn uses_display_timestamps_not_cpu_or_csv_arrival_rate() {
        let schema = CsvSchema::parse(HEADER).unwrap();
        let now = Instant::now();
        let mut tracker = FrameTracker::default();
        // CPU submits at 100 FPS, display advances at 50 FPS; all rows arrive
        // together as an ETW batch. Neither CPU nor arrival timing is the metric.
        for n in 0..=100 {
            let row = format!("ui,7,A,{},{},20", n * 10, n * 10 + 5);
            tracker.push(schema.frame(&row, 7).unwrap(), now);
        }
        assert!((tracker.snapshot(now).fps.unwrap() - 50.0).abs() < 0.001);
        assert!(tracker.snapshot(now + Duration::from_secs(4)).fps.is_none());
    }

    #[test]
    fn does_not_sum_chains_or_count_repeated_frames() {
        let now = Instant::now();
        let mut tracker = FrameTracker::default();
        for n in 0..=60 {
            for chain in ["A", "B"] {
                let frame = || DisplayFrame {
                    chain: chain.into(),
                    displayed_at_ms: n as f64 * 1000.0 / 60.0,
                };
                tracker.push(frame(), now);
                tracker.push(frame(), now);
            }
        }
        assert!((tracker.snapshot(now).fps.unwrap() - 60.0).abs() < 0.001);
    }

    #[test]
    fn long_held_frame_reduces_rate_and_recovers() {
        let now = Instant::now();
        let mut tracker = FrameTracker::default();
        for time in [0.0, 1000.0] {
            tracker.push(
                DisplayFrame {
                    chain: "A".into(),
                    displayed_at_ms: time,
                },
                now,
            );
        }
        assert_eq!(tracker.snapshot(now).fps, Some(1.0));
        for n in 1..=60 {
            tracker.push(
                DisplayFrame {
                    chain: "A".into(),
                    displayed_at_ms: 1000.0 + n as f64 * 1000.0 / 60.0,
                },
                now,
            );
        }
        assert!((tracker.snapshot(now).fps.unwrap() - 60.0).abs() < 0.001);
    }

    #[test]
    fn missing_schema_and_initial_data_are_not_zero_fps() {
        assert!(CsvSchema::parse("Application,ProcessID,CPUStartTime").is_none());
        assert!(
            FrameTracker::default()
                .snapshot(Instant::now())
                .fps
                .is_none()
        );
        assert!(csv_fields("\"unterminated").is_none());
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "starts the installed PresentMon executable and checks its lifecycle"]
    fn installed_capture_lifecycle() {
        let monitor = DisplayMonitor::start_capture();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let stats = monitor.snapshot();
            if stats.status.contains("权限不足") || stats.status.contains("未找到") {
                assert!(stats.fps.is_none());
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let stats = monitor.snapshot();
        eprintln!("PresentMon smoke: {stats:?}");
        assert_ne!(stats.status, "正在启动 PresentMon");
        let shutdown = Instant::now();
        drop(monitor);
        assert!(shutdown.elapsed() < Duration::from_secs(6));
    }
}
