//! TokenTracker CLI 生命周期管理
//!
//! 托管 `tracker serve`（tokentracker-cli npm 包）子进程，为前端提供
//! "Token 统计"面板数据源：
//! - 启动：spawn `tracker serve --port <p>`（stdout/stderr 落盘，无控制台窗口），
//!   端口探活线程置 Running
//! - 停止：taskkill /PID /T 优雅退出 → 超时强杀进程树
//! - 状态：事件驱动（进程退出回调）+ 端口探活兜底
//!
//! 看板地址 `http://127.0.0.1:<port>`（仅回环、无认证），前端 iframe 内嵌
//! 或独立窗口打开。tokentracker-cli 本身零编译（npm 分发），本模块只负责
//! 检测 / 安装 / 托管，不 vendor 任何 TokenTracker 代码。

use crate::core::command;
use crate::core::logging::{LogLevel, LogSource, Logger};
use crate::core::port;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// TokenTracker 运行状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TokentrackerStatus {
    /// 未运行
    Stopped,
    /// 正在启动
    Starting,
    /// 运行中
    Running,
    /// 停止中
    Stopping,
    /// 出错
    Error,
}

/// tracker serve 默认端口（与 tokentracker serve.js 默认一致；被占时自动 +1）
pub const DEFAULT_TRACKER_PORT: u16 = 7680;
/// 端口探测上限（与 tokentracker 的 DEFAULT_MAX_PORT_ATTEMPTS 对齐）
const MAX_PORT_ATTEMPTS: u16 = 20;

/// TokenTracker 进程管理器（全局单例，由 Tauri state 持有）
pub struct TokentrackerManager {
    child: Arc<Mutex<Option<Child>>>,
    pid: Arc<Mutex<u32>>,
    status: Arc<Mutex<TokentrackerStatus>>,
    port: Arc<Mutex<u16>>,
    logger: Arc<Logger>,
    stopping: AtomicBool,
    /// 生命周期操作互斥锁：同一时刻只有一个 start/stop 在执行
    op_lock: Mutex<()>,
}

impl TokentrackerManager {
    pub fn new(logger: Arc<Logger>) -> Self {
        Self {
            child: Arc::new(Mutex::new(None)),
            pid: Arc::new(Mutex::new(0)),
            status: Arc::new(Mutex::new(TokentrackerStatus::Stopped)),
            port: Arc::new(Mutex::new(0)),
            logger,
            stopping: AtomicBool::new(false),
            op_lock: Mutex::new(()),
        }
    }

    pub fn status(&self) -> TokentrackerStatus {
        self.status.lock().map(|s| *s).unwrap_or(TokentrackerStatus::Error)
    }

    pub fn current_port(&self) -> u16 {
        self.port.lock().map(|p| *p).unwrap_or(0)
    }

    /// 看板地址（仅回环，无认证）
    pub fn dashboard_url(&self) -> String {
        let p = self.current_port();
        if p == 0 {
            String::new()
        } else {
            format!("http://127.0.0.1:{p}")
        }
    }

    /// CLI 是否可用（PATH 中能解析 `tracker --version`）。
    /// 15s 超时兜底（npm 全局 .cmd 首次调用可能因 npm 检查变慢）。
    pub fn cli_available(&self) -> bool {
        let mut c = command::hidden_cmd("tracker");
        c.args(["--version"]);
        command::run_with_timeout(c, Duration::from_secs(15))
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// 安装 CLI（npm 全局安装 tokentracker-cli；Node ≥ 20，工具链面板已保障）
    pub fn install_cli(&self) -> Result<(), String> {
        let mut c = command::hidden_cmd("npm");
        c.args(["install", "-g", "tokentracker-cli"]);
        let out = command::run_with_timeout(c, Duration::from_secs(300))
            .map_err(|e| format!("npm install 失败: {e}"))?;
        if out.status.success() {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Info,
                "tokentracker-cli 已安装（npm 全局）",
            );
            Ok(())
        } else {
            let err = crate::core::text::decode(&out.stderr);
            Err(format!("npm install tokentracker-cli 失败：{}", err.trim()))
        }
    }

