//! 官方 API 适配器：**唯一**允许调用 `dsh`/`pnpm` 与读写 profile 文件的地方
//!
//! 边界（ADR-0005 API 一节）：
//! - 行发现：`dsh --profile <p> --dump-config`（只读、不启动插件）；
//! - 依赖变更：`dsh plugin --profile <p> add|remove|update|install ...`；
//! - profile manifest / patch 层：只读 manifest，只写 `cordis.patch.yml` 的受管区块。
//!
//! 其余模块禁止自行 `Command::new("dsh")` 或直接写 `$DSH_HOME`。

use crate::core::dshhome;
use crate::core::github::{self, DshProbe};
use crate::core::plugin::state::{PluginError, PluginErrorKind};
use crate::core::text;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

/// dump / 只读查询的超时
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(120);
/// 依赖变更（pnpm 网络操作）的超时
pub const MUTATION_TIMEOUT: Duration = Duration::from_secs(900);

/// profile manifest 中与插件管理相关的切片
#[derive(Debug, Clone, Default)]
pub struct ProfileManifest {
    /// `dependencies`
    pub dependencies: BTreeMap<String, String>,
    /// `dsh.profile.bundles`
    pub bundles: Vec<String>,
    /// `dsh.profile.patchReload`
    pub patch_reload: Option<String>,
}

/// 解析出的 dsh 可执行入口
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshEntry {
    /// GitHub 通道：源码目录 + node 直接启动
    SourceDir { dir: PathBuf, node: PathBuf },
    /// PATH 中的 dsh（npm 全局包或可用的自家 shim）
    Path,
}

/// GitHub 通道安装目录（存在源码入口时才算可用）。
pub fn install_dir() -> Option<PathBuf> {
    let dir = github::github_clone_dir();
    if dir.join("apps/cli/src/bin.ts").exists() {
        Some(dir)
    } else {
        None
    }
}

