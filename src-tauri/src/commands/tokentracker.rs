//! TokenTracker IPC 命令：状态 / 检测 / 安装 / 启动 / 停止 + 看板窗口
//!
//! 并发策略与 commands/dsh.rs 一致：async 命令 + spawn_blocking（阻塞操作不进
//! async 运行时）；窗口创建经 run_on_main_thread 回主线程（Tauri 窗口非线程安全）。

use crate::core::tokentracker::TokentrackerStatus;
use crate::AppState;
use tauri::{Manager, State};

/// 查询 TokenTracker 运行状态（快，直接返回）
#[tauri::command]
pub fn get_tokentracker_status(state: State<'_, AppState>) -> TokentrackerStatus {
    state.tokentracker.status()
}

/// 当前看板端口（0 = 未运行）
#[tauri::command]
pub fn get_tokentracker_port(state: State<'_, AppState>) -> u16 {
    state.tokentracker.current_port()
}

/// 检测 tokentracker-cli 是否已安装（PATH 中 tracker 可用）
#[tauri::command]
pub async fn detect_tokentracker_cli(state: State<'_, AppState>) -> Result<bool, String> {
    let m = std::sync::Arc::clone(&state.tokentracker);
    tauri::async_runtime::spawn_blocking(move || m.cli_available())
        .await
        .map_err(|e| format!("任务执行失败: {e}"))
}

/// 安装 tokentracker-cli（npm 全局，Node ≥ 20）
#[tauri::command]
pub async fn install_tokentracker_cli(state: State<'_, AppState>) -> Result<String, String> {
    let m = std::sync::Arc::clone(&state.tokentracker);
    tauri::async_runtime::spawn_blocking(move || {
        m.install_cli()?;
        Ok::<_, String>("tokentracker-cli 安装完成".to_string())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 启动 tracker serve（阻塞等待 spawn；返回实际端口）
#[tauri::command]
pub async fn start_tokentracker(state: State<'_, AppState>) -> Result<u16, String> {
    let m = std::sync::Arc::clone(&state.tokentracker);
    tauri::async_runtime::spawn_blocking(move || {
        let port = m.start()?;
        Ok::<_, String>(port)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 停止 tracker serve
#[tauri::command]
pub async fn stop_tokentracker(state: State<'_, AppState>) -> Result<String, String> {
    let m = std::sync::Arc::clone(&state.tokentracker);
    tauri::async_runtime::spawn_blocking(move || {
        m.stop()?;
        Ok::<_, String>("TokenTracker 已停止".to_string())
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
}

/// 打开 TokenTracker 看板独立窗口（主线程建窗，失败落日志）
#[tauri::command]
pub async fn open_tokentracker_dashboard(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let url = state.tokentracker.dashboard_url();
    if url.is_empty() {
        return Err("TokenTracker 未在运行，请先启动".to_string());
    }
    let app2 = app.clone();
    app2.run_on_main_thread(move || {
        let _ = create_dashboard_window(&app, &url);
    })
    .map_err(|e| format!("调度窗口创建到主线程失败: {e}"))?;
    Ok("已请求打开 TokenTracker 看板窗口".to_string())
}

/// 创建 TokenTracker 看板窗口（独立 WebviewWindow，loopback 导航全放行）
fn create_dashboard_window(app: &tauri::AppHandle, url: &str) -> Result<String, String> {
    let parsed: tauri::Url = url
        .parse()
        .map_err(|_| format!("非法看板地址: {url}"))?;
    let label = format!("tokentracker-dash-{}", unique_window_suffix());

    let builder = tauri::WebviewWindowBuilder::new(
        app,
        label.clone(),
        tauri::WebviewUrl::External(parsed),
    )
    .title("TokenTracker 统计")
    .inner_size(1500.0, 900.0)
    // 载体窗口：导航全放行（TokenTracker 看板内部跳转不拦截）
    .on_navigation(|_url| true);

    builder.build().map_err(|e| {
        if let Some(state) = app.try_state::<AppState>() {
            state.logger.log(
                crate::core::logging::LogSource::Launcher,
                crate::core::logging::LogLevel::Warn,
                &format!("创建 TokenTracker 看板窗口失败: {e}"),
            );
        }
        format!("创建看板窗口失败: {e}")
    })?;
    Ok(label)
}

/// 窗口 label 唯一化（与 commands/dsh.rs 同法：毫秒时间戳 + 进程内计数）
fn unique_window_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{ts:x}-{count:x}")
}
