//! `cordis.patch.yml` 的受管区块（marker 包裹、块外逐字节保留、原子写）
//!
//! dsh 只热重载两个用户 patch 层文件：`$DSH_HOME/profiles/<p>/cordis.patch.yml`
//! 与 `$DSH_HOME/cordis.patch.yml`（见 profile-boot.ts 的 watchUserPatches）。
//! 启动器需要往这两个文件里写两类内容，因此必须与用户手写内容共存：
//!
//! - **managed 区块**（`dsh-launcher managed v1`）：按行启停（`- id:` + `disabled:`）；
//! - **shared 区块**（`dsh-launcher shared v1`）：技能/指令共享的行配置（`- id:` + `config:`）。
//!
//! 共同契约：
//! - 启动器只拥有 marker 之间的一段；
//! - marker 之外的字节**逐字节不变**（注释、空行、行尾风格都保留）；
//! - 同一期望态渲染出同一字节序列 → 可判定"无变化"从而不落盘（幂等的基础）。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// managed 区块 marker 前缀（版本号参与解析，升级走迁移）
pub const MARK_BEGIN_PREFIX: &str = "# >>> dsh-launcher managed ";
/// managed 区块结束 marker 前缀
pub const MARK_END_PREFIX: &str = "# <<< dsh-launcher managed ";
/// shared 区块 marker 前缀
pub const SHARED_BEGIN_PREFIX: &str = "# >>> dsh-launcher shared ";
/// shared 区块结束 marker 前缀
pub const SHARED_END_PREFIX: &str = "# <<< dsh-launcher shared ";
/// 当前区块版本
pub const MARK_VERSION: &str = "v1";

/// managed 区块起始 marker 完整文本
pub fn mark_begin() -> String {
    format!("{MARK_BEGIN_PREFIX}{MARK_VERSION} — 由启动器维护，请勿手工编辑 >>>")
}
/// managed 区块结束 marker 完整文本
pub fn mark_end() -> String {
    format!("{MARK_END_PREFIX}{MARK_VERSION} <<<")
}
/// shared 区块起始 marker 完整文本
pub fn shared_mark_begin() -> String {
    format!("{SHARED_BEGIN_PREFIX}{MARK_VERSION} — 由启动器维护，请勿手工编辑 >>>")
}
/// shared 区块结束 marker 完整文本
pub fn shared_mark_end() -> String {
    format!("{SHARED_END_PREFIX}{MARK_VERSION} <<<")
}

/// 受管条目：一行插件的启停
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedEntry {
    /// 行 id（与 dump 中的 `- id:` 一致）
    pub id: String,
    /// 期望 disabled 值
    pub disabled: bool,
    /// 归属包名（仅渲染为行尾注释，便于人工排查）
    pub package: Option<String>,
}

impl ManagedEntry {
    pub fn new(id: impl Into<String>, disabled: bool, package: Option<String>) -> Self {
        Self {
            id: id.into(),
            disabled,
            package,
        }
    }
}

/// 写入结果：无变化（不落盘）/ 已写入
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockOutcome {
    Unchanged,
    Written,
}

/// 校验行 id 是否可安全写入 YAML（防注入）。
pub fn validate_row_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 200 {
        return Err(format!("非法行 id: {id:?}"));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@' | '/'))
    {
        return Err(format!("非法行 id（含不安全字符）: {id:?}"));
    }
    Ok(())
}

/// 读取文件（不存在返回 None）。
fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取 {} 失败: {e}", path.display())),
    }
}