/// 解析 node.exe（用户级工具链优先，其次 PATH）。
pub fn node_exe() -> Option<PathBuf> {
    let candidate = crate::core::toolchain::node_dir().join("node.exe");
    if candidate.exists() {
        return Some(candidate);
    }
    let mut probe = crate::core::command::hidden("where");
    probe.arg("node");
    if let Ok(out) = probe.output() {
        if out.status.success() {
            let decoded = text::decode(&out.stdout);
            if let Some(line) = decoded.lines().map(str::trim).find(|l| !l.is_empty()) {
                let path = PathBuf::from(line);
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// 解析 dsh 入口；不可安全执行时返回 `DshNotInstalled`。
///
/// 复用 `github::probe_dsh_command()` 的静态判定：自家 shim 指向损坏目录时
/// **绝不执行** dsh（否则 pnpm 递归进程爆炸，见 core/github.rs 的说明）。
pub fn resolve_entry() -> Result<DshEntry, PluginError> {
    let probe = github::probe_dsh_command();
    if probe == DshProbe::OwnedShimBroken {
        return Err(PluginError::new(
            PluginErrorKind::DshNotInstalled,
            "dsh 安装目录缺失（GitHub shim 指向的目录已不存在或为空），请重新安装 dsh",
        ));
    }
    if let Some(dir) = install_dir() {
        let node = node_exe().ok_or_else(|| {
            PluginError::new(
                PluginErrorKind::DshNotInstalled,
                "未找到 node.exe，无法调用 dsh（请先安装 Node 工具链）",
            )
        })?;
        return Ok(DshEntry::SourceDir { dir, node });
    }
    if probe != DshProbe::None {
        return Ok(DshEntry::Path);
    }
    Err(PluginError::new(
        PluginErrorKind::DshNotInstalled,
        "未找到 dsh（PATH 无 dsh 且无 GitHub 安装目录），请先在版本管理中安装 dsh",
    ))
}

/// 构造 `dsh <args...>` 命令（不 spawn）。
pub fn build_dsh_command(args: &[String]) -> Result<Command, PluginError> {
    match resolve_entry()? {
        DshEntry::SourceDir { dir, node } => {
            let mut cmd = crate::core::command::hidden(&node);
            cmd.current_dir(&dir);
            cmd.args(["--import", "tsx/esm", "apps/cli/src/bin.ts"]);
            cmd.args(args);
            Ok(cmd)
        }
        DshEntry::Path => {
            let mut cmd = crate::core::command::hidden_cmd("dsh");
            cmd.args(args);
            Ok(cmd)
        }
    }
}

/// 带超时执行 dsh 并捕获输出。
pub fn run_dsh(args: &[String], timeout: Duration) -> Result<Output, PluginError> {
    let cmd = build_dsh_command(args)?;
    crate::core::command::run_with_timeout(cmd, timeout).map_err(|e| {
        PluginError::internal(format!("执行 dsh 失败: {e}"))
    })
}

/// 执行 `dsh --profile <p> --dump-config` 并返回 stdout。
pub fn dump_config(profile: &str) -> Result<String, PluginError> {
    let args = vec![
        "--profile".to_string(),
        profile.to_string(),
        "--dump-config".to_string(),
    ];
    let out = run_dsh(&args, QUERY_TIMEOUT)?;
    if !out.status.success() {
        return Err(PluginError::internal(format!(
            "dsh --dump-config 失败（退出码 {}）：{}",
            out.status.code().unwrap_or(-1),
            text::decode(&out.stderr).trim()
        )));
    }
    Ok(text::decode(&out.stdout))
}

/// 读取 profile manifest。
pub fn read_manifest(dir: &Path) -> Result<ProfileManifest, PluginError> {
    let path = dir.join("package.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| PluginError::internal(format!("读取 {} 失败: {e}", path.display())))?;
    let value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| PluginError::internal(format!("解析 {} 失败: {e}", path.display())))?;

    let mut dependencies = BTreeMap::new();
    if let Some(map) = value.get("dependencies").and_then(|v| v.as_object()) {
        for (key, item) in map {
            if let Some(spec) = item.as_str() {
                dependencies.insert(key.clone(), spec.to_string());
            }
        }
    }
    let dsh = value.get("dsh");
    let bundles = dsh
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.get("bundles"))
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let patch_reload = dsh
        .and_then(|v| v.get("profile"))
        .and_then(|v| v.get("patchReload"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(ProfileManifest {
        dependencies,
        bundles,
        patch_reload,
    })
}

/// 解析已安装包目录：先 profile `node_modules`，再 dsh 安装目录。
pub fn resolve_package_dir(package: &str, profile_dir: &Path) -> Option<PathBuf> {
    let mut anchors: Vec<PathBuf> = Vec::new();
    anchors.push(profile_dir.join("node_modules").join(package));
    if let Some(dir) = install_dir() {
        anchors.push(dir.join("node_modules").join(package));
    }
    if let Ok(profile_anchor) = std::env::current_dir() {
        anchors.push(profile_anchor.join("node_modules").join(package));
    }
    anchors.into_iter().find(|path| path.join("package.json").exists())
}

/// 读取已安装包的 manifest（找不到返回 None）。
pub fn read_package_manifest(package: &str, profile_dir: &Path) -> Option<serde_json::Value> {
    let dir = resolve_package_dir(package, profile_dir)?;
    let raw = std::fs::read_to_string(dir.join("package.json")).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 包是否声明 `dsh.bundle.patch`（bundle 判定，对齐 plugin.ts:36-45）。
pub fn declares_bundle(package: &str, profile_dir: &Path) -> bool {
    read_package_manifest(package, profile_dir)
        .and_then(|value| {
            value
                .get("dsh")
                .and_then(|v| v.get("bundle"))
                .and_then(|v| v.get("patch"))
                .map(|_| ())
        })
        .is_some()
}

/// 读取已安装包版本。
pub fn package_version(package: &str, profile_dir: &Path) -> Option<String> {
    read_package_manifest(package, profile_dir)
        .and_then(|value| value.get("version").and_then(|v| v.as_str()).map(|s| s.to_string()))
}

/// 读取包名清单（profile 依赖）。
pub fn dependency_names(dir: &Path) -> Result<Vec<String>, PluginError> {
    Ok(read_manifest(dir)?.dependencies.into_keys().collect())
}

/// 备份若干文件到 `dest` 目录（不存在或读取失败的文件跳过），返回备份清单。
pub fn backup_files(files: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>, PluginError> {
    std::fs::create_dir_all(dest)
        .map_err(|e| PluginError::internal(format!("创建备份目录 {} 失败: {e}", dest.display())))?;
    let mut saved = Vec::new();
    for file in files {
        if !file.exists() {
            continue;
        }
        let Some(name) = file.file_name() else {
            continue;
        };
        let target = dest.join(name);
        std::fs::copy(file, &target).map_err(|e| {
            PluginError::internal(format!(
                "备份 {} → {} 失败: {e}",
                file.display(),
                target.display()
            ))
        })?;
        saved.push(target);
    }
    Ok(saved)
}

/// 用备份覆盖回原文件（按文件名匹配）。
pub fn restore_files(backup_dir: &Path, targets: &[PathBuf]) -> Result<Vec<String>, PluginError> {
    let mut restored = Vec::new();
    for target in targets {
        let Some(name) = target.file_name() else {
            continue;
        };
        let source = backup_dir.join(name);
        if !source.exists() {
            continue;
        }
        std::fs::copy(&source, target).map_err(|e| {
            PluginError::internal(format!(
                "回滚 {} 失败: {e}",
                target.display()
            ))
        })?;
        restored.push(target.display().to_string());
    }
    Ok(restored)
}

/// 时间戳（用于备份目录名，避免引入 chrono 依赖）。
pub fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{now}")
}

/// profile 目录（`$DSH_HOME/profiles/<name>`）。
pub fn profile_dir(profile: &str) -> Result<PathBuf, PluginError> {
    dshhome::profile_dir(profile)
        .map_err(|e| PluginError::internal(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_parsing() {
        let dir = std::env::temp_dir().join(format!("dsh-launcher-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("package.json"),
            r#"{
              "name": "dsh-profile-web",
              "dependencies": { "dshmarket": "^1.45.0", "dsh-cost-meter": "^1.7.13" },
              "dsh": { "profile": { "bundles": ["@deepseek-ai/dsh-base", "dshmarket"], "patchReload": "live" } }
            }"#,
        )
        .unwrap();
        let manifest = read_manifest(&dir).unwrap();
        assert_eq!(manifest.dependencies.len(), 2);
        assert_eq!(manifest.dependencies.get("dshmarket").map(String::as_str), Some("^1.45.0"));
        assert_eq!(manifest.bundles, vec!["@deepseek-ai/dsh-base", "dshmarket"]);
        assert_eq!(manifest.patch_reload.as_deref(), Some("live"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_backup_and_restore_roundtrip() {
        let root = std::env::temp_dir().join(format!("dsh-launcher-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("profile");
        let bak = root.join("bak");
        std::fs::create_dir_all(&src).unwrap();
        let file = src.join("package.json");
        std::fs::write(&file, "{\"a\":1}").unwrap();
        backup_files(std::slice::from_ref(&file), &bak).unwrap();
        std::fs::write(&file, "{\"a\":2}").unwrap();
        restore_files(&bak, std::slice::from_ref(&file)).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