    /// 启动 tracker serve（阻塞等待 spawn 结果，返回实际端口）
    pub fn start(&self) -> Result<u16, String> {
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(
            self.status(),
            TokentrackerStatus::Running
                | TokentrackerStatus::Starting
                | TokentrackerStatus::Stopping
        ) {
            return Err("TokenTracker 正在运行或切换中".to_string());
        }

        // 收养优先（v0.5.8）：TokenTracker 云端 OAuth（谷歌登录）回调白名单仅含
        // 默认端口 7680 —— 端口递增到 7681+ 会导致回调被拒
        // ("http://127.0.0.1:7682/ is not in the allowed redirect URLs")。
        // 因此默认端口被占时，若监听进程是 tracker serve 则直接收养（同 7680，
        // 与独立窗口同 origin，localStorage 登录态共享）；仅非 tracker 占用时才递增。
        if port::is_port_in_use(DEFAULT_TRACKER_PORT) {
            if let Some(pid) = Self::pid_by_port(DEFAULT_TRACKER_PORT) {
                if Self::process_looks_like_tracker(pid) {
                    *self.pid.lock().unwrap() = pid;
                    *self.port.lock().unwrap() = DEFAULT_TRACKER_PORT;
                    *self.status.lock().unwrap() = TokentrackerStatus::Running;
                    self.logger.log(
                        LogSource::Launcher,
                        LogLevel::Info,
                        &format!(
                            "检测到外部 tracker serve 已占用 {}（pid={pid}），收养为托管看板",
                            DEFAULT_TRACKER_PORT
                        ),
                    );
                    return Ok(DEFAULT_TRACKER_PORT);
                }
            }
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!(
                    "端口 {DEFAULT_TRACKER_PORT} 被非 tracker 进程占用，看板将退让到更高端口（谷歌登录回调将不可用）"
                ),
            );
        }

        // 从默认端口开始探测空闲端口（7680 在部分 Windows 主机被 Delivery Optimization 占用）
        let mut port = DEFAULT_TRACKER_PORT;
        let mut free_port = None;
        for _ in 0..MAX_PORT_ATTEMPTS {
            if !port::is_port_in_use(port) {
                free_port = Some(port);
                break;
            }
            port += 1;
        }
        let port = free_port.ok_or_else(|| {
            format!(
                "{DEFAULT_TRACKER_PORT}~{} 端口均被占用，请释放后重试",
                DEFAULT_TRACKER_PORT + MAX_PORT_ATTEMPTS - 1
            )
        })?;
        if !self.cli_available() {
            return Err("未检测到 tokentracker-cli（npm 全局包），请先点击安装".to_string());
        }

