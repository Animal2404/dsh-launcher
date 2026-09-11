//! 技能扫描：枚举**受管用户级技能根**、解析 frontmatter、标注 rank 生效关系
//!
//! 事实依据（deepseek-harness，逐条标注官方出处）：
//! - 两个用户级根：`<dshHome>/skills`（rank 400，带 `skipSystem` 即忽略 `.system`）
//!   与 `<agentsHome>/skills`（rank 500），见
//!   `packages/skill/skill-filesystem/src/index.ts:40,253-254`。
//! - 发现只认**根顶层**的 `<name>/SKILL.md` 与 `<name>.md`；
//!   **嵌套 `**/SKILL.md` 刻意不被发现**，见 `skill-filesystem/README.md:36,146`。
//! - rank 数值大者生效（同名技能只有一个进入目录）。
//!
//! 本模块**只读**：不写任何文件。单个技能的解析失败不应让整个面板失败，
//! 故解析是**宽容**的（失败即标注为不可用状态），与写入路径的**严格**拒绝互补。

use crate::core::dshhome;
use crate::core::skill::frontmatter::{self, DisableState};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// 官方 rank 常量（只复刻，不自创优先级）
pub const RANK_USER_DSH: u32 = 400;
/// 官方 rank 常量：`<agentsHome>/skills`
pub const RANK_USER_AGENTS: u32 = 500;

/// 技能文件固定名
const SKILL_FILE: &str = "SKILL.md";
/// 回收站目录名（本产品约定；官方不会把它当作技能：既非顶层 `*.md`，也不含 `SKILL.md`）
pub const TRASH_DIR: &str = ".trash";
/// 官方对 `<dshHome>/skills` 施加的忽略项（`skipSystem: true`）
const SYSTEM_DIR: &str = ".system";

/// 面向 UI 的技能可用状态（严格的只读投影）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillState {
    /// 无该键 → 官方默认允许模型调用
    Enabled,
    /// 显式停用（`disable-model-invocation: true`）
    Disabled,
    /// 键重复或值非规范布尔 → 只读，拒绝操作
    Conflict,
    /// 无 frontmatter / 未闭合 / 缺 name 或 description / 读失败 → 只读
    Unreadable,
}

impl SkillState {
    /// 是否允许对该技能执行写操作（开关）
    pub fn writable(self) -> bool {
        matches!(self, SkillState::Enabled | SkillState::Disabled)
    }
}

/// 一个受管技能根
#[derive(Debug, Clone)]
pub struct SkillRoot {
    /// 根目录绝对路径
    pub path: PathBuf,
    /// 根标识：`user-dsh` / `user-agents`
    pub source: &'static str,
    /// 官方 rank
    pub rank: u32,
    /// 是否跳过 `.system`（官方对 rank 400 施加）
    pub skip_system: bool,
}

/// 受管根列表。只返回官方**用户级**两处；项目根取决于会话工作区、custom 根
/// 由 preset 声明，二者对启动器均不可知或不可见，故按 ADR-0007 D5 排除。
pub fn managed_roots() -> Vec<SkillRoot> {
    vec![
        SkillRoot {
            path: dshhome::dsh_home_skills_dir(),
            source: "user-dsh",
            rank: RANK_USER_DSH,
            skip_system: true,
        },
        SkillRoot {
            path: dshhome::agents_skills_dir(),
            source: "user-agents",
            rank: RANK_USER_AGENTS,
            skip_system: false,
        },
    ]
}

/// 一条技能记录（面向 UI）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillEntry {
    /// 绝对路径：**身份键**（ADR-0007 D10）。目录包技能指向其 `SKILL.md`。
    pub path: String,
    /// frontmatter 的 `name`（解析失败时为目录/文件名）
    pub name: String,
    /// frontmatter 的 `description`
    pub description: String,
    /// frontmatter 的 `whenToUse`
    pub when_to_use: Option<String>,
    /// 可用状态
    pub state: SkillState,
    /// 根标识
    pub source: String,
    /// 官方 rank
    pub rank: u32,
    /// 目录包（`<name>/SKILL.md`）还是平铺（`<name>.md`）
    pub bundled: bool,
    /// 不可用时的具名原因
    pub reason: Option<String>,
    /// 被同名更高 rank 技能覆盖时，覆盖者的名字
    pub overridden_by: Option<String>,
    /// 是否是符号链接（链接技能禁止删除，见 ADR-0007 D11）
    pub is_symlink: bool,
    /// 所在根是否存在
    pub root_exists: bool,
}

