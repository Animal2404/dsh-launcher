//! 技能导入（ADR-0008）：从任意 git 仓库**递归收集 + 扁平化**到用户级技能根
//!
//! ## 为什么必须扁平化，而不是「克隆即用」
//!
//! 官方发现规则**只认扫描根顶层的一层**：`<root>/<name>/SKILL.md` 或 `<root>/<name>.md`；
//! README 原文「nested `**/SKILL.md` files are **deliberately not discovered**」、
//! 「Discovery is **one level deep**」（`packages/skill/skill-filesystem/README.md:36,146`）。
//!
//! 而真实技能仓库普遍是**分类嵌套**的 —— 例如 `mattpocock/skills` 的布局是
//! `skills/<分类>/<name>/SKILL.md`（两层）。把这样的仓库直接克隆到技能根，顶层只有
//! `engineering/`、`productivity/` 两个目录，两者都不含 `SKILL.md` → **零个可发现技能**。
//!
//! 因此导入 = 浅克隆到临时目录 → 递归找 `SKILL.md` → 逐个校验 → 复制到 `<root>/<name>/`。
//!
//! ## 目录名取 frontmatter 的 `name`，不取仓库里的目录名
//!
//! 实测本机 93 个技能中有 2 个目录名与 `name` 不符
//! （`composition-patterns` → `vercel-composition-patterns`）；
//! 照抄仓库目录名会让「磁盘上的名字」与「官方解析出的名字」不一致，产生歧义。
//!
//! ## 覆盖策略：**文件级**（Q29 A）
//!
//! 上游提供的文件覆盖；**上游没有的本地文件/目录原样保留**（例如 30 个技能里的
//! `agents/` 子目录、70 个技能里的兄弟 `.md` —— 这些是**本地产物，上游并不存在**）。
//! 若改为「目录级替换」，这些本地内容会被**静默删除**。远程删除的文件也**不落地删除**
//! （删除是破坏性动作，只提示不执行）。

use crate::core::logging::Logger;
use crate::core::skill::frontmatter;
use crate::core::skill::scan;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 单个技能文件在导入/更新时的差异类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileChange {
    /// 本地没有，将新增
    Added,
    /// 两边都有但内容不同，将覆盖
    Updated,
    /// 内容一致，无需动作
    Same,
    /// **仅本地有**，上游没有 → 保留（绝不删除）
    LocalOnly,
    /// 仅上游有但本地已存在同名技能目录，且该文件不属于上游 → 不适用（保留给 LocalOnly）
    Removed,
}

/// 单个技能的导入计划
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillImportPlan {
    /// frontmatter 的 name（也是目标目录名）
    pub name: String,
    /// 仓库内的来源相对路径（诊断用）
    pub repo_path: String,
    /// 目标绝对路径
    pub target_path: String,
    /// 是否需要写盘（存在 Added/Updated）
    pub actionable: bool,
    /// 新增文件数
    pub added: usize,
    /// 覆盖文件数
    pub updated: usize,
    /// 未变化文件数
    pub same: usize,
    /// 仅本地有的文件数（保留）
    pub local_only: usize,
    /// 上游已删除、但本地仍保留的文件数（提示不删除）
    pub removed_upstream: usize,
    /// 逐文件明细（相对路径 + 差异类型）
    pub files: Vec<FileDiff>,
    /// 跳过原因（无 name/description、名字非法、同名冲突等）
    pub skip_reason: Option<String>,
}

/// 单文件差异
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    /// 相对技能目录的路径
    pub path: String,
    pub change: FileChange,
}

/// 导入/检查结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    /// 来源 URL（用户输入原样）
    pub url: String,
    /// 克隆到的 commit sha
    pub commit: Option<String>,
    /// 逐技能计划
    pub plans: Vec<SkillImportPlan>,
    /// 是否真的写盘（检查模式恒 false —— **永不自动写盘**）
    pub applied: bool,
    /// 面向用户的摘要
    pub message: String,
}

/// 导入失败
#[derive(Debug)]
pub enum ImportError {
    /// git 不可用
    GitMissing(String),
    /// 克隆失败
    CloneFailed(String),
    /// 仓库里一个技能都没找到
    NoSkills(String),
    /// 目标根不可用
    TargetUnavailable(String),
    /// IO 错误
    Io(String),
}

