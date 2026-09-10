//! DSH_HOME / profiles / agents home 路径解析（全项目唯一实现）
//!
//! 与 dsh 官方保持一致（对齐 `deepseek-harness/packages/util/home-paths/src/index.ts`）：
//! - `DSH_HOME` 环境变量非空白时优先，否则 `%USERPROFILE%\.dsh`；
//! - 支持 `~` / `~/` / `~\` 前缀展开；
//! - 空白 `DSH_HOME` 视为未设置（避免把 home 解析到当前目录）。
//!
//! 启动器不设置 `DSH_HOME`，子进程继承同一环境，因此双方解析结果一致；
//! 所有需要 DSH_HOME 的模块必须调用本模块，禁止各自拼路径。
//!
//! **官方扫描根常量集中在本模块**（ADR-0006 R13 / D17），每处都标注官方出处，
//! 便于随上游版本核对：
//!
//! | 资源 | 路径 | 官方出处 |
//! |---|---|---|
//! | 技能（官方根，rank 500） | `<agentsHome>/skills` | `packages/skill/skill-filesystem/src/index.ts:40,254` |
//! | 技能（dsh home 根，rank 400） | `<dshHome>/skills` | 同上 `:253` |
//! | 用户全局指令 | `<dshHome>/AGENTS.md` | `packages/context/agent-instructions/src/files.ts:285-291`、`render.ts:98,106` |
//! | 共享 agent 根 | `<agentsHome>`（默认 `~/.agents`） | `skill-filesystem/src/index.ts:164` |

use std::path::{Path, PathBuf};

/// 覆盖 Harness home 的环境变量名
pub const DSH_HOME_ENV: &str = "DSH_HOME";
/// 覆盖共享 agent 根目录的环境变量名
pub const DSH_AGENTS_HOME_ENV: &str = "DSH_AGENTS_HOME";
/// Harness home 下的 profile 目录名
pub const PROFILES_DIR: &str = "profiles";
/// 启动器唯一管理的 profile（`dsh web` 的官方别名目标）
pub const MANAGED_PROFILE: &str = "web";
/// profile 用户 patch 层文件名
pub const PROFILE_PATCH_FILENAME: &str = "cordis.patch.yml";
/// home 级用户 patch 层文件名
pub const HOME_PATCH_FILENAME: &str = "cordis.patch.yml";
/// 技能目录名（官方两个技能根都叫 `skills`）
pub const SKILLS_DIR: &str = "skills";
/// 用户全局指令文件名（官方固定读 `<dshHome>/AGENTS.md`）
pub const AGENTS_MD_FILENAME: &str = "AGENTS.md";
/// 词表文件名（**dsh 不读**：全仓无引用，仅 agent/技能侧约定资源）
pub const CONTEXT_MD_FILENAME: &str = "CONTEXT.md";