/// 探测行尾风格（默认 LF）。
fn detect_eol(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// 去掉行内注释（` # ` 之后），用于解析 `- id: x  # plugin: y`。
fn strip_inline_comment(line: &str) -> &str {
    match line.find(" # ") {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// 解析 managed 区块文本（marker 之间的内容）。
fn parse_block(block: &str) -> Result<Vec<ManagedEntry>, String> {
    let mut entries: Vec<ManagedEntry> = Vec::new();
    for raw in block.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.trim().is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("- id: ") {
            let id = strip_inline_comment(rest).trim().to_string();
            validate_row_id(&id)?;
            entries.push(ManagedEntry {
                id,
                disabled: false,
                package: None,
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("  disabled: ") {
            if line.starts_with("   ") {
                return Err(format!(
                    "受管区块出现非法缩进行（只允许 2 空格缩进的 disabled）: {}",
                    line.trim()
                ));
            }
            let value = strip_inline_comment(rest).trim();
            let disabled = match value {
                "true" => true,
                "false" => false,
                other => {
                    return Err(format!(
                        "受管区块 disabled 取值非法（只允许 true/false）: {other:?}"
                    ))
                }
            };
            let Some(last) = entries.last_mut() else {
                return Err("受管区块出现 disabled 但没有对应的 - id 行".to_string());
            };
            last.disabled = disabled;
            continue;
        }
        return Err(format!(
            "受管区块出现非法行（只允许 `- id:` 与 `  disabled:`）: {}",
            line.trim()
        ));
    }
    Ok(entries)
}

/// 区块在文件中的定位结果
enum Located {
    /// 文件不存在或没有 marker
    None,
    /// 找到完整区块
    Block {
        before: String,
        block: String,
        after: String,
    },
    /// marker 不成对/重复
    Broken(String),
}

fn locate_with(content: &str, begin_marker: &str, end_marker: &str) -> Located {
    let begin_count = content.matches(begin_marker).count();
    let end_count = content.matches(end_marker).count();
    if begin_count == 0 && end_count == 0 {
        return Located::None;
    }
    if begin_count != 1 || end_count != 1 {
        return Located::Broken(format!(
            "受管区块 marker 不成对或重复（起始 {begin_count} 个 / 结束 {end_count} 个）"
        ));
    }
    let Some(begin_pos) = content.find(begin_marker) else {
        return Located::Broken("受管区块缺少起始 marker".to_string());
    };
    let block_start = begin_pos + begin_marker.len();
    let Some(end_rel) = content[block_start..].find(end_marker) else {
        return Located::Broken("受管区块缺少结束 marker".to_string());
    };
    let end_pos = block_start + end_rel;
    Located::Block {
        before: content[..begin_pos].to_string(),
        block: content[block_start..end_pos].to_string(),
        after: content[end_pos + end_marker.len()..].to_string(),
    }
}

/// 规范化排序（按 id 字典序），保证同一期望态渲染稳定。
fn sorted(entries: &[ManagedEntry]) -> Vec<ManagedEntry> {
    let mut out = entries.to_vec();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// 渲染 managed 区块体（不含 marker）。
fn render_managed_body(entries: &[ManagedEntry], eol: &str) -> String {
    let mut out = String::new();
    for entry in sorted(entries) {
        out.push_str(&format!("- id: {}", entry.id));
        if let Some(pkg) = &entry.package {
            out.push_str(&format!("  # plugin: {pkg}"));
        }
        out.push_str(eol);
        out.push_str(&format!(
            "  disabled: {}",
            if entry.disabled { "true" } else { "false" }
        ));
        out.push_str(eol);
    }
    out
}

/// 渲染完整 managed 区块（含 marker）。
pub fn render_block(entries: &[ManagedEntry], eol: &str) -> String {
    format!(
        "{}{eol}{}{eol}",
        mark_begin(),
        render_managed_body(entries, eol).trim_end_matches(eol)
    )
}

/// 原子写文件（临时文件 + rename，Windows 上先删目标）。
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录 {} 失败: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("yml.tmp");
    fs::write(&tmp, content).map_err(|e| format!("写入临时文件 {} 失败: {e}", tmp.display()))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("替换 {} 失败: {e}", path.display())
    })
}

/// 定位“文档体为空数组 `[]`”的情形。
///
/// patch 文件必须是**单个**顶层 YAML 数组：模板里的 `[]` 是空数组占位符，
/// 在它后面追加 `- id: ...` 会变成两个 YAML 文档（dsh 解析直接报错），
/// 因此写受管区块时必须把 `[]` 这一行**替换**掉，而不是追加在其后。
/// @returns `(该行之前的文本, 该行之后的文本)`；不是空数组占位符时返回 None
fn split_empty_array(content: &str) -> Option<(String, String)> {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            offset += line.len();
            continue;
        }
        if trimmed == "[]" {
            return Some((
                content[..offset].to_string(),
                content[offset + line.len()..].to_string(),
            ));
        }
        return None;
    }
    None
}

/// 是否含有实质 YAML 内容（非注释、非空行）。
fn has_significant(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    })
}

/// 文档体是否只由注释/空行/块序列行组成（块序列才能安全追加）。
fn is_block_sequence(content: &str) -> bool {    content.lines().all(|line| {
        let trimmed = line.trim_start();
        trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || line.starts_with(' ')
            || line.starts_with('\t')
    })
}