/// 扫描失败（仅根级 IO 故障；单个技能的解析失败在条目内表达）
#[derive(Debug)]
pub enum SkillError {
    /// 路径非法（越出受管根、不存在、不是技能）
    NotManaged(String),
    /// 文件未变化，无需写入（内部信号，不面向用户）
    Unchanged,
    /// frontmatter 歧义（具名原因）
    Frontmatter(frontmatter::WriteError),
    /// 其他 IO/内部错误
    Io(String),
}

impl SkillError {
    /// 面向用户的中文说明
    pub fn message(&self) -> String {
        match self {
            SkillError::NotManaged(detail) => format!("技能已变化，请刷新：{detail}"),
            SkillError::Unchanged => "无需变更".to_string(),
            SkillError::Frontmatter(e) => e.message().to_string(),
            SkillError::Io(detail) => detail.clone(),
        }
    }
}

/// 解析 frontmatter 中的顶层字符串字段（只读，宽容）。
///
/// 支持三种真实形态：裸标量、单/双引号标量、块标量（`|`/`>`/`>-`/`|-`）与纯多行标量
/// （`key:` 空值 + 缩进续行）。真实数据里这些形态都存在（本机 93 个技能中 3 个块标量、
/// 1 个纯多行标量），故只读解析必须能读懂它们。
fn read_string_field(text: &str, key: &str) -> Option<String> {
    let lines = split_lines(text);
    // 跳过首行 ---
    let mut i = 1usize;
    while i < lines.len() {
        let line = lines[i];
        if line == "---" {
            break;
        }
        if let Some((k, raw)) = super_top_level_key(line) {
            if k == key {
                let value = raw.trim();
                // 块标量或纯多行标量：`>`/`|`/空值 → 收集后续缩进行
                if value.is_empty() || value.starts_with('>') || value.starts_with('|') {
                    let mut collected: Vec<&str> = Vec::new();
                    let mut j = i + 1;
                    while j < lines.len() {
                        let next = lines[j];
                        if next == "---" {
                            break;
                        }
                        if next.is_empty() {
                            j += 1;
                            continue;
                        }
                        if next.starts_with(' ') || next.starts_with('\t') {
                            collected.push(next.trim());
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    let joined = collected.join(" ");
                    if joined.is_empty() {
                        return None;
                    }
                    return Some(joined);
                }
                return Some(unquote(value));
            }
        }
        i += 1;
    }
    None
}

/// 从技能文本中读取顶层字符串字段（复验用；`name`/`description` 的存在性检查）。
///
/// 复验只需要「字段仍存在」，故直接复用只读解析器。
pub fn resolve_managed_field(text: &str, key: &str) -> Option<String> {
    read_string_field(text, key)
}

/// 把 frontmatter 的 `disable-model-invocation` 状态映射为稳定的短标识。
///
/// 供集成测试与诊断使用：`"unset"` / `"enabled"` / `"disabled"` / `"conflict"` / `"unreadable"`。
pub fn frontmatter_state(text: &str) -> &'static str {
    match frontmatter::disable_state(text) {
        DisableState::Unset => "unset",
        DisableState::Enabled => "enabled",
        DisableState::Disabled => "disabled",
        DisableState::Conflict => "conflict",
        DisableState::Unreadable => "unreadable",
    }
}

/// 切行（不含行尾符）。与 `frontmatter` 模块同一套语义：只认 `\n`，剥一个尾随 `\r`。
fn split_lines(text: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for raw in text.split('\n') {
        out.push(raw.strip_suffix('\r').unwrap_or(raw));
    }
    // split('\n') 在末尾总会给出一个空串；若原文本以 \n 结尾则去掉该伪行
    if text.ends_with('\n') {
        out.pop();
    }
    out
}

/// 顶层（第 0 列）`key: value`
fn super_top_level_key(line: &str) -> Option<(&str, &str)> {
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty() || key.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    Some((key, &line[colon + 1..]))
}

/// 去掉一对匹配的单/双引号（保留内部转义原样，只读展示用）
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// 校验技能名是否符合官方 `isSkillName`：`^[a-z0-9]+(?:-[a-z0-9]+)*$`（无长度上限）
pub fn is_valid_skill_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let mut prev_dash = true; // 首字符不得为 '-'
    for ch in name.chars() {
        match ch {
            'a'..='z' | '0'..='9' => prev_dash = false,
            '-' => {
                if prev_dash {
                    return false; // 连续 '-' 或以 '-' 开头
                }
                prev_dash = true;
            }
            _ => return false, // 大写/下划线/点等一律非法
        }
    }
    !prev_dash // 不得以 '-' 结尾
}