impl ImportError {
    /// 面向用户的中文说明
    pub fn message(&self) -> String {
        match self {
            ImportError::GitMissing(d) => format!("未找到 git：{d}"),
            ImportError::CloneFailed(d) => format!("克隆仓库失败：{d}"),
            ImportError::NoSkills(d) => format!("该仓库中没有找到任何技能：{d}"),
            ImportError::TargetUnavailable(d) => format!("目标技能根不可用：{d}"),
            ImportError::Io(d) => d.clone(),
        }
    }
}

/// 导入目标根：用户级 rank 500（`<agentsHome>/skills`）。
///
/// 选择 rank 500 而非 400 的理由：官方对 `<dshHome>/skills` 施加 `skipSystem`，
/// 且 500 是原生覆盖的原生根；导入的东西放这里语义最正。
pub fn target_root() -> PathBuf {
    crate::core::dshhome::agents_skills_dir()
}

/// 临时克隆目录：`%LOCALAPPDATA%\dsh-launcher\skill-import\<时间戳>`
fn temp_clone_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("dsh-launcher")
        .join("skill-import")
        .join(nanos.to_string())
}

/// 浅克隆仓库到临时目录，返回 (目录, commit sha)。
///
/// 克隆目录由调用方负责清理（正常路径在 `import_from_url` 内清理）。
pub fn clone_repo(
    url: &str,
    logger: &Arc<Logger>,
) -> Result<(PathBuf, Option<String>), ImportError> {
    let url = url.trim();
    if url.is_empty() {
        return Err(ImportError::CloneFailed("URL 为空".to_string()));
    }
    // 只接受 http(s) / git / ssh / 本地路径形态，挡掉明显非 URL 的输入
    let looks_like_repo = url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("git@")
        || url.starts_with("ssh://")
        || url.starts_with("git://")
        || url.starts_with("file://")
        || url.starts_with('/')
        || url.starts_with("\\\\")
        || (url.len() > 2 && url.as_bytes()[1] == b':');
    if !looks_like_repo {
        return Err(ImportError::CloneFailed(format!(
            "「{url}」不像仓库地址（需 http(s)://、git@、ssh:// 或本地路径）"
        )));
    }

    let dest = temp_clone_dir();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ImportError::Io(format!("创建临时目录失败: {e}")))?;
    }

    logger.info(&format!("开始克隆技能仓库 {url} …"));
    let cmd = crate::core::github::git_clone_command(url, None, &dest)
        .map_err(ImportError::GitMissing)?;
    let output = crate::core::command::run_with_timeout(cmd, std::time::Duration::from_secs(180))
        .map_err(|e| ImportError::CloneFailed(format!("{e}")))?;
    if !output.status.success() {
        let stderr = crate::core::text::decode(&output.stderr);
        let tail: String = stderr.lines().rev().take(6).collect::<Vec<_>>().join("\n");
        let _ = std::fs::remove_dir_all(&dest);
        return Err(ImportError::CloneFailed(if tail.trim().is_empty() {
            format!("git clone 退出码 {:?}", output.status.code())
        } else {
            tail
        }));
    }

    // 取 commit sha（克隆目录内的 HEAD）
    let sha = read_head_commit(&dest);
    logger.info(&format!(
        "克隆完成（commit {}）",
        sha.clone().unwrap_or_else(|| "未知".to_string())
    ));
    Ok((dest, sha))
}

/// 读取克隆目录的 HEAD commit sha
fn read_head_commit(repo_dir: &Path) -> Option<String> {
    let mut cmd = crate::core::command::hidden("git");
    cmd.arg("-C").arg(repo_dir).arg("rev-parse").arg("HEAD");
    let out = crate::core::command::run_with_timeout(cmd, std::time::Duration::from_secs(30)).ok()?;
    if !out.status.success() {
        return None;
    }
    let text = crate::core::text::decode(&out.stdout);
    let sha = text.trim().to_string();
    if sha.is_empty() {
        None
    } else {
        Some(sha)
    }
}

