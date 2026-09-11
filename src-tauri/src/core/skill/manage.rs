//! 技能写操作编排：身份校验 → 备份 → 原子写入 → 复验 → 回滚
//!
//! 设计铁律（ADR-0007 D2/D8/D10/D11）：
//! 1. **身份校验优先**：写前必须重新扫描受管根，确认「路径仍在受管根内 ∧ 磁盘上该路径的
//!    frontmatter `name` 与前端声明一致」。不通过即拒绝，且**此前不读目标文件**。
//!    这防的是「面板打开后技能被重命名/移动，用户点下的开关误伤另一个同名技能」。
//! 2. **前置失败即不写**：任何拒绝路径都不得留下半写状态。
//! 3. **备份 + 复验 + 回滚**：写前备份原字节；写后复验（frontmatter 仍可解析 ∧ `name`/
//!    `description` 仍在 ∧ 目标键取值为预期）；复验失败用备份回滚。
//! 4. **幂等**：目标状态已达成时返回 `changed: false`，不写盘、不发事件。
//! 5. **删除可恢复**：移入 `<root>/.trash/<name>-<ts>/`；**符号链接技能硬拒绝**。

use crate::core::logging::Logger;
use crate::core::plugin::managed;
use crate::core::skill::frontmatter::{self, Toggle};
use crate::core::skill::scan::{self, SkillError};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 备份根目录：`%LOCALAPPDATA%\dsh-launcher\backups\skills`
///
/// 与插件的 `backups\plugins`、MCP 的 `backups\mcp` 平级，互不干扰。
pub fn backup_root() -> PathBuf {
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("dsh-launcher")
        .join("backups")
        .join("skills")
}

/// Unix 秒字符串（沿用插件注册表的做法：避免引入时间库）
fn now_secs() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// 开关操作结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToggleReport {
    /// 是否真的发生了写入（false = 已是目标状态，幂等空操作）
    pub changed: bool,
    /// 面向用户的结果说明
    pub message: String,
    /// 备份文件路径（未写盘时为 None）
    pub backup: Option<String>,
}

/// 删除操作结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteReport {
    /// 移入回收站后的路径
    pub trashed_to: String,
    /// 面向用户的结果说明
    pub message: String,
}

/// 原子写（与 `plugin::managed::write_atomic` 同模式，但临时文件后缀正确）。
///
/// `managed::write_atomic` 把临时名硬编码为 `.yml.tmp`，对 `SKILL.md` 会产生
/// `SKILL.yml.tmp` 这一语义错位的中间文件；本案不改动 `managed.rs`（避免波及插件与
/// MCP 两条既有链路），在此实现后缀正确的版本。
fn write_atomic_md(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("md.tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("写入临时文件 {} 失败: {e}", tmp.display()))?;
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("移除旧文件 {} 失败: {e}", path.display())
        })?;
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换 {} 失败: {e}", path.display())
    })
}

/// 把原文件备份到 `backups\skills\<name>\<ts>\SKILL.md`，返回备份路径。
fn backup_skill(skill_path: &Path, name: &str, original: &str) -> Result<PathBuf, String> {
    // 名字用于目录名，需过滤非法字符（技能名本身已受官方字符集约束，此处仍防御）
    let safe_name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = backup_root().join(safe_name).join(now_secs());
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录 {} 失败: {e}", dir.display()))?;
    let file_name = skill_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "SKILL.md".to_string());
    let target = dir.join(file_name);
    std::fs::write(&target, original).map_err(|e| format!("写入备份 {} 失败: {e}", target.display()))?;
    Ok(target)
}