/// 判断路径是否为符号链接（Windows 上用 `symlink_metadata` 的 file_type）
fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// 解析单条技能（宽容：失败以状态与原因表达，不返回 Err）
fn parse_entry(
    skill_path: &Path,
    dir_path: &Path,
    bundled: bool,
    root: &SkillRoot,
) -> SkillEntry {
    let display_name = if bundled {
        dir_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    } else {
        skill_path
            .file_stem()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    };

    let symlinked = is_symlink(dir_path);

    let Ok(text) = std::fs::read_to_string(skill_path) else {
        return SkillEntry {
            path: skill_path.display().to_string(),
            name: display_name,
            description: String::new(),
            when_to_use: None,
            state: SkillState::Unreadable,
            source: root.source.to_string(),
            rank: root.rank,
            bundled,
            reason: Some("无法读取 SKILL.md".to_string()),
            overridden_by: None,
            is_symlink: symlinked,
            root_exists: true,
        };
    };

    let name = read_string_field(&text, "name");
    let description = read_string_field(&text, "description");
    let when_to_use = read_string_field(&text, "whenToUse");

    let (state, reason) = match (&name, &description) {
        (Some(_), Some(_)) => match frontmatter::disable_state(&text) {
            DisableState::Unset | DisableState::Enabled => (SkillState::Enabled, None),
            DisableState::Disabled => (SkillState::Disabled, None),
            DisableState::Conflict => (
                SkillState::Conflict,
                Some("disable-model-invocation 重复或值非规范 true/false".to_string()),
            ),
            DisableState::Unreadable => (
                SkillState::Unreadable,
                Some("frontmatter 缺失或未闭合".to_string()),
            ),
        },
        (None, _) => (
            SkillState::Unreadable,
            Some("frontmatter 缺少必填字段 name".to_string()),
        ),
        (_, None) => (
            SkillState::Unreadable,
            Some("frontmatter 缺少必填字段 description".to_string()),
        ),
    };

    SkillEntry {
        path: skill_path.display().to_string(),
        name: name.unwrap_or(display_name),
        description: description.unwrap_or_default(),
        when_to_use,
        state,
        source: root.source.to_string(),
        rank: root.rank,
        bundled,
        reason,
        overridden_by: None,
        is_symlink: symlinked,
        root_exists: true,
    }
}

/// 扫描单个根
fn scan_root(root: &SkillRoot, out: &mut Vec<SkillEntry>) {
    let Ok(entries) = std::fs::read_dir(&root.path) else {
        return; // 根不存在或不可读 → 视为空（UI 侧据 roots 状态提示）
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 回收站与（rank 400 的）.system 永不作为技能
        if name == TRASH_DIR {
            continue;
        }
        if root.skip_system && name == SYSTEM_DIR {
            continue;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        // 目录包：`<name>/SKILL.md`
        if file_type.is_dir() || is_symlink(&path) {
            let skill_file = path.join(SKILL_FILE);
            if skill_file.is_file() {
                out.push(parse_entry(&skill_file, &path, true, root));
            }
            continue;
        }
        // 平铺文件：`<name>.md`
        if file_type.is_file() && name.ends_with(".md") {
            out.push(parse_entry(&path, &root.path, false, root));
        }
    }
}

/// 扫描结果：条目 + 根状态（供 UI 显示「本机无用户级技能根」）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillList {
    /// 全部受管技能（含被覆盖者，按 rank 降序 + 名称升序）
    pub skills: Vec<SkillEntry>,
    /// 受管根本身的状态
    pub roots: Vec<RootStatus>,
    /// 已停用数量
    pub disabled_count: usize,
    /// 冲突数量
    pub conflict_count: usize,
    /// 不可解析数量
    pub unreadable_count: usize,
    /// 回收站占用（条目数）
    pub trash_count: usize,
    /// 备份根目录
    pub backup_root: String,
}

/// 根状态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RootStatus {
    /// 根绝对路径
    pub path: String,
    /// 根标识
    pub source: String,
    /// 官方 rank
    pub rank: u32,
    /// 根目录是否存在
    pub exists: bool,
    /// 该根下的技能数
    pub skill_count: usize,
}