/// 递归收集仓库内所有技能（含 `SKILL.md` 的目录），排除 `.git`。
///
/// 递归是刻意的：真实仓库用分类目录嵌套（`skills/engineering/<name>/SKILL.md`），
/// 而官方只发现一层，故必须由导入器把嵌套结构压平。
fn collect_skills(repo_dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![repo_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // .git 与常见产物目录不参与扫描
            if name == ".git" || name == "node_modules" {
                continue;
            }
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                if path.join("SKILL.md").is_file() {
                    found.push(path.clone());
                    // 技能目录内部不再继续下钻（避免把技能的 resources 当技能）
                    continue;
                }
                stack.push(path);
            }
        }
    }
    found.sort();
    found
}

/// 解析一个仓库内技能目录的 frontmatter，返回 (name, description)。
fn read_identity(skill_dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(skill_dir.join("SKILL.md")).ok()?;
    let name = scan::resolve_managed_field(&text, "name")?;
    let description = scan::resolve_managed_field(&text, "description")?;
    Some((name, description))
}

/// 递归列出目录下所有文件（相对路径，使用 `/` 分隔以跨平台一致）
fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                if let Ok(rel) = path.strip_prefix(root) {
                    out.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    out.sort();
    out
}

/// 比较单个技能：源目录 vs 目标目录，产出逐文件差异（Q29 A 文件级语义）
fn plan_one(skill_dir: &Path, repo_dir: &Path, target_root: &Path) -> SkillImportPlan {
    let repo_path = skill_dir
        .strip_prefix(repo_dir)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    let identity = read_identity(skill_dir);
    let (name, _description) = match &identity {
        Some((n, d)) => (n.clone(), d.clone()),
        None => {
            return SkillImportPlan {
                name: skill_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                repo_path,
                target_path: String::new(),
                actionable: false,
                added: 0,
                updated: 0,
                same: 0,
                local_only: 0,
                removed_upstream: 0,
                files: Vec::new(),
                skip_reason: Some(
                    "缺少可解析的 frontmatter name/description（官方会丢弃该技能）".to_string(),
                ),
            };
        }
    };

    // 官方 `isSkillName` 校验：不符合官方字符集的技能官方根本不会加载
    if !scan::is_valid_skill_name(&name) {
        return SkillImportPlan {
            name,
            repo_path,
            target_path: String::new(),
            actionable: false,
            added: 0,
            updated: 0,
            same: 0,
            local_only: 0,
            removed_upstream: 0,
            files: Vec::new(),
            skip_reason: Some(
                "技能名不符合官方规则 ^[a-z0-9]+(?:-[a-z0-9]+)*$（仅小写字母、数字、连字符）"
                    .to_string(),
            ),
        };
    }

    let target = target_root.join(&name);
    let src_files = list_files(skill_dir);
    let mut diffs: Vec<FileDiff> = Vec::new();
    let mut added = 0usize;
    let mut updated = 0usize;
    let mut same = 0usize;

    let src_set: BTreeSet<&String> = src_files.iter().collect();
    for rel in &src_files {
        let src = skill_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let dst = target.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        let change = if !dst.exists() {
            added += 1;
            FileChange::Added
        } else if files_equal(&src, &dst) {
            same += 1;
            FileChange::Same
        } else {
            updated += 1;
            FileChange::Updated
        };
        diffs.push(FileDiff {
            path: rel.clone(),
            change,
        });
    }

    // 本地独有文件（上游没有）→ 保留，绝不删除
    let mut local_only = 0usize;
    if target.is_dir() {
        for rel in list_files(&target) {
            if !src_set.contains(&rel) {
                local_only += 1;
                diffs.push(FileDiff {
                    path: rel,
                    change: FileChange::LocalOnly,
                });
            }
        }
    }

    diffs.sort_by(|a, b| a.path.cmp(&b.path));
    let actionable = added > 0 || updated > 0;
    SkillImportPlan {
        name,
        repo_path,
        target_path: target.display().to_string(),
        actionable,
        added,
        updated,
        same,
        local_only,
        // 上游删除而本地保留的文件数 == 本地独有（语义上无法区分「本地新增」与
        // 「上游曾提供后删除」，故统一按「保留」处理并在此计数提示）
        removed_upstream: 0,
        files: diffs,
        skip_reason: None,
    }
}

