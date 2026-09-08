//! upstream 插件同步：目标解析与计划生成
//!
//! ADR-0005 D6：
//! - 只有 `origin == Upstream` 的包进入计划；`in-house`/`unknown` 一律跳过（不静默改动）；
//! - npm 源比对 registry 最新版本；git 源比对远端引用 sha，并把 spec 钉到该 sha；
//! - 本模块只负责"解析 + 计划"，实际执行（stop → add → verify → start）在 plugin::sync。

use crate::core::command;
use crate::core::github;
use crate::core::plugin::registry::PluginRecord;
use crate::core::plugin::spec::{self, Origin, SpecKind};
use crate::core::text;
use serde::Serialize;
use std::time::Duration;

/// 网络查询超时
const REMOTE_TIMEOUT: Duration = Duration::from_secs(120);

/// 一个同步计划项
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPlanItem {
    pub package: String,
    pub origin: Origin,
    pub kind: SpecKind,
    /// 当前版本/commit
    pub current: Option<String>,
    /// 目标版本/commit
    pub target: Option<String>,
    /// 需要写入 profile 的新 spec（None 表示无需变更）
    pub new_spec: Option<String>,
    /// 是否需要变更
    pub actionable: bool,
    /// 说明（跳过原因 / 变更描述）
    pub reason: String,
}

/// 查询 npm 包的最新版本（走已配置的 registry 镜像）。
pub fn npm_latest(package: &str) -> Result<Option<String>, String> {
    let mut cmd = command::hidden_cmd("npm");
    cmd.args(["view", package, "version", "--json"]);
    if let Some(registry) = crate::core::config::current_npm_registry() {
        cmd.arg("--registry").arg(registry);
    }
    let out = command::run_with_timeout(cmd, REMOTE_TIMEOUT)
        .map_err(|e| format!("npm view 执行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "npm view {package} 失败: {}",
            text::decode(&out.stderr).trim()
        ));
    }
    let raw = text::decode(&out.stdout);
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("解析 npm view 输出失败: {e}"))?;
    Ok(value.as_str().map(|s| s.to_string()))
}

/// 查询 git 源的目标 commit（无引用时查 HEAD）。
pub fn git_target(repo: &str, reference: Option<&str>) -> Result<Option<String>, String> {
    github::ls_remote_ref(repo, reference.unwrap_or(""))
}

/// 为一个注册记录生成同步计划项。
///
/// `warn` 是诊断输出 sink（用闭包而不是 Logger，避免把 Tauri 事件层拖进本模块的
/// 测试依赖图——否则 lib 测试二进制会连带链接 UI 运行时而需要应用清单）。
pub fn plan_for(
    record: &PluginRecord,
    installed_version: Option<String>,
    warn: &dyn Fn(&str),
) -> SyncPlanItem {
    let package = record.package.clone();
    let base = |actionable: bool, reason: String, target: Option<String>, new_spec: Option<String>, current: Option<String>| {
        SyncPlanItem {
            package: package.clone(),
            origin: record.origin,
            kind: record
                .source
                .as_ref()
                .map(|source| source.kind)
                .unwrap_or(SpecKind::Unknown),
            current,
            target,
            new_spec,
            actionable,
            reason,
        }
    };

    if record.origin != Origin::Upstream {
        return base(
            false,
            format!(
                "{}：跳过（{:?} 插件不自动同步）",
                package, record.origin
            ),
            None,
            None,
            installed_version,
        );
    }
    let Some(source) = &record.source else {
        return base(
            false,
            format!("{package}：跳过（注册表缺少来源信息）"),
            None,
            None,
            installed_version,
        );
    };

    match source.kind {
        SpecKind::Npm => match npm_latest(&package) {
            Err(error) => base(false, format!("{package}：查询失败（{error}）"), None, None, installed_version),
            Ok(None) => {
                warn(&format!("npm view {package} 无版本输出"));
                base(false, format!("{package}：registry 无版本信息"), None, None, installed_version)
            }
            Ok(Some(latest)) => {
                let current = installed_version.clone();
                let actionable = current.as_deref() != Some(latest.as_str());
                let reason = if actionable {
                    format!(
                        "{package}：{} → {}",
                        current.clone().unwrap_or_else(|| "未知".to_string()),
                        latest
                    )
                } else {
                    format!("{package}：已是最新（{latest}）")
                };
                base(
                    actionable,
                    reason,
                    Some(latest.clone()),
                    Some(format!("{package}@{latest}")),
                    current,
                )
            }
        },
        SpecKind::Git => {
            let Some(repo) = source.repo.clone() else {
                return base(
                    false,
                    format!("{package}：跳过（缺少 git 仓库地址）"),
                    None,
                    None,
                    source.commit.clone(),
                );
            };
            match git_target(&repo, source.reference.as_deref()) {
                Err(error) => base(false, format!("{package}：查询失败（{error}）"), None, None, source.commit.clone()),
                Ok(None) => base(false, format!("{package}：远端无该引用"), None, None, source.commit.clone()),
                Ok(Some(target)) => {
                    let current = source
                        .commit
                        .clone()
                        .or_else(|| spec::pinned_commit(&source.spec));
                    let actionable = current.as_deref() != Some(target.as_str());
                    let reason = if actionable {
                        format!(
                            "{package}：{} → {}",
                            current.clone().unwrap_or_else(|| "未知".to_string()),
                            short(&target)
                        )
                    } else {
                        format!("{package}：已是最新（{}）", short(&target))
                    };
                    base(
                        actionable,
                        reason,
                        Some(target.clone()),
                        Some(format!("{repo}#{target}")),
                        current,
                    )
                }
            }
        }
        _ => base(
            false,
            format!("{package}：跳过（非 npm/git 源）"),
            None,
            None,
            installed_version,
        ),
    }
}

/// 生成完整计划（保持注册表顺序）。
pub fn plan(
    records: &[PluginRecord],
    version_of: impl Fn(&str) -> Option<String>,
    warn: &dyn Fn(&str),
) -> Vec<SyncPlanItem> {
    records
        .iter()
        .map(|record| plan_for(record, version_of(&record.package), warn))
        .collect()
}

fn short(value: &str) -> String {
    value.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugin::registry::PluginSource;

    #[test]
    fn test_plan_skips_in_house_and_unknown() {
        // 使用一个不会被网络调用的 origin，验证跳过分支
        let mut record = PluginRecord::new("my-local-plugin");
        record.origin = Origin::InHouse;
        record.source = Some(PluginSource {
            kind: SpecKind::Path,
            spec: "link:../p".to_string(),
            repo: None,
            reference: None,
            commit: None,
        });
        let warn = |_: &str| {};
        let item = plan_for(&record, Some("1.0.0".to_string()), &warn);
        assert!(!item.actionable);
        assert!(item.reason.contains("不自动同步"), "{}", item.reason);
        assert_eq!(item.kind, SpecKind::Path);

        let mut unknown = PluginRecord::new("weird");
        unknown.origin = Origin::Unknown;
        let item = plan_for(&unknown, None, &warn);
        assert!(!item.actionable);
        assert!(item.reason.contains("不自动同步"));
    }

    #[test]
    fn test_plan_missing_source_is_skipped() {
        let mut record = PluginRecord::new("p");
        record.origin = Origin::Upstream;
        let warn = |_: &str| {};
        let item = plan_for(&record, None, &warn);
        assert!(!item.actionable);
        assert!(item.reason.contains("缺少来源信息"));
    }

    #[test]
    fn test_short_commit() {
        assert_eq!(short("0123456789abcdef"), "0123456789");
        assert_eq!(short("abc"), "abc");
    }
}