/// 列出全部受管技能（**只读**，与 dsh 运行状态无关）
pub fn list() -> SkillList {
    let roots = managed_roots();
    let mut skills: Vec<SkillEntry> = Vec::new();
    let mut root_status: Vec<RootStatus> = Vec::new();

    for root in &roots {
        let before = skills.len();
        scan_root(root, &mut skills);
        root_status.push(RootStatus {
            path: root.path.display().to_string(),
            source: root.source.to_string(),
            rank: root.rank,
            exists: root.path.is_dir(),
            skill_count: skills.len() - before,
        });
    }

    // 同名覆盖标注：按 rank **降序**排序后，首次出现的 name 为生效者，其余标记被覆盖
    skills.sort_by(|a, b| {
        b.rank
            .cmp(&a.rank)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut winner: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for skill in skills.iter_mut() {
        match winner.get(&skill.name) {
            // 已有更高 rank 的同名者 → 本条被覆盖
            Some(name) => skill.overridden_by = Some(name.clone()),
            // 首次出现 → 本条生效，登记供后续比对
            None => {
                winner.insert(skill.name.clone(), skill.name.clone());
            }
        }
    }

    let disabled_count = skills
        .iter()
        .filter(|s| s.state == SkillState::Disabled)
        .count();
    let conflict_count = skills
        .iter()
        .filter(|s| s.state == SkillState::Conflict)
        .count();
    let unreadable_count = skills
        .iter()
        .filter(|s| s.state == SkillState::Unreadable)
        .count();
    let trash_count = roots
        .iter()
        .map(|r| r.path.join(TRASH_DIR))
        .filter(|p| p.is_dir())
        .map(|p| {
            std::fs::read_dir(&p)
                .map(|it| it.flatten().count())
                .unwrap_or(0)
        })
        .sum();

    SkillList {
        skills,
        roots: root_status,
        disabled_count,
        conflict_count,
        unreadable_count,
        trash_count,
        backup_root: crate::core::skill::manage::backup_root()
            .display()
            .to_string(),
    }
}

/// 在受管根内重新解析某个绝对路径，返回其 frontmatter `name`（供写前身份校验）。
///
/// 返回 `Ok(Some(name))` 表示该路径仍是受管技能且 name 已知；`Ok(None)` 表示路径
/// 不在受管根内或已不存在。
pub fn resolve_managed(skill_path: &Path) -> Option<String> {
    let canonical_target = skill_path;
    for root in managed_roots() {
        let root_canonical = std::fs::canonicalize(&root.path).ok();
        // 路径必须在受管根之下
        let inside = match (&root_canonical, canonical_target.canonicalize().ok()) {
            (Some(root_real), Some(target_real)) => target_real.starts_with(root_real),
            _ => false,
        };
        if !inside {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(canonical_target) else {
            return None;
        };
        return read_string_field(&text, "name");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 技能名校验与官方一致() {
        for ok in ["a", "research", "tauri-app-autostart", "gpui-kit", "x1", "a-1-b"] {
            assert!(is_valid_skill_name(ok), "{ok} 应合法");
        }
        for bad in [
            "", "Research", "my_skill", "my.skill", "-lead", "trail-", "a--b", "a b", "技能",
        ] {
            assert!(!is_valid_skill_name(bad), "{bad} 应非法");
        }
    }

    #[test]
    fn 切行剥掉_crlf() {
        let lines = split_lines("---\r\nname: x\r\n---\r\n");
        assert_eq!(lines, vec!["---", "name: x", "---"]);
    }

    #[test]
    fn 读取裸标量与引号标量() {
        let text = "---\nname: research\ndescription: \"a: b\"\n---\n";
        assert_eq!(read_string_field(text, "name").unwrap(), "research");
        assert_eq!(read_string_field(text, "description").unwrap(), "a: b");
    }

    #[test]
    fn 读取块标量() {
        let text = "---\nname: gpui-bench\ndescription: >-\n  Line one.\n  Line two.\n---\n";
        assert_eq!(
            read_string_field(text, "description").unwrap(),
            "Line one. Line two."
        );
    }

    #[test]
    fn 读取纯多行标量() {
        let text = "---\nname: x\ndescription:\n  First.\n  Second.\n---\n";
        assert_eq!(
            read_string_field(text, "description").unwrap(),
            "First. Second."
        );
    }

    #[test]
    fn 嵌套映射的同名键不被当作顶层() {
        let text = "---\nname: rust-skills\nmetadata:\n  name: nested\n---\n";
        assert_eq!(read_string_field(text, "name").unwrap(), "rust-skills");
    }
}