/// 比较两个文件内容是否一致（字节级）
fn files_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 在已克隆目录上计算计划（便于单测，不触发网络）
pub fn plan_from_clone(
    repo_dir: &Path,
    url: &str,
    commit: Option<String>,
) -> Result<ImportReport, ImportError> {
    let target_root = target_root();
    std::fs::create_dir_all(&target_root).map_err(|e| {
        ImportError::TargetUnavailable(format!("{}（{e}）", target_root.display()))
    })?;

    let skill_dirs = collect_skills(repo_dir);
    if skill_dirs.is_empty() {
        return Err(ImportError::NoSkills(format!(
            "在 {} 下未找到任何含 SKILL.md 的目录。注意官方只发现扫描根顶层的 \
             <名称>/SKILL.md，故仓库需含此类结构",
            repo_dir.display()
        )));
    }

    let plans: Vec<SkillImportPlan> = skill_dirs
        .iter()
        .map(|dir| plan_one(dir, repo_dir, &target_root))
        .collect();

    let importable = plans.iter().filter(|p| p.skip_reason.is_none()).count();
    let skipped = plans.len() - importable;
    let to_write = plans.iter().filter(|p| p.actionable).count();
    let message = format!(
        "共发现 {importable} 个技能（跳过 {skipped} 个）；{to_write} 个需要写入"
    );

    Ok(ImportReport {
        url: url.to_string(),
        commit,
        plans,
        applied: false,
        message,
    })
}

/// 应用导入计划（**写盘**）：按文件级覆盖复制，保留本地独有文件
///
/// `name` 是用户在批量导入时填写的仓库标签，落进来源记录供 UI 展示；
/// 单 URL 导入时为 `None`。
pub fn apply_plan(
    repo_dir: &Path,
    mut report: ImportReport,
    name: Option<&str>,
    logger: &Arc<Logger>,
) -> Result<ImportReport, ImportError> {
    let mut written = 0usize;
    for plan in report.plans.iter_mut() {
        if plan.skip_reason.is_some() || !plan.actionable {
            continue;
        }
        let src_dir = repo_dir.join(plan.repo_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let dst_dir = PathBuf::from(&plan.target_path);
        std::fs::create_dir_all(&dst_dir)
            .map_err(|e| ImportError::Io(format!("创建 {} 失败: {e}", dst_dir.display())))?;

        for diff in plan.files.iter() {
            // 只处理上游提供的文件；LocalOnly 一律跳过（保留本地）
            match diff.change {
                FileChange::Added | FileChange::Updated => {
                    let src = src_dir.join(diff.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    let dst = dst_dir.join(diff.path.replace('/', std::path::MAIN_SEPARATOR_STR));
                    if let Some(parent) = dst.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| {
                            ImportError::Io(format!("创建 {} 失败: {e}", parent.display()))
                        })?;
                    }
                    std::fs::copy(&src, &dst).map_err(|e| {
                        ImportError::Io(format!(
                            "复制 {} → {} 失败: {e}",
                            src.display(),
                            dst.display()
                        ))
                    })?;
                    written += 1;
                }
                FileChange::Same | FileChange::LocalOnly | FileChange::Removed => {}
            }
        }
        logger.info(&format!(
            "技能 {} 已导入到 {}（新增 {}，覆盖 {}，保留本地 {}）",
            plan.name,
            plan.target_path,
            plan.added,
            plan.updated,
            plan.local_only
        ));
    }

    // 记录来源（手工维护的元数据；不驱动任何自动更新）
    if let Some(commit) = report.commit.clone() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let skills: Vec<String> = report
            .plans
            .iter()
            .filter(|p| p.skip_reason.is_none())
            .map(|p| p.name.clone())
            .collect();
        let mut registry = crate::core::skill::source::SourceRegistry::load();
        registry.upsert(crate::core::skill::source::SourceRecord {
            name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()),
            url: report.url.clone(),
            commit: Some(commit),
            imported_at: now,
            checked_at: None,
            last_update_count: 0,
            skills,
        });
        if let Err(error) = registry.save() {
            // 注册表写失败不应让导入整体失败：技能文件已经落地，元数据可后补
            logger.warn(&format!("技能来源注册表保存失败（不影响已导入文件）: {error}"));
        }
    }

    report.applied = true;
    report.message = format!("导入完成，共写入 {written} 个文件");
    // 写盘后释放克隆目录由调用方负责
    Ok(report)
}