/// 复验：重新读取文件，确认 frontmatter 仍可解析、`name`/`description` 仍在、
/// 且目标键取值符合预期。任一不成立即需回滚。
fn verify_after_write(skill_path: &Path, expect_disabled: bool) -> Result<(), String> {
    let text = std::fs::read_to_string(skill_path)
        .map_err(|e| format!("复验读取失败: {e}"))?;
    let name = scan::resolve_managed_field(&text, "name");
    let description = scan::resolve_managed_field(&text, "description");
    if name.is_none() {
        return Err("复验失败：写入后 frontmatter 的 name 丢失".to_string());
    }
    if description.is_none() {
        return Err("复验失败：写入后 frontmatter 的 description 丢失".to_string());
    }
    let state = frontmatter::disable_state(&text);
    let ok = if expect_disabled {
        state == frontmatter::DisableState::Disabled
    } else {
        matches!(
            state,
            frontmatter::DisableState::Enabled | frontmatter::DisableState::Unset
        )
    };
    if !ok {
        return Err(format!(
            "复验失败：写入后 disable-model-invocation 取值不符合预期（当前 {state:?}）"
        ));
    }
    Ok(())
}

/// 用备份内容回滚目标文件
fn rollback(skill_path: &Path, original: &str) -> Result<(), String> {
    write_atomic_md(skill_path, original)
}

/// 身份校验（ADR-0007 D10）：路径必须仍解析为受管技能，且其 frontmatter `name` 与声明一致。
///
/// 返回该技能所属的根路径。
fn assert_identity(skill_path: &Path, declared_name: &str) -> Result<PathBuf, SkillError> {
    if !skill_path.is_file() {
        return Err(SkillError::NotManaged(format!(
            "路径不存在或不是文件：{}",
            skill_path.display()
        )));
    }
    // 定位所属受管根（同时确保路径落在受管根之下）
    let mut owning_root: Option<PathBuf> = None;
    for root in scan::managed_roots() {
        let Ok(root_real) = std::fs::canonicalize(&root.path) else {
            continue;
        };
        let Ok(target_real) = std::fs::canonicalize(skill_path) else {
            continue;
        };
        if target_real.starts_with(&root_real) {
            owning_root = Some(root.path.clone());
            break;
        }
    }
    let Some(root_path) = owning_root else {
        return Err(SkillError::NotManaged(
            "该路径不在受管的用户级技能根内".to_string(),
        ));
    };

    let actual = scan::resolve_managed(skill_path).ok_or_else(|| {
        SkillError::NotManaged("无法解析该技能的 frontmatter name".to_string())
    })?;
    if actual != declared_name {
        return Err(SkillError::NotManaged(format!(
            "磁盘上的技能名已变为 \"{actual}\"（面板显示 \"{declared_name}\"）"
        )));
    }
    Ok(root_path)
}

/// 启用/停用技能（单一开关 = `disable-model-invocation`）
pub fn set_enabled(
    skill_path: &Path,
    declared_name: &str,
    enabled: bool,
    logger: &Arc<Logger>,
) -> Result<ToggleReport, SkillError> {
    // ① 身份校验（此前不读目标文件内容）
    assert_identity(skill_path, declared_name)?;

    // ② 取锁（与 cordis.patch.yml 的写入共用按路径重入锁）
    let _guard = managed::file_write_lock(skill_path);

    // ③ 读取
    let original = managed::read_file(skill_path)
        .map_err(SkillError::Io)?
        .ok_or_else(|| SkillError::NotManaged("文件不存在".to_string()))?;

    // ④ 外科手术（严格：任何歧义即拒绝）
    let toggle = frontmatter::set_disable_model_invocation(&original, !enabled)
        .map_err(SkillError::Frontmatter)?;

    let new_text = match toggle {
        // ⑤ 幂等：已是目标状态 → 不写盘、不备份、不发事件
        Toggle::Unchanged => {
            let word = if enabled { "启用" } else { "停用" };
            return Ok(ToggleReport {
                changed: false,
                message: format!("{declared_name} 已是「{word}」状态，无需变更"),
                backup: None,
            });
        }
        Toggle::Rewritten(text) => text,
    };

    // ⑥ 备份原字节
    let backup_path = backup_skill(skill_path, declared_name, &original).map_err(SkillError::Io)?;

    // ⑦ 原子写入
    if let Err(error) = write_atomic_md(skill_path, &new_text) {
        // 写入失败：目标未被替换（原子写保证），但可能留下临时文件
        logger.warn(&format!("技能写入失败 {declared_name}: {error}"));
        return Err(SkillError::Io(error));
    }

    // ⑧ 复验；失败即回滚
    if let Err(error) = verify_after_write(skill_path, !enabled) {
        logger.warn(&format!("技能写入复验失败，回滚 {declared_name}: {error}"));
        if let Err(rollback_error) = rollback(skill_path, &original) {
            return Err(SkillError::Io(format!(
                "{error}；且回滚失败：{rollback_error}（备份在 {}）",
                backup_path.display()
            )));
        }
        return Err(SkillError::Io(format!("{error}（已回滚到原内容）")));
    }

    let word = if enabled { "启用" } else { "停用" };
    logger.info(&format!(
        "技能 {declared_name} 已{word}（dsh live 热重载，无需重启；备份 {}）",
        backup_path.display()
    ));
    Ok(ToggleReport {
        changed: true,
        message: format!("{declared_name} 已{word}（dsh 热重载生效，无需重启）"),
        backup: Some(backup_path.display().to_string()),
    })
}

