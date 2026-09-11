//! MCP 前置探测：`@deepseek-ai/dsh-mcp-client` 的**可解析性**（只读，不经 dsh）
//!
//! 官方事实与本机实测（ADR-0006 §Context 1「前置」）：
//! - 该包**不是** profile 的 `dependencies` 成员，也**不在** `dsh.profile.bundles` 中；
//!   它由 dsh 安装通过符号链接提供：
//!   `$DSH_HOME/profiles/node_modules/@deepseek-ai/dsh-mcp-client`
//!   → `$DSH_SRC/apps/cli/node_modules/@deepseek-ai/dsh-mcp-client`；
//! - `dsh plugin --profile web why @deepseek-ai/dsh-mcp-client` 输出为空、退出码 0
//!   （本机实测）→ **官方 `why` 通道不能作为前置判定信号**，故本模块只做文件系统
//!   解析探测（沿用 ADR-0005 `probe_dsh_command` 的只读探测先例）。
//!
//! 探测结果只用于只读展示：不阻塞 `list`，只在 `add` 时以 `CapabilityMissing`(6) 拒绝。

use crate::core::dshhome;

/// 前置包名
pub const REQUIRED_PACKAGE: &str = "@deepseek-ai/dsh-mcp-client";

/// 前置探测结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrereqState {
    /// 可解析（目录存在且是有效包）
    Installed,
    /// 解析链全部未命中
    Missing,
}

/// 解析链（按此顺序尝试；任一条命中即可解析）
///
/// 1. `$DSH_HOME/profiles/node_modules/<pkg>`（dsh 安装写入的符号链接 / 目录）
/// 2. `$DSH_HOME/profiles/<profile>/node_modules/<pkg>`（profile 局部依赖）
pub fn resolve_candidates(profile: &str) -> Vec<std::path::PathBuf> {
    let profiles = dshhome::profiles_dir();
    vec![
        profiles.join("node_modules").join(REQUIRED_PACKAGE),
        profiles.join(profile).join("node_modules").join(REQUIRED_PACKAGE),
    ]
}

/// 目录（或其符号链接 / junction 目标）是否可解析。
///
/// `Path::is_dir()` 会跟随链接；但**符号链接悬空**时它返回 false，而
/// `symlink_metadata` 仍成功 —— 因此这里要求 `is_dir()` 为真，再额外确认能读到
/// 包的 `package.json`：悬空链接不算"可解析"。
pub fn is_resolvable_dir(path: &std::path::Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    // 悬空链接的兜底：真实读一次目录内容
    std::fs::read_dir(path).is_ok()
}

/// 包的 `package.json` 是否存在（可解析的更强证据）。
pub fn has_package_manifest(path: &std::path::Path) -> bool {
    path.join("package.json").is_file()
}

/// 解析出的实际目录（未命中返回 None）
pub fn resolve_dir(profile: &str) -> Option<std::path::PathBuf> {
    resolve_candidates(profile)
        .into_iter()
        .find(|path| is_resolvable_dir(path))
}

/// 探测前置（只读；`Missing` 不阻塞 `list`）。
pub fn probe(profile: &str) -> PrereqState {
    match resolve_dir(profile) {
        Some(dir) if has_package_manifest(&dir) => PrereqState::Installed,
        // 目录存在但无 package.json：dsh 无法把它当作包加载 → 视为缺失
        _ => PrereqState::Missing,
    }
}

/// 前置视图（IPC / CLI 输出形状，与 ADR §API 2 的 `prereq` 字段一致）
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrereqView {
    pub installed: bool,
    pub package: String,
}

impl PrereqView {
    pub fn from_state(state: &PrereqState) -> Self {
        Self {
            installed: matches!(state, PrereqState::Installed),
            package: REQUIRED_PACKAGE.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_candidates_order() {
        let candidates = resolve_candidates("web");
        assert_eq!(candidates.len(), 2);
        // ① profiles/node_modules（dsh 安装写入）优先于 ② profile 局部
        assert!(candidates[0].ends_with("profiles\\node_modules\\@deepseek-ai\\dsh-mcp-client")
            || candidates[0].ends_with("profiles/node_modules/@deepseek-ai/dsh-mcp-client"));
        assert!(candidates[1].to_string_lossy().contains("web"));
    }

    #[test]
    fn test_resolvable_dir_and_manifest() {
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-mcp-prereq-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 不存在
        let missing = root.join("missing");
        assert!(!is_resolvable_dir(&missing));
        assert!(!has_package_manifest(&missing));

        // 空目录：可解析但不是有效包
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(is_resolvable_dir(&empty));
        assert!(!has_package_manifest(&empty));

        // 有效包
        let valid = root.join("valid");
        std::fs::create_dir_all(&valid).unwrap();
        std::fs::write(valid.join("package.json"), "{\"name\":\"x\"}").unwrap();
        assert!(is_resolvable_dir(&valid));
        assert!(has_package_manifest(&valid));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_probe_missing_when_no_candidates() {
        // 用一个不可能存在的 profile 名 + 临时 DSH_HOME（不触碰真实 ~/.dsh）
        let previous = std::env::var_os("DSH_HOME");
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-mcp-prereq-home-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("DSH_HOME", &root);

        let state = probe("no-such-profile");
        assert_eq!(state, PrereqState::Missing);
        assert!(!PrereqView::from_state(&state).installed);
        assert_eq!(PrereqView::from_state(&state).package, REQUIRED_PACKAGE);

        // 造出 `profiles/node_modules/@deepseek-ai/dsh-mcp-client/package.json` → installed
        let pkg = root
            .join("profiles")
            .join("node_modules")
            .join("@deepseek-ai")
            .join("dsh-mcp-client");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("package.json"), "{\"name\":\"x\"}").unwrap();
        assert_eq!(probe("no-such-profile"), PrereqState::Installed);
        assert!(PrereqView::from_state(&probe("no-such-profile")).installed);

        // 还原环境
        match previous {
            Some(value) => std::env::set_var("DSH_HOME", value),
            None => std::env::remove_var("DSH_HOME"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
