//! 技能「检查更新」（ADR-0008）：**纯手动、永不自动写盘**
//!
//! 设计要点（用户明确要求，见 ADR-0008 D-Update）：
//! 1. **只在用户点按钮时运行**，没有任何定时任务与启动钩子。全仓唯一既有的周期循环
//!    是 `process.rs` 的 5 秒状态对账，本功能**不**接入它。
//! 2. **永不自动写盘**：检查只产出三类清单（新增 / 更新 / 本地独有保留），
//!    应用必须由用户在界面上二次确认后才发生。
//! 3. 判定依据是**逐文件内容比较**（不是「commit 变了就覆盖」）—— 因为上游可能只改了
//!    README，而技能文件本身没动。只有内容真的不同才计入「更新」。
//!
//! 比较的基准是**远程最新内容**（浅克隆临时目录），而非本地记录的 commit：
//! 这样即使本地被手工改过，也能如实报告差异（用户改动不会被静默吞掉）。

use crate::core::logging::Logger;
use crate::core::skill::import::{
    self, FileChange, ImportError, ImportReport, SkillImportPlan,
};
use crate::core::skill::source::SourceRegistry;
use serde::Serialize;
use std::sync::Arc;

/// 一个来源的检查结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCheck {
    /// 来源 URL
    pub url: String,
    /// 记录中的 commit（上次导入时的）
    pub recorded_commit: Option<String>,
    /// 远端当前 commit
    pub remote_commit: Option<String>,
    /// commit 是否变化（仅作提示；是否真的需要更新看文件差异）
    pub commit_changed: bool,
    /// 逐技能计划（含逐文件差异）
    pub plans: Vec<SkillImportPlan>,
    /// 需要写入的技能数
    pub actionable_count: usize,
    /// 仅本地保留的文件总数
    pub local_only_count: usize,
    /// 检查失败原因（网络/仓库不可达等）
    pub error: Option<String>,
}

/// 全部来源的检查结果
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheckReport {
    /// 逐来源结果
    pub sources: Vec<SourceCheck>,
    /// 需要更新的技能总数
    pub actionable_total: usize,
    /// 记录数
    pub source_count: usize,
    /// 面向用户的摘要
    pub message: String,
}

/// 计算各来源的差异（**只读，绝不写盘**）
pub fn check_updates(logger: &Arc<Logger>) -> UpdateCheckReport {
    let registry = SourceRegistry::load();
    check_registry(&registry, logger)
}

/// 对给定注册表做检查（可单测）
pub fn check_registry(registry: &SourceRegistry, logger: &Arc<Logger>) -> UpdateCheckReport {
    let mut results: Vec<SourceCheck> = Vec::new();

    for record in &registry.sources {
        // 只读计划：clone → plan → 清理（绝不写盘）
        let outcome = import::import_from_url(&record.url, record.name.as_deref(), false, logger);
        match outcome {
            Ok(report) => {
                results.push(build_check(record, report));
            }
            Err(error) => {
                results.push(SourceCheck {
                    url: record.url.clone(),
                    recorded_commit: record.commit.clone(),
                    remote_commit: None,
                    commit_changed: false,
                    plans: Vec::new(),
                    actionable_count: 0,
                    local_only_count: 0,
                    error: Some(describe_error(&error)),
                });
            }
        }
    }

    let actionable_total: usize = results.iter().map(|r| r.actionable_count).sum();
    let failed = results.iter().filter(|r| r.error.is_some()).count();
    let message = if registry.sources.is_empty() {
        "尚无已登记来源（通过「从 URL 导入」登记后才可检查更新）".to_string()
    } else if actionable_total == 0 {
        format!(
            "已检查 {} 个来源，全部为最新{}",
            results.len(),
            if failed > 0 {
                format!("（{failed} 个来源检查失败）")
            } else {
                String::new()
            }
        )
    } else {
        format!(
            "已检查 {} 个来源，{actionable_total} 个技能有可用更新{}",
            results.len(),
            if failed > 0 {
                format!("（{failed} 个来源检查失败）")
            } else {
                String::new()
            }
        )
    };

    UpdateCheckReport {
        sources: results,
        actionable_total,
        source_count: registry.sources.len(),
        message,
    }
}