/// 删除技能（移入回收站，可恢复）
pub fn delete(
    skill_path: &Path,
    declared_name: &str,
    logger: &Arc<Logger>,
) -> Result<DeleteReport, SkillError> {
    // 目录包技能才有独立目录可移动；平铺 `.md` 技能与目录包同样处理其父定位
    let owning_root = assert_identity(skill_path, declared_name)?;

    // 目录包：技能目录 = SKILL.md 的父目录；仅是文件本身为平铺技能时，直接移动该文件
    let is_bundled = skill_path
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("SKILL.md"))
        .unwrap_or(false);

    // 符号链接技能硬拒绝（ADR-0007 D11）：移动链接会把链接**目标**移出原位
    if is_bundled {
        if let Some(dir) = skill_path.parent() {
            let meta = std::fs::symlink_metadata(dir).map_err(|e| {
                SkillError::Io(format!("读取 {} 元数据失败: {e}", dir.display()))
            })?;
            if meta.file_type().is_symlink() {
                return Err(SkillError::NotManaged(
                    "该技能是符号链接，删除会移走链接目标，故拒绝（可停用它代替）".to_string(),
                ));
            }
        }
    } else {
        let meta = std::fs::symlink_metadata(skill_path).map_err(|e| {
            SkillError::Io(format!("读取 {} 元数据失败: {e}", skill_path.display()))
        })?;
        if meta.file_type().is_symlink() {
            return Err(SkillError::NotManaged(
                "该技能是符号链接，删除会移走链接目标，故拒绝（可停用它代替）".to_string(),
            ));
        }
    }

    // 回收站目录：<root>/.trash/<name>-<ts>/
    let trash_dir = owning_root.join(scan::TRASH_DIR);
    std::fs::create_dir_all(&trash_dir)
        .map_err(|e| SkillError::Io(format!("创建回收站 {} 失败: {e}", trash_dir.display())))?;
    let target = trash_dir.join(format!("{declared_name}-{}", now_secs()));
    if target.exists() {
        return Err(SkillError::Io(format!(
            "回收站目标已存在：{}",
            target.display()
        )));
    }

    let source = if is_bundled {
        skill_path
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| SkillError::NotManaged("无法定位技能目录".to_string()))?
    } else {
        skill_path.to_path_buf()
    };

    // 同根内 rename，原子
    std::fs::rename(&source, &target).map_err(|e| {
        SkillError::Io(format!(
            "移入回收站失败（{} → {}）: {e}",
            source.display(),
            target.display()
        ))
    })?;

    logger.info(&format!(
        "技能 {declared_name} 已移入回收站 {}（可从该目录恢复）",
        target.display()
    ));
    Ok(DeleteReport {
        trashed_to: target.display().to_string(),
        message: format!(
            "{declared_name} 已移入回收站（可恢复）：{}",
            target.display()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 备份根在_localappdata_下() {
        let root = backup_root();
        assert!(root.ends_with("skills"));
        assert!(root.parent().unwrap().ends_with("backups"));
    }

    #[test]
    fn 时间戳是数字() {
        let ts = now_secs();
        assert!(ts.chars().all(|c| c.is_ascii_digit()), "ts={ts}");
    }
}