/// 校验技能文本的 frontmatter 是否可安全参与后续启停（供导入前预检）
pub fn precheck_frontmatter(text: &str) -> Result<(), frontmatter::WriteError> {
    frontmatter::set_disable_model_invocation(text, true).map(|_| ())
}

/// 顶层编排：克隆 → 计划 →（可选）应用 → 清理。
///
/// `apply == false` 时是**纯只读检查**（用于「检查更新」与导入前预览），
/// **绝不写盘**；`apply == true` 时才按文件级覆盖写入。
///
/// `name` 是仓库标签（批量导入时填写），`None` 表示单 URL 导入、无标签。
///
/// 克隆目录在两种路径下都被清理（含失败路径），不会残留。
pub fn import_from_url(
    url: &str,
    name: Option<&str>,
    apply: bool,
    logger: &Arc<Logger>,
) -> Result<ImportReport, ImportError> {
    let (repo_dir, commit) = clone_repo(url, logger)?;
    let planned = plan_from_clone(&repo_dir, url, commit);
    let result = match planned {
        Ok(report) if apply => apply_plan(&repo_dir, report, name, logger),
        other => other,
    };
    // 无论成功失败都清理临时克隆目录
    if let Err(error) = std::fs::remove_dir_all(&repo_dir) {
        logger.warn(&format!(
            "临时克隆目录清理失败（可手工删除）{}: {error}",
            repo_dir.display()
        ));
    }
    result
}

/// 批量导入的一条待导入条目（仓库标签 + URL）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    /// 仓库标签（友好名，可为空）
    #[serde(default)]
    pub name: String,
    /// 仓库 URL
    pub url: String,
}

/// 批量导入的单条结果
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchItemResult {
    /// 仓库标签
    pub name: String,
    /// 仓库 URL
    pub url: String,
    /// 该条是否成功（Err 时 message 为错误原因）
    pub ok: bool,
    /// 结果说明
    pub message: String,
    /// 成功时的计划（只读预览模式有值）
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<SkillImportPlan>,
    /// 成功时的 commit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
}

/// 批量导入聚合报告
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportReport {
    /// 逐条结果（顺序与输入一致）
    pub items: Vec<BatchItemResult>,
    /// 是否真的写盘（apply=true）
    pub applied: bool,
    /// 成功条数
    pub ok_count: usize,
    /// 失败条数
    pub failed_count: usize,
    /// 面向用户的摘要
    pub message: String,
}