        // stdout/stderr 重定向到日志文件（与 dsh 同模式：管道在部分机器不实时）
        let out_path = crate::core::logging::logs_dir().join("tokentracker.out.log");
        let err_path = crate::core::logging::logs_dir().join("tokentracker.err.log");
        let out_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out_path)
            .map_err(|e| format!("创建 tracker 输出日志失败: {e}"))?;
        let err_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&err_path)
            .map_err(|e| format!("创建 tracker 错误日志失败: {e}"))?;

        // hidden_cmd：cmd /D /C tracker ...（.cmd 包装 + node PATH 注入，无控制台窗口）
        let mut cmd = command::hidden_cmd("tracker");
        cmd.args(["serve", "--port", &port.to_string()]);
        cmd.stdout(Stdio::from(out_file))
            .stderr(Stdio::from(err_file))
            .stdin(Stdio::null());

        let child = cmd.spawn().map_err(|e| {
            self.logger.log(
                LogSource::Launcher,
                LogLevel::Error,
                &format!("spawn tracker serve 失败: {e}"),
            );
            format!("启动 tracker serve 失败: {e}（请确认 tokentracker-cli 已安装）")
        })?;

        self.logger.log(
            LogSource::Launcher,
            LogLevel::Info,
            &format!("TokenTracker 已启动 (pid={}, port={})", child.id(), port),
        );

        *self.pid.lock().unwrap() = child.id();
        *self.port.lock().unwrap() = port;
        *self.status.lock().unwrap() = TokentrackerStatus::Starting;
        self.stopping.store(false, Ordering::SeqCst);
        *self.child.lock().unwrap() = Some(child);
        let pid = *self.pid.lock().unwrap();

        self.spawn_monitor(pid);
        self.spawn_startup_probe(port, pid);
        Ok(port)
    }

    /// 停止（taskkill /T 优雅退出 → 等待 ≤1s → 超时强杀进程树）
    pub fn stop(&self) -> Result<(), String> {
        let _op = self.op_lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.stopping.swap(true, Ordering::SeqCst) {
            return Ok(()); // 已在停止中
        }
        let mut status = self.status.lock().unwrap();
        if !matches!(*status, TokentrackerStatus::Running | TokentrackerStatus::Starting) {
            self.stopping.store(false, Ordering::SeqCst);
            return Ok(());
        }
        *status = TokentrackerStatus::Stopping;
        drop(status);

        let pid = *self.pid.lock().unwrap();
        if pid != 0 {
            // 优雅排空（无 /F）；node 进程树对无 /F 的 taskkill 常不响应，1s 后升级强杀
            let mut c = command::hidden("taskkill");
            c.args(["/PID", &pid.to_string(), "/T"]);
            let _ = c.output();

            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                if !process_alive(pid) {
                    break;
                }
                if Instant::now() >= deadline {
                    command::kill_process_tree(pid);
                    break;
                }
                thread::sleep(Duration::from_millis(150));
            }
        }

        *self.pid.lock().unwrap() = 0;
        *self.port.lock().unwrap() = 0;
        *self.status.lock().unwrap() = TokentrackerStatus::Stopped;
        self.stopping.store(false, Ordering::SeqCst);
        self.logger.log(LogSource::Launcher, LogLevel::Info, "TokenTracker 已停止");
        Ok(())
    }

    /// 监视线程：子进程退出（非主动停止）→ 状态落 Error
    fn spawn_monitor(&self, pid: u32) {
        let child = self.child.lock().unwrap().take();
        let Some(mut child) = child else { return };
        let logger = Arc::clone(&self.logger);
        let status = Arc::clone(&self.status);
        thread::spawn(move || {
            let _ = child.wait();
            let s = status.lock().map(|s| *s).unwrap_or(TokentrackerStatus::Error);
            if s == TokentrackerStatus::Stopping || s == TokentrackerStatus::Stopped {
                return; // 主动停止路径已置 Stopped
            }
            logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("tracker serve (pid={pid}) 意外退出"),
            );
            let mut st = status.lock().unwrap();
            if matches!(*st, TokentrackerStatus::Running | TokentrackerStatus::Starting) {
                *st = TokentrackerStatus::Error;
            }
        });
    }

    /// 启动探活线程：端口监听则置 Running；超时置 Error
    fn spawn_startup_probe(&self, port: u16, pid: u32) {
        let status = Arc::clone(&self.status);
        let logger = Arc::clone(&self.logger);
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(20);
            while Instant::now() < deadline {
                if port::probe(port) == Some(true) {
                    let mut st = status.lock().unwrap();
                    if *st == TokentrackerStatus::Starting {
                        *st = TokentrackerStatus::Running;
                        logger.log(
                            LogSource::Launcher,
                            LogLevel::Info,
                            &format!("TokenTracker 看板就绪: http://127.0.0.1:{port}"),
                        );
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(400));
            }
            logger.log(
                LogSource::Launcher,
                LogLevel::Warn,
                &format!("tracker serve (pid={pid}) 端口 {port} 未在 20s 内就绪"),
            );
            let mut st = status.lock().unwrap();
            if *st == TokentrackerStatus::Starting {
                *st = TokentrackerStatus::Error;
            }
        });
    }
    /// 按端口查找监听进程 PID（Windows Get-NetTCPConnection，与 ProcessManager 同模式）
    fn pid_by_port(port: u16) -> Option<u32> {
        let ps = format!(
            "(Get-NetTCPConnection -LocalPort {port} -State Listen | Select-Object -First 1 -ExpandProperty OwningProcess)"
        );
        let mut c = command::hidden("powershell");
        c.args(["-NoProfile", "-Command", &ps]);
        let out = command::run_with_timeout(c, Duration::from_secs(15)).ok()?;
        if !out.status.success() {
            return None;
        }
        crate::core::text::decode(&out.stdout).trim().parse().ok()
    }

    /// 监听进程是否形如 tracker serve（命令行含 tracker.js / tokentracker）
    fn process_looks_like_tracker(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        let ps = format!(
            "(Get-CimInstance Win32_Process -Filter \"ProcessId={pid}\" | Select-Object -First 1 -ExpandProperty CommandLine)"
        );
        let mut c = command::hidden("powershell");
        c.args(["-NoProfile", "-Command", &ps]);
        match command::run_with_timeout(c, Duration::from_secs(15)) {
            Ok(out) if out.status.success() => {
                let cmd = crate::core::text::decode(&out.stdout).to_lowercase();
                cmd.contains("tracker.js") || cmd.contains("tokentracker")
            }
            _ => false,
        }
    }
}

/// 检查指定 PID 的进程是否存活（tasklist 精确过滤，解析第 2 列避免数字子串误判）
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let mut c = command::hidden("tasklist");
    c.args(["/FI", &format!("PID eq {pid}"), "/NH"]);
    match c.output() {
        Ok(out) => {
            let text = crate::core::text::decode(&out.stdout);
            text.lines().any(|line| {
                let mut cols = line.split_whitespace();
                cols.next(); // 映像名
                cols.next()
                    .and_then(|p| p.parse::<u32>().ok())
                    .map(|p| p == pid)
                    .unwrap_or(false)
            })
        }
        Err(_) => false,
    }
}