/// 操作系统用户目录（`USERPROFILE` 优先，兼容 `HOME`）。
pub fn user_home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 展开 `~` / `~/` / `~\` 前缀。
pub fn expand_home(raw: &str, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

/// 纯函数版 home 解析（可单测，不读进程环境）。
pub fn resolve_dsh_home(configured: Option<&str>, user: &Path) -> PathBuf {
    match configured {
        Some(v) if !v.trim().is_empty() => expand_home(v.trim(), user),
        _ => user.join(".dsh"),
    }
}

/// 纯函数版共享 agent 根解析（可单测，不读进程环境）。
pub fn resolve_agents_home(configured: Option<&str>, user: &Path) -> PathBuf {
    match configured {
        Some(v) if !v.trim().is_empty() => expand_home(v.trim(), user),
        _ => user.join(".agents"),
    }
}

/// 当前进程的 Harness home。
pub fn dsh_home() -> PathBuf {
    let configured = std::env::var(DSH_HOME_ENV).ok();
    resolve_dsh_home(configured.as_deref(), &user_home())
}

/// 当前进程的共享 agent 根（默认 `~/.agents`）。
pub fn agents_home() -> PathBuf {
    let configured = std::env::var(DSH_AGENTS_HOME_ENV).ok();
    resolve_agents_home(configured.as_deref(), &user_home())
}

/// 共享资源真源根 = **官方 `agentsHome` 根**（默认 `~/.agents`）。
///
/// 官方语义（`skill-filesystem/src/index.ts:164,254`）：
/// `agentsHome` 是**共享 agent 配置根**，其下的 `skills/` 才是 `user-agents`
/// 技能根（rank 500）。因此共享真源就是 `agentsHome` 本身，**不是** `agentsHome/agent`
/// —— 后者既非官方扫描根、又会让 `<agentsHome>/agent/skills` 落空（ADR-0006 D17）。
pub fn shared_agent_dir() -> PathBuf {
    agents_home()
}

/// 官方技能根（`user-agents`，rank 500）：`<agentsHome>/skills`。
///
/// 见 `packages/skill/skill-filesystem/src/index.ts:40,254`。这是启动器的
/// **技能共享真源**；因为 rank 500 原生覆盖，技能**不需要任何链接**。
pub fn agents_skills_dir() -> PathBuf {
    agents_home().join(SKILLS_DIR)
}

/// 官方 dsh-home 技能根（`user-dsh`，rank 400）：`<dshHome>/skills`。
///
/// 见 `packages/skill/skill-filesystem/src/index.ts:253`（带 `skipSystem`）。
pub fn dsh_home_skills_dir() -> PathBuf {
    dsh_home().join(SKILLS_DIR)
}

/// 官方用户全局指令文件：**固定**为 `<dshHome>/AGENTS.md`。
///
/// 见 `packages/context/agent-instructions/src/files.ts:285-291` 与
/// `render.ts:98,106`（`USER_GLOBAL_FILE`/`USER_GLOBAL_DIRECTORY`）。
pub fn dsh_home_agents_md() -> PathBuf {
    dsh_home().join(AGENTS_MD_FILENAME)
}

/// 共享真源侧的指令文件：`<agentsHome>/AGENTS.md`。
pub fn agents_home_agents_md() -> PathBuf {
    agents_home().join(AGENTS_MD_FILENAME)
}

/// `<dshHome>/profiles`。
pub fn profiles_dir() -> PathBuf {
    dsh_home().join(PROFILES_DIR)
}

/// 校验 profile 名（与 dsh `resolveProfileDir` 同一套拒绝规则）。
pub fn validate_profile_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name == "node_modules"
        || name.contains('/')
        || name.contains('\\')
    {
        return Err(format!("非法 profile 名: {name:?}"));
    }
    Ok(())
}

/// 解析 profile 目录（不保证存在）。
pub fn profile_dir(name: &str) -> Result<PathBuf, String> {
    validate_profile_name(name)?;
    Ok(profiles_dir().join(name))
}

/// profile 的 `cordis.patch.yml` 路径。
pub fn profile_patch_path(name: &str) -> Result<PathBuf, String> {
    Ok(profile_dir(name)?.join(PROFILE_PATCH_FILENAME))
}

/// home 级 `$DSH_HOME/cordis.patch.yml` 路径。
pub fn home_patch_path() -> PathBuf {
    dsh_home().join(HOME_PATCH_FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_dsh_home_precedence() {
        let user = Path::new("C:\\Users\\tester");
        // 未设置 → 默认 ~/.dsh
        assert_eq!(resolve_dsh_home(None, user), user.join(".dsh"));
        // 空白视为未设置
        assert_eq!(resolve_dsh_home(Some("   "), user), user.join(".dsh"));
        // 显式路径优先
        assert_eq!(
            resolve_dsh_home(Some("D:\\data\\dsh"), user),
            PathBuf::from("D:\\data\\dsh")
        );
        // ~ 展开
        assert_eq!(
            resolve_dsh_home(Some("~/.dsh-alt"), user),
            user.join(".dsh-alt")
        );
        assert_eq!(resolve_dsh_home(Some("~"), user), user.to_path_buf());
    }

    #[test]
    fn test_resolve_agents_home() {
        let user = Path::new("C:\\Users\\tester");
        assert_eq!(resolve_agents_home(None, user), user.join(".agents"));
        assert_eq!(
            resolve_agents_home(Some("~/.agents2"), user),
            user.join(".agents2")
        );
    }

    #[test]
    fn test_validate_profile_name() {
        assert!(validate_profile_name("web").is_ok());
        assert!(validate_profile_name("tui-custom").is_ok());
        for bad in ["", ".", "..", "node_modules", "a/b", "a\\b"] {
            assert!(validate_profile_name(bad).is_err(), "{bad} 应被拒绝");
        }
    }
}