/// 通用区块写入：`body` 为 marker 之间的内容（None/空 = 删除区块）。
fn apply_body(
    path: &Path,
    begin_marker: &str,
    end_marker: &str,
    body: Option<&str>,
) -> Result<BlockOutcome, String> {
    let existing = read_optional(path)?;
    let eol = detect_eol(existing.as_deref().unwrap_or(""));
    let rendered = match body {
        None => String::new(),
        Some(text) if text.trim().is_empty() => String::new(),
        Some(text) => format!(
            "{begin_marker}{eol}{}{eol}{end_marker}{eol}",
            text.trim_end_matches(|c| c == '\n' || c == '\r')
                .replace('\n', eol)
        ),
    };
    let empty = rendered.is_empty();

    let Some(content) = existing else {
        if empty {
            return Ok(BlockOutcome::Unchanged);
        }
        return match write_atomic(path, &rendered) {
            Ok(()) => Ok(BlockOutcome::Written),
            Err(e) => Err(e),
        };
    };

    match locate_with(&content, begin_marker, end_marker) {
        Located::Broken(reason) => Err(format!("{}: {reason}", path.display())),
        Located::None => {
            if empty {
                return Ok(BlockOutcome::Unchanged);
            }
            // ① 空数组占位符 `[]`：替换该行（否则会形成两个 YAML 文档）
            if let Some((before, after)) = split_empty_array(&content) {
                let mut out = before;
                out.push_str(&rendered);
                out.push_str(after.trim_start_matches(|c| c == '\r' || c == '\n'));
                write_atomic(path, &out)?;
                return Ok(BlockOutcome::Written);
            }
            // ② 块序列（或纯注释/空文件）：安全追加
            if !is_block_sequence(&content) {
                return Err(format!(
                    "{}: 顶层不是 YAML 数组（既不是 `[]` 也不是块序列），拒绝写入受管区块",
                    path.display()
                ));
            }
            let mut out = content.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(eol);
            }
            if !out.is_empty() {
                out.push_str(eol);
            }
            out.push_str(&rendered);
            write_atomic(path, &out)?;
            Ok(BlockOutcome::Written)
        }
        Located::Block {
            before,
            block,
            after,
        } => {
            let existing_rendered = format!("{begin_marker}{block}{end_marker}");
            if !empty && existing_rendered.trim_end() == rendered.trim_end() {
                return Ok(BlockOutcome::Unchanged);
            }
            if empty {
                if block.trim().is_empty() {
                    return Ok(BlockOutcome::Unchanged);
                }
                let mut out = before.clone();
                out.push_str(after.trim_start_matches(|c| c == '\r' || c == '\n'));
                // 删掉最后一条受管条目后，文件里可能只剩注释——必须补回 `[]`，
                // 否则 dsh 解析到空文档会报 “must be a top-level YAML array”。
                if !has_significant(&out) {
                    if out.is_empty() {
                        out = "[]\n".to_string();
                    } else {
                        if !out.ends_with('\n') {
                            out.push_str(eol);
                        }
                        out.push_str("[]\n");
                    }
                }
                write_atomic(path, &out)?;
            } else {
                // 区块已存在时，顺手清理它前面可能残留的 `[]` 占位符
                let healed_before = match split_empty_array(&before) {
                    Some((head, _tail)) => head,
                    None => before.clone(),
                };
                let mut out = healed_before;
                out.push_str(&rendered);
                out.push_str(&after);
                write_atomic(path, &out)?;
            }
            Ok(BlockOutcome::Written)
        }
    }
}

/// 读取 managed 区块（无区块返回 None）。
pub fn read_block(path: &Path) -> Result<Option<Vec<ManagedEntry>>, String> {
    let Some(content) = read_optional(path)? else {
        return Ok(None);
    };
    match locate_with(&content, &mark_begin(), &mark_end()) {
        Located::None => Ok(None),
        Located::Broken(reason) => Err(format!("{}: {reason}", path.display())),
        Located::Block { block, .. } => Ok(Some(parse_block(&block)?)),
    }
}

/// 把 managed 区块写入文件（只动区块，块外逐字节保留）。
///
/// - 条目为空且无区块 → `Unchanged`（不创建文件）；
/// - 条目为空且有区块 → 删除区块；
/// - 渲染结果与现有区块一致 → `Unchanged`（不落盘，幂等）。
pub fn apply_block(path: &Path, entries: &[ManagedEntry]) -> Result<BlockOutcome, String> {
    for entry in entries {
        validate_row_id(&entry.id)?;
    }
    let existing = read_optional(path)?;
    let eol = detect_eol(existing.as_deref().unwrap_or(""));
    let body = if entries.is_empty() {
        None
    } else {
        Some(render_managed_body(entries, eol))
    };
    apply_body(path, &mark_begin(), &mark_end(), body.as_deref())
}