/// 批量导入（逐条克隆 → 计划 → 可选应用）。**单条失败不中断其余条目**。
pub fn import_batch(
    items: &[ImportItem],
    apply: bool,
    logger: &Arc<Logger>,
) -> BatchImportReport {
    let mut results = Vec::with_capacity(items.len());
    let mut ok = 0usize;
    let mut failed = 0usize;

    for item in items {
        let url = item.url.trim();
        if url.is_empty() {
            results.push(BatchItemResult {
                name: item.name.clone(),
                url: item.url.clone(),
                ok: false,
                message: "URL 为空，已跳过".to_string(),
                plans: Vec::new(),
                commit: None,
            });
            failed += 1;
            continue;
        }
        let name = if item.name.trim().is_empty() {
            None
        } else {
            Some(item.name.as_str())
        };
        match import_from_url(url, name, apply, logger) {
            Ok(report) => {
                results.push(BatchItemResult {
                    name: name.unwrap_or(url).to_string(),
                    url: url.to_string(),
                    ok: true,
                    message: report.message.clone(),
                    plans: report.plans.clone(),
                    commit: report.commit.clone(),
                });
                ok += 1;
            }
            Err(error) => {
                results.push(BatchItemResult {
                    name: name.unwrap_or(url).to_string(),
                    url: url.to_string(),
                    ok: false,
                    message: error.message(),
                    plans: Vec::new(),
                    commit: None,
                });
                failed += 1;
            }
        }
    }

    let message = if apply {
        format!("批量导入完成：成功 {ok} 个，失败 {failed} 个")
    } else {
        format!("批量预览完成：成功 {ok} 个，失败 {failed} 个")
    };
    BatchImportReport {
        items: results,
        applied: apply,
        ok_count: ok,
        failed_count: failed,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录里造一个「分类嵌套」的仓库（模拟 mattpocock/skills 的真实布局）
    fn make_repo(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("dsh-skill-repo-{tag}-{nanos}"));
        let _ = std::fs::remove_dir_all(&root);
        // skills/engineering/alpha/SKILL.md（两层嵌套）
        let alpha = root.join("skills/engineering/alpha");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::write(
            alpha.join("SKILL.md"),
            "---\nname: alpha\ndescription: Alpha skill.\n---\n\nBody.\n",
        )
        .unwrap();
        std::fs::write(alpha.join("helper.md"), "helper content").unwrap();
        // skills/productivity/beta/SKILL.md
        let beta = root.join("skills/productivity/beta");
        std::fs::create_dir_all(&beta).unwrap();
        std::fs::write(
            beta.join("SKILL.md"),
            "---\nname: beta\ndescription: Beta skill.\n---\n\nBody.\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn 递归收集能穿透分类嵌套() {
        let repo = make_repo("collect");
        let found = collect_skills(&repo);
        assert_eq!(found.len(), 2, "两层嵌套应被收集：{found:?}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 递归收集不把技能内部资源当技能() {
        let repo = make_repo("nested-res");
        // 在技能目录里再放一个含 SKILL.md 的子目录：不应被当作独立技能
        let inner = repo.join("skills/engineering/alpha/resources/inner");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(
            inner.join("SKILL.md"),
            "---\nname: inner\ndescription: Inner.\n---\n",
        )
        .unwrap();
        let found = collect_skills(&repo);
        assert_eq!(found.len(), 2, "技能内部不应继续下钻：{found:?}");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 收集时跳过_git_目录() {
        let repo = make_repo("skip-git");
        let git_skill = repo.join(".git/hooks/fake");
        std::fs::create_dir_all(&git_skill).unwrap();
        std::fs::write(
            git_skill.join("SKILL.md"),
            "---\nname: fake\ndescription: Fake.\n---\n",
        )
        .unwrap();
        let found = collect_skills(&repo);
        assert!(
            found.iter().all(|p| !p.to_string_lossy().contains(".git")),
            "不应扫描 .git：{found:?}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 计划阶段不写盘() {
        let repo = make_repo("plan-readonly");
        let report = plan_from_clone(&repo, "https://example.invalid/x", None).unwrap();
        assert!(!report.applied, "检查模式必须 applied=false");
        assert_eq!(report.plans.len(), 2);
        // 目标根下不应出现任何技能目录（只读）
        for plan in &report.plans {
            assert!(
                !Path::new(&plan.target_path).exists(),
                "计划阶段不应创建 {}",
                plan.target_path
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 非法技能名被跳过并给出原因() {
        let repo = make_repo("badname");
        let bad = repo.join("skills/x/Bad_Name");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(
            bad.join("SKILL.md"),
            "---\nname: Bad_Name\ndescription: Bad.\n---\n",
        )
        .unwrap();
        let report = plan_from_clone(&repo, "https://example.invalid/x", None).unwrap();
        let skipped = report
            .plans
            .iter()
            .find(|p| p.name == "Bad_Name")
            .expect("应含 Bad_Name");
        assert!(skipped.skip_reason.is_some(), "非法名应有跳过原因");
        assert!(!skipped.actionable);
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 缺_description_被跳过() {
        let repo = make_repo("nodesc");
        let bad = repo.join("skills/x/nodesc");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), "---\nname: nodesc\n---\n").unwrap();
        let report = plan_from_clone(&repo, "https://example.invalid/x", None).unwrap();
        let item = report
            .plans
            .iter()
            .find(|p| p.name == "nodesc")
            .expect("应含 nodesc");
        assert!(item.skip_reason.is_some(), "缺 description 应被跳过");
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn 无技能仓库报错() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("dsh-empty-repo-{nanos}"));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("README.md"), "no skills here").unwrap();
        let err = plan_from_clone(&root, "https://example.invalid/x", None);
        assert!(matches!(err, Err(ImportError::NoSkills(_))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 非仓库地址被拒绝() {
        let log = Arc::new(Logger::init());
        let err = clone_repo("not a url at all", &log);
        assert!(matches!(err, Err(ImportError::CloneFailed(_))));
    }
}