/// 由导入计划构造检查结果
fn build_check(
    record: &crate::core::skill::source::SourceRecord,
    report: ImportReport,
) -> SourceCheck {
    let actionable_count = report.plans.iter().filter(|p| p.actionable).count();
    let local_only_count = report
        .plans
        .iter()
        .map(|p| p.local_only)
        .sum::<usize>();
    let commit_changed = match (&record.commit, &report.commit) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    };
    SourceCheck {
        url: report.url,
        recorded_commit: record.commit.clone(),
        remote_commit: report.commit,
        commit_changed,
        plans: report.plans,
        actionable_count,
        local_only_count,
        error: None,
    }
}

/// 把导入错误转成面向用户的说明
fn describe_error(error: &ImportError) -> String {
    error.message()
}

/// 应用某个来源的更新（**需用户已确认**；文件级覆盖，保留本地独有文件）
pub fn apply_source_update(
    url: &str,
    logger: &Arc<Logger>,
) -> Result<ImportReport, ImportError> {
    // 更新时保留来源记录里的仓库标签
    let name = SourceRegistry::load()
        .find(url)
        .and_then(|r| r.name.clone());
    let report = import::import_from_url(url, name.as_deref(), true, logger)?;
    // 记录本次检查结果（供 UI 显示；仅元数据）
    let mut registry = SourceRegistry::load();
    if let Some(record) = registry.find(url).cloned() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs().to_string())
            .unwrap_or_else(|_| "0".to_string());
        let count = report.plans.iter().filter(|p| p.actionable).count();
        registry.upsert(crate::core::skill::source::SourceRecord {
            checked_at: Some(now),
            last_update_count: count,
            ..record
        });
        if let Err(error) = registry.save() {
            logger.warn(&format!("来源注册表更新失败（不影响已写入技能）: {error}"));
        }
    }
    Ok(report)
}

/// 摘要单条计划的文件计数（供 UI 与测试使用）
pub fn summarize_plan(plan: &SkillImportPlan) -> String {
    format!(
        "新增 {} / 覆盖 {} / 相同 {} / 保留本地 {}",
        plan.added, plan.updated, plan.same, plan.local_only
    )
}

/// 判断某差异类型是否会写盘
pub fn is_write(diff: &crate::core::skill::import::FileDiff) -> bool {
    matches!(diff.change, FileChange::Added | FileChange::Updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::skill::source::{SourceRecord, SourceRegistry};

    #[test]
    fn 空注册表的检查结果给出引导文案() {
        let log = Arc::new(Logger::init());
        let report = check_registry(&SourceRegistry::default(), &log);
        assert_eq!(report.source_count, 0);
        assert_eq!(report.actionable_total, 0);
        assert!(report.message.contains("尚无已登记来源"), "{}", report.message);
    }

    #[test]
    fn 不可达来源记录错误而不中断整体检查() {
        let log = Arc::new(Logger::init());
        let mut registry = SourceRegistry::default();
        registry.upsert(SourceRecord {
            name: None,
            url: "https://example.invalid/definitely/not/a/repo".to_string(),
            commit: Some("deadbeef".to_string()),
            imported_at: "0".to_string(),
            checked_at: None,
            last_update_count: 0,
            skills: vec!["x".to_string()],
        });
        let report = check_registry(&registry, &log);
        assert_eq!(report.source_count, 1);
        assert_eq!(report.sources.len(), 1);
        assert!(
            report.sources[0].error.is_some(),
            "不可达来源应记录错误：{:?}",
            report.sources[0]
        );
        assert_eq!(report.actionable_total, 0);
        assert!(report.message.contains("检查失败"), "{}", report.message);
    }

    #[test]
    fn 写入判定只认新增与更新() {
        use crate::core::skill::import::{FileDiff, FileChange};
        for (change, expected) in [
            (FileChange::Added, true),
            (FileChange::Updated, true),
            (FileChange::Same, false),
            (FileChange::LocalOnly, false),
            (FileChange::Removed, false),
        ] {
            let diff = FileDiff {
                path: "x".to_string(),
                change,
            };
            assert_eq!(is_write(&diff), expected, "{change:?}");
        }
    }
}