/// 读取 shared 区块体（无区块返回 None）。
pub fn read_shared_body(path: &Path) -> Result<Option<String>, String> {
    let Some(content) = read_optional(path)? else {
        return Ok(None);
    };
    match locate_with(&content, &shared_mark_begin(), &shared_mark_end()) {
        Located::None => Ok(None),
        Located::Broken(reason) => Err(format!("{}: {reason}", path.display())),
        Located::Block { block, .. } => Ok(Some(block.trim().to_string())),
    }
}

/// 写入 shared 区块体（None 或空白 = 删除区块）。
pub fn apply_shared_body(path: &Path, body: Option<&str>) -> Result<BlockOutcome, String> {
    apply_body(
        path,
        &shared_mark_begin(),
        &shared_mark_end(),
        body,
    )
}

/// 合并/覆盖若干条目到现有区块（按 id 覆盖，其余保留），返回新条目集合。
pub fn upsert(
    existing: Option<Vec<ManagedEntry>>,
    updates: &[ManagedEntry],
    remove_ids: &[String],
) -> Vec<ManagedEntry> {
    let mut map: BTreeMap<String, ManagedEntry> = BTreeMap::new();
    for entry in existing.unwrap_or_default() {
        map.insert(entry.id.clone(), entry);
    }
    for id in remove_ids {
        map.remove(id);
    }
    for entry in updates {
        map.insert(entry.id.clone(), entry.clone());
    }
    map.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-launcher-managed-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("cordis.patch.yml")
    }

    const USER_TEMPLATE: &str = "# 用户自己的注释\n# 第二行\n[]\n";

    #[test]
    fn test_append_to_user_file_preserves_bytes() {
        let path = temp_path("append");
        fs::write(&path, USER_TEMPLATE).unwrap();
        let entries = vec![
            ManagedEntry::new("cost-meter", false, Some("dsh-cost-meter".into())),
            ManagedEntry::new("dsh-market", true, Some("dshmarket".into())),
        ];
        assert_eq!(apply_block(&path, &entries).unwrap(), BlockOutcome::Written);
        let content = fs::read_to_string(&path).unwrap();
        // 注释逐字节保留；`[]` 占位符被替换（否则会形成两个 YAML 文档）
        assert!(content.starts_with("# 用户自己的注释\n# 第二行\n"));
        assert!(!content.contains("\n[]\n"));
        assert!(content.contains("- id: cost-meter  # plugin: dsh-cost-meter"));
        assert!(content.contains("- id: dsh-market  # plugin: dshmarket"));
        let cost = content.find("cost-meter").unwrap();
        let market = content.find("dsh-market").unwrap();
        assert!(cost < market);
        // 幂等：同样条目再次写入 → Unchanged，且文件不变
        let before = fs::read(&path).unwrap();
        assert_eq!(
            apply_block(&path, &entries).unwrap(),
            BlockOutcome::Unchanged
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn test_empty_array_placeholder_is_replaced_not_appended() {
        // dsh 初始化 profile 时的真实模板
        let path = temp_path("placeholder");
        let template = "# Your patch layer for this dsh profile, applied after every bundle layer:\n# a top-level YAML array of loader patch entries\n[]\n";
        fs::write(&path, template).unwrap();
        apply_block(&path, &[ManagedEntry::new("cost-meter", true, None)]).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        // 单文档：`[]` 行不存在，块直接接在注释之后
        assert!(!content.lines().any(|line| line.trim() == "[]"), "{content}");
        assert!(content.contains("- id: cost-meter"));
        assert!(content.contains("  disabled: true"));
        // 顶层确实是数组（第一行非注释内容以 `- ` 开头）
        let first_significant = content
            .lines()
            .find(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            .unwrap();
        assert!(first_significant.starts_with("- "), "{first_significant}");
    }

    #[test]
    fn test_non_array_file_is_rejected() {
        let path = temp_path("not-array");
        fs::write(&path, "root: {}\n").unwrap();
        let err = apply_block(&path, &[ManagedEntry::new("a", true, None)]).unwrap_err();
        assert!(err.contains("不是 YAML 数组"), "{err}");
        // 拒绝写入：文件未被修改
        assert_eq!(fs::read_to_string(&path).unwrap(), "root: {}\n");
    }

    #[test]
    fn test_update_existing_block_only_touches_block() {
        let path = temp_path("update");
        apply_block(&path, &[ManagedEntry::new("a", true, None)]).unwrap();
        let with_user_edit = fs::read_to_string(&path).unwrap() + "\n# 用户追加\n";
        fs::write(&path, &with_user_edit).unwrap();

        assert_eq!(
            apply_block(&path, &[ManagedEntry::new("a", false, None)]).unwrap(),
            BlockOutcome::Written
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("# 用户追加"));
        assert!(content.contains("  disabled: false"));
        assert!(!content.contains("  disabled: true"));
    }

    #[test]
    fn test_remove_block_restores_template_when_only_block() {
        let path = temp_path("remove-only");
        apply_block(&path, &[ManagedEntry::new("a", true, None)]).unwrap();
        assert_eq!(apply_block(&path, &[]).unwrap(), BlockOutcome::Written);
        assert_eq!(fs::read_to_string(&path).unwrap(), "[]\n");
        assert_eq!(apply_block(&path, &[]).unwrap(), BlockOutcome::Unchanged);
    }

    #[test]
    fn test_missing_file_with_empty_entries_is_unchanged() {
        let path = temp_path("missing");
        assert_eq!(apply_block(&path, &[]).unwrap(), BlockOutcome::Unchanged);
        assert!(!path.exists(), "空条目不得创建文件");
    }

    #[test]
    fn test_broken_marker_fails_loud() {
        let path = temp_path("broken");
        fs::write(
            &path,
            format!("{}\n- id: a\n  disabled: true\n", mark_begin()),
        )
        .unwrap();
        let err = apply_block(&path, &[ManagedEntry::new("a", false, None)]).unwrap_err();
        assert!(err.contains("不成对") || err.contains("缺少结束"), "{err}");
    }

    #[test]
    fn test_crlf_preserved() {
        let path = temp_path("crlf");
        fs::write(&path, "# 注释\r\n[]\r\n").unwrap();
        apply_block(&path, &[ManagedEntry::new("a", true, None)]).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("\r\n"));
    }

    #[test]
    fn test_row_id_validation() {
        assert!(validate_row_id("ui-task-board").is_ok());
        assert!(validate_row_id("@scope/pkg/row").is_ok());
        assert!(validate_row_id("bad id").is_err());
        assert!(validate_row_id("bad\nid").is_err());
        assert!(validate_row_id("").is_err());
    }

    #[test]
    fn test_upsert_overrides_and_removes() {
        let existing = vec![
            ManagedEntry::new("a", true, None),
            ManagedEntry::new("b", false, None),
        ];
        let out = upsert(
            Some(existing),
            &[ManagedEntry::new("a", false, None)],
            &["b".to_string()],
        );
        assert_eq!(out, vec![ManagedEntry::new("a", false, None)]);
    }

    #[test]
    fn test_shared_block_roundtrip_and_idempotency() {
        let path = temp_path("shared");
        fs::write(&path, USER_TEMPLATE).unwrap();
        let body = "- id: skill-filesystem\n  config:\n    agentsHome: 'C:\\\\a\\\\.agents\\\\agent'\n";
        assert_eq!(
            apply_shared_body(&path, Some(body)).unwrap(),
            BlockOutcome::Written
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("# 用户自己的注释\n# 第二行\n"));
        assert!(!content.lines().any(|line| line.trim() == "[]"));
        assert!(content.contains(shared_mark_begin().as_str()));
        assert_eq!(
            read_shared_body(&path).unwrap().as_deref(),
            Some(body.trim())
        );
        // 幂等
        assert_eq!(
            apply_shared_body(&path, Some(body)).unwrap(),
            BlockOutcome::Unchanged
        );
        // managed 区块与 shared 区块互不干扰
        apply_block(&path, &[ManagedEntry::new("row-a", true, None)]).unwrap();
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains(shared_mark_begin().as_str()));
        assert!(content.contains(mark_begin().as_str()));
        assert!(read_block(&path).unwrap().unwrap().len() == 1);
        // 删除 shared 区块后 managed 区块仍在
        assert_eq!(
            apply_shared_body(&path, None).unwrap(),
            BlockOutcome::Written
        );
        let content = fs::read_to_string(&path).unwrap();
        assert!(!content.contains(shared_mark_begin().as_str()));
        assert!(content.contains(mark_begin().as_str()));
    }
}
