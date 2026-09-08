//! 技能共享资源 IPC：状态 / 应用共享模式 / 迁移
//!
//! 薄壳：全部业务在 `core::skill`。

use crate::core::dshhome::MANAGED_PROFILE;
use crate::core::plugin::state::PluginError;
use crate::core::skill::{self, MigrateReport, SkillApplyReport, SkillStatus};
use crate::AppState;
use std::sync::Arc;
use tauri::State;

fn format_error(error: PluginError) -> String {
    format!("[{}] {}", error.kind.exit_code(), error.message)
}

/// 读取共享资源状态
#[tauri::command]
pub async fn skill_status() -> Result<SkillStatus, String> {
    tauri::async_runtime::spawn_blocking(skill::status)
        .await
        .map_err(|e| format!("任务执行失败: {e}"))
}

/// 应用共享模式（auto/link/config）
#[tauri::command]
pub async fn skill_apply(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    mode: String,
    resource: Option<String>,
) -> Result<SkillApplyReport, String> {
    let logger = Arc::clone(&state.logger);
    let selected = resource.filter(|value| !value.is_empty());
    let result = tauri::async_runtime::spawn_blocking(move || {
        skill::apply(MANAGED_PROFILE, &mode, selected.as_deref(), &logger)
    })
    .await
    .map_err(|e| format!("任务执行失败: {e}"))?
    .map_err(format_error)?;
    crate::core::events::emit_skill_changed(&app);
    Ok(result)
}

/// 迁移冲突资源到共享真源（dryRun 只报告）
#[tauri::command]
pub async fn skill_migrate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    dry_run: bool,
) -> Result<MigrateReport, String> {
    let logger = Arc::clone(&state.logger);
    let result = tauri::async_runtime::spawn_blocking(move || skill::migrate(dry_run, &logger))
        .await
        .map_err(|e| format!("任务执行失败: {e}"))?
        .map_err(format_error)?;
    if !dry_run {
        crate::core::events::emit_skill_changed(&app);
    }
    Ok(result)
}
