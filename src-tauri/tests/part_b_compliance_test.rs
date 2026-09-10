//! Part B 合规回归（ADR-0006 §Testing 5；**不依赖 dsh 运行**）
//!
//! 覆盖 ADR §Testing 5 的五张表：
//! 1. **锚点合规**：锚点常量 ∈ 官方根集合；代码中不再出现 `<agentsHome>/agent` 形式的真源锚点；
//! 2. **链接修复与断链可检出**：建链后 `readlink` + 目标存在性断言全通过；**人为造断链时
//!    `skill status` 必须报 `broken`**（回归 A1 的检测能力）；
//! 3. **冲突不删**：目标位置为真实目录且有内容 → 判定 `conflict` 且**零删除**；
//! 4. **白名单例外可审计**：静态断言写 `package.json`/`pnpm-lock.yaml`/`pnpm-workspace.yaml`
//!    的调用点**仅**出现在失败回滚路径，且其后必跟官方通道 `dsh plugin … install`；
//! 5. **无自造机制**：无新增退出码数值；无新增 `%APPDATA%` 状态文件；无新增 registry/账本/加载器。
//!
//! 隔离：`DSH_HOME` / `DSH_AGENTS_HOME` 指向临时目录（不触碰真实 `~/.dsh`、`~/.agents`）。
//! 环境变量是进程级的，故用一把互斥锁串行执行。

use dsh_launcher_lib::core::dshhome;
use dsh_launcher_lib::core::plugin::state::PluginErrorKind;
use dsh_launcher_lib::core::skill::{self, ResourceState, ShareResource};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 环境变量互斥（进程级共享状态，用例必须串行）
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 隔离的临时 home 环境
struct Home {
    root: PathBuf,
    dsh_home: PathBuf,
    agents_home: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl Home {
    fn new(name: &str) -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-partb-it-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let dsh_home = root.join("dsh-home");
        let agents_home = root.join("agents-home");
        std::fs::create_dir_all(&dsh_home).unwrap();
        std::fs::create_dir_all(&agents_home).unwrap();
        std::env::set_var(dshhome::DSH_HOME_ENV, &dsh_home);
        std::env::set_var(dshhome::DSH_AGENTS_HOME_ENV, &agents_home);
        Self {
            root,
            dsh_home,
            agents_home,
            _guard: guard,
        }
    }

    fn canonical(&self, resource: ShareResource) -> PathBuf {
        skill::canonical_path(resource)
    }

    fn view(&self, resource: ShareResource) -> PathBuf {
        skill::view_path(resource)
    }

    fn status_of(&self, resource: ShareResource) -> skill::ResourceStatus {
        skill::status()
            .resources
            .into_iter()
            .find(|item| item.resource == resource.key())
            .expect("资源必须在状态表里")
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        std::env::remove_var(dshhome::DSH_HOME_ENV);
        std::env::remove_var(dshhome::DSH_AGENTS_HOME_ENV);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ==================== 1. 锚点合规（官方事实对照）====================

#[test]
fn anchor_constants_match_official_scan_roots() {
    let home = Home::new("anchors");

    // 官方 `user-agents` 技能根 = `<agentsHome>/skills`（rank 500）
    // 官方出处：packages/skill/skill-filesystem/src/index.ts:40,254
    assert_eq!(
        dshhome::agents_skills_dir(),
        home.agents_home.join("skills"),
        "技能真源必须是官方 agentsHome 根的 skills/ 子目录"
    );
    // 官方 `user-dsh` 技能根 = `<dshHome>/skills`（rank 400）
    // 官方出处：同文件 :253
    assert_eq!(dshhome::dsh_home_skills_dir(), home.dsh_home.join("skills"));
    // 用户全局指令固定 `<dshHome>/AGENTS.md`
    // 官方出处：packages/context/agent-instructions/src/files.ts:285-291、render.ts:98,106
    assert_eq!(
        dshhome::dsh_home_agents_md(),
        home.dsh_home.join("AGENTS.md")
    );
    // 共享 agent 根 = agentsHome 本身（不是 agentsHome/agent）
    assert_eq!(dshhome::shared_agent_dir(), home.agents_home);
    assert_eq!(dshhome::agents_home(), home.agents_home);

    // 三个资源的 canonical 全部落在官方 agentsHome 根下
    assert_eq!(
        home.canonical(ShareResource::Skills),
        home.agents_home.join("skills")
    );
    assert_eq!(
        home.canonical(ShareResource::AgentsMd),
        home.agents_home.join("AGENTS.md")
    );
    assert_eq!(
        home.canonical(ShareResource::ContextMd),
        home.agents_home.join("CONTEXT.md")
    );

    // 链接策略：只有指令需要链接；技能由 rank 500 原生覆盖；词表 dsh 不读
    assert!(ShareResource::AgentsMd.needs_link());
    assert!(!ShareResource::Skills.needs_link());
    assert!(!ShareResource::ContextMd.needs_link());
}

#[test]
fn no_source_reference_to_legacy_agent_anchor() {
    // 静态断言：源码中不再出现 `<agentsHome>/agent` 形式的真源锚点
    // （注释里的历史说明允许出现，但**代码**里不得再有拼接）
    let mut offenders: Vec<String> = Vec::new();
    for file in rust_sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            // `join("agent")` / `join(SHARED_AGENT_DIR)` 这类真源拼接已删除
            if code.contains("join(\"agent\")") || code.contains("SHARED_AGENT_DIR") {
                offenders.push(format!("{}:{}: {}", file.display(), index + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "源码仍存在 `<agentsHome>/agent` 形式的真源锚点：\n{}",
        offenders.join("\n")
    );
}

#[test]
fn anchor_constants_are_centralized_in_dshhome() {
    // R13：锚点常量集中在 core/dshhome.rs 一处，并在注释里标注官方出处
    let text = std::fs::read_to_string(source_root().join("core/dshhome.rs")).unwrap();
    assert!(text.contains("skill-filesystem/src/index.ts"), "必须标注官方出处");
    assert!(text.contains("agent-instructions"), "必须标注官方出处");
    assert!(text.contains("pub fn agents_skills_dir"));
    assert!(text.contains("pub fn dsh_home_agents_md"));
}

// ==================== 2. 链接修复与断链可检出 ====================

#[test]
fn broken_link_is_detected_as_broken() {
    let home = Home::new("broken-link");

    // 人为造断链：`<dshHome>/AGENTS.md` → `<agentsHome>/AGENTS.md`，但目标不存在
    let view = home.view(ShareResource::AgentsMd);
    let canonical = home.canonical(ShareResource::AgentsMd);
    assert!(!canonical.exists(), "前提：真源不存在");
    create_file_symlink(&canonical, &view);

    let status = home.status_of(ShareResource::AgentsMd);
    assert_eq!(
        status.state,
        ResourceState::Broken,
        "断链必须报 broken（回归 ADR-0006 A1 的检测能力）：{}",
        status.detail
    );
    assert!(status.detail.contains("断链"), "{}", status.detail);
}

#[test]
fn link_repair_creates_resolvable_link_and_reports_linked() {
    let home = Home::new("link-repair");

    // 前置：真源存在
    std::fs::write(home.canonical(ShareResource::AgentsMd), "# user instructions\n").unwrap();
    let view = home.view(ShareResource::AgentsMd);
    let canonical = home.canonical(ShareResource::AgentsMd);

    // 先造一条**指向别处**的断链，验证"修复"路径
    let wrong = home.root.join("wrong-target.md");
    std::fs::write(&wrong, "wrong").unwrap();
    create_file_symlink(&wrong, &view);
    let before = home.status_of(ShareResource::AgentsMd);
    assert_eq!(before.state, ResourceState::Broken);

    // 应用（link 模式）→ 修复链接
    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    let report = skill::apply("web", "link", Some("agents-md"), &logger).expect("apply 必须成功");
    assert!(report.changed, "断链修复必须报告 changed");
    let after = home.status_of(ShareResource::AgentsMd);
    assert_eq!(after.state, ResourceState::Linked, "{}", after.detail);

    // readlink + 目标存在性 双重断言
    let target = std::fs::read_link(&view).expect("必须是符号链接");
    assert_eq!(target, canonical);
    assert!(canonical.exists(), "链接目标必须存在");
    assert_eq!(
        std::fs::read_to_string(&view).unwrap(),
        "# user instructions\n",
        "内容必须等于 canonical"
    );

    // 幂等：再次 apply → unchanged
    let again = skill::apply("web", "link", Some("agents-md"), &logger).unwrap();
    assert!(!again.changed, "已 linked 必须幂等");
}

#[test]
fn skills_are_native_when_official_root_exists() {
    let home = Home::new("skills-native");
    // 造出官方技能根（含一个技能）
    let skill_dir = home.agents_home.join("skills").join("code-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# code review").unwrap();

    let status = home.status_of(ShareResource::Skills);
    assert_eq!(
        status.state,
        ResourceState::Native,
        "技能由官方 rank 500 原生覆盖，不需要链接：{}",
        status.detail
    );
    assert!(!status.needs_link);
    // **不做任何文件操作**：`<dshHome>/skills` 不得被创建
    assert!(
        !home.view(ShareResource::Skills).exists(),
        "启动器不得为技能创建任何链接/目录"
    );

    // skill status 的技能计数 = 官方根下的技能数（口径不变，基数变了）
    let overall = skill::status();
    assert_eq!(overall.skill_count, 1);
    assert_eq!(overall.agents_skills_root, home.agents_home.join("skills").display().to_string());
    assert_eq!(overall.active_mode, "native", "需要链接的资源都不缺 → native");
}

#[test]
fn context_md_is_never_linked() {
    let home = Home::new("context-nolink");
    std::fs::write(home.canonical(ShareResource::ContextMd), "# vocab\n").unwrap();
    let status = home.status_of(ShareResource::ContextMd);
    assert_eq!(status.state, ResourceState::Native);
    assert!(!status.needs_link);

    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    let report = skill::apply("web", "link", Some("context-md"), &logger).unwrap();
    assert!(!report.changed);
    assert!(
        !home.view(ShareResource::ContextMd).exists(),
        "dsh 不读 CONTEXT.md，启动器不得建链接"
    );
}

// ==================== 3. 冲突不删 ====================

#[test]
fn real_agents_md_is_conflict_and_never_deleted() {
    let home = Home::new("conflict");
    // 真源存在；视图位置是**真实文件**（用户手写的指令）
    std::fs::write(home.canonical(ShareResource::AgentsMd), "# canonical\n").unwrap();
    let view = home.view(ShareResource::AgentsMd);
    std::fs::write(&view, "# 用户手写的真实文件\n").unwrap();

    let status = home.status_of(ShareResource::AgentsMd);
    assert_eq!(
        status.state,
        ResourceState::Conflict,
        "真实文件必须判 conflict：{}",
        status.detail
    );

    // 尝试链接 → 必须拒绝（CapabilityMissing=6），且**零删除**
    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    let error = skill::apply("web", "link", Some("agents-md"), &logger).unwrap_err();
    assert_eq!(error.kind, PluginErrorKind::CapabilityMissing);
    assert_eq!(error.kind.exit_code(), 6);
    assert!(view.is_file(), "用户真实文件必须原样保留");
    assert_eq!(
        std::fs::read_to_string(&view).unwrap(),
        "# 用户手写的真实文件\n"
    );
}

#[test]
fn real_skills_directory_is_left_untouched() {
    let home = Home::new("skills-untouched");
    // 用户在 `<dshHome>/skills` 放了自己的技能（rank 400 语义）
    let own = home.dsh_home.join("skills").join("my-own");
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("SKILL.md"), "# own").unwrap();
    // 官方根也有技能
    let official = home.agents_home.join("skills").join("code-review");
    std::fs::create_dir_all(&official).unwrap();
    std::fs::write(official.join("SKILL.md"), "# official").unwrap();

    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    let report = skill::apply("web", "auto", None, &logger).unwrap();

    // 用户自己的技能目录必须原样保留（不移动、不合并、不删除）
    assert!(own.join("SKILL.md").is_file(), "用户技能目录被改动！");
    assert!(official.join("SKILL.md").is_file());
    // 用户自有的 `<dshHome>/skills` 是真实目录 → 不能被替换成链接
    assert!(
        std::fs::symlink_metadata(home.dsh_home.join("skills"))
            .map(|meta| meta.file_type().is_dir() && !meta.file_type().is_symlink())
            .unwrap_or(false),
        "启动器不得把用户真实的 skills 目录换成链接"
    );
    // 用户在 `<dshHome>/skills` 的真实目录是**用户内容** → 判定 conflict（绝不替换/删除）；
    // 技能真源由官方 `<agentsHome>/skills`（rank 500）原生覆盖，两者互不干扰。
    let skills = report
        .resources
        .iter()
        .find(|item| item.resource == "skills")
        .unwrap();
    assert_eq!(
        skills.state,
        ResourceState::Conflict,
        "用户真实目录必须判 conflict 且零动作：{}",
        skills.detail
    );

    // 再次 apply 仍不得动它（幂等 + 零删除）
    let again = skill::apply("web", "auto", None, &logger).unwrap();
    let skills_again = again
        .resources
        .iter()
        .find(|item| item.resource == "skills")
        .unwrap();
    assert_eq!(skills_again.state, ResourceState::Conflict);
    assert!(!again.changed, "无任何可做的变更时应为 unchanged");
    assert!(own.join("SKILL.md").is_file(), "用户技能目录被二次改动！");
}

// ==================== 4. 白名单例外可审计 ====================

/// 写这三个 pnpm 管辖文件的调用点**只允许**出现在失败回滚 helper 内。
#[test]
fn pnpm_managed_files_are_written_only_in_the_registered_rollback_path() {
    let targets = ["package.json", "pnpm-lock.yaml", "pnpm-workspace.yaml", "cordis.patch.yml"];
    let backup_fn = source_root().join("core/plugin/mod.rs");
    let text = std::fs::read_to_string(&backup_fn).unwrap();

    // ① 备份清单本身必须来自唯一一处 backup_targets()
    assert!(text.contains("fn backup_targets"), "必须保留 backup_targets");
    assert!(
        text.contains("fn rollback_after_failed_official_op"),
        "必须存在唯一回滚 helper（D18）"
    );

    // ② 全库静态扫查：`restore_files` / `backup_files` 的调用点
    let mut restore_calls: Vec<String> = Vec::new();
    let mut backup_calls: Vec<String> = Vec::new();
    for file in rust_sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        // 跳过测试代码（`#[cfg(test)]` 之后的部分不再参与生产路径断言）
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        for (index, line) in production.lines().enumerate() {
            if line.contains("restore_files(") && !line.trim_start().starts_with("pub fn") {
                restore_calls.push(format!("{}:{}", file.display(), index + 1));
            }
            if line.contains("backup_files(") && !line.trim_start().starts_with("pub fn") {
                backup_calls.push(format!("{}:{}", file.display(), index + 1));
            }
        }
    }
    // 调用点清单（人工审计基线）：restore 只在回滚 helper 与 MCP 自校验回滚里；
    // backup 只在装卸前置备份与 MCP 前置备份里。
    assert!(
        !restore_calls.is_empty(),
        "至少应有回滚调用点（否则 D18 例外登记失去意义）"
    );
    for call in &restore_calls {
        assert!(
            call.contains("core\\plugin\\mod.rs") || call.contains("core/plugin/mod.rs")
                || call.contains("core\\mcp\\mod.rs") || call.contains("core/mcp/mod.rs"),
            "出现未登记的 restore_files 调用点：{call}"
        );
    }

    // ③ 回滚 helper 内部：还原之后**必须**跟一次官方通道 install
    let helper_start = text.find("fn rollback_after_failed_official_op").unwrap();
    let helper_end = text[helper_start..]
        .find("\n}\n")
        .map(|offset| helper_start + offset)
        .unwrap_or(text.len());
    let helper = &text[helper_start..helper_end];
    assert!(helper.contains("restore_files("), "helper 必须做还原");
    assert!(
        helper.contains("run_plugin_forward(") && helper.contains("\"install\""),
        "还原后必须紧跟官方通道 `dsh plugin … install` 收敛（D18）"
    );

    // ④ 除 helper 与 MCP 之外，任何地方都不得再直接写这三个文件
    for file in rust_sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        let production = text.split("#[cfg(test)]").next().unwrap_or("");
        for (index, line) in production.lines().enumerate() {
            if line.trim_start().starts_with("//") || line.trim_start().starts_with("///") {
                continue;
            }
            let writes = line.contains("fs::write(") || line.contains("write_atomic(");
            if !writes {
                continue;
            }
            for target in targets {
                if line.contains(&format!("join(\"{target}\")")) {
                    panic!(
                        "未登记的直接写入 {} ：{}:{}: {}",
                        target,
                        file.display(),
                        index + 1,
                        line.trim()
                    );
                }
            }
        }
    }
}

// ==================== 5. 无自造机制 ====================

#[test]
fn no_new_exit_code_values() {
    // 复用既有分级：2/3/4/5/6/7/8/1 —— 数值集合不得变化
    let kinds = [
        PluginErrorKind::IllegalTransition,
        PluginErrorKind::NotFound,
        PluginErrorKind::Busy,
        PluginErrorKind::VerificationFailed,
        PluginErrorKind::CapabilityMissing,
        PluginErrorKind::ManagedBlockConflict,
        PluginErrorKind::DshNotInstalled,
        PluginErrorKind::Internal,
    ];
    let codes: Vec<i32> = kinds.iter().map(|kind| kind.exit_code()).collect();
    assert_eq!(codes, vec![2, 3, 4, 5, 6, 7, 8, 1]);

    // 源码中不得出现**字面量**自定义退出码（`main.rs` 的
    // `exit(cli::run(...))` 是透传既有分级，不属于新增）
    for file in rust_sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if !code.contains("std::process::exit(") {
                continue;
            }
            // 只允许透传（参数不是字面量）或 0/1（内部错误）
            let literal = code
                .split("std::process::exit(")
                .nth(1)
                .and_then(|rest| rest.split(')').next())
                .map(str::trim)
                .unwrap_or("");
            let is_passthrough = !literal.is_empty()
                && !literal.chars().all(|c| c.is_ascii_digit() || c == '-');
            assert!(
                is_passthrough || literal == "0" || literal == "1",
                "出现未登记的自定义退出码字面量：{}:{}: {}",
                file.display(),
                index + 1,
                line.trim()
            );
        }
    }
}

#[test]
fn no_new_persistent_state_file_for_mcp() {
    // ADR-0006 明确：MCP **不引入任何新的持久化状态文件**（磁盘即事实源）。
    // 静态断言：源码里不存在 `mcps.json` / `mcp.json` 之类的状态文件名。
    for file in rust_sources() {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for banned in ["mcps.json", "mcp.json", "mcp-state.json"] {
                assert!(
                    !code.contains(banned),
                    "不得新增 MCP 状态文件：{}:{}: {}",
                    file.display(),
                    index + 1,
                    line.trim()
                );
            }
        }
    }
    // 运行时路径里也不应出现
    assert!(!dsh_launcher_lib::core::mcp::backups_root()
        .to_string_lossy()
        .contains(".json"));
}

// ==================== 6. 迁移动作（G3 清单的可重跑实现）====================

#[test]
fn repair_links_cleans_launcher_broken_links_and_repairs_instruction_link() {
    let home = Home::new("repair-links");
    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());

    // 复刻修复前的现场：
    // ① 真源（官方 agentsHome 根）—— 技能与指令都在
    let skill_dir = home.agents_home.join("skills").join("code-review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# review").unwrap();
    std::fs::write(home.canonical(ShareResource::AgentsMd), "# 用户全局指令\n").unwrap();
    std::fs::write(home.canonical(ShareResource::ContextMd), "# 词表\n").unwrap();
    // ② 启动器遗留的三条断链（全部指向已空的 `<agentsHome>/agent/*`）
    let legacy = home.agents_home.join("agent");
    std::fs::create_dir_all(&legacy).unwrap();
    create_dir_link(&legacy.join("skills"), &home.view(ShareResource::Skills));
    create_file_symlink(&legacy.join("AGENTS.md"), &home.view(ShareResource::AgentsMd));
    create_file_symlink(
        &legacy.join("CONTEXT.md"),
        &home.view(ShareResource::ContextMd),
    );
    // 前置断言：三条都是断链
    for resource in ShareResource::ALL {
        assert_eq!(
            home.status_of(resource).state,
            ResourceState::Broken,
            "{:?} 应为断链",
            resource
        );
    }

    // 执行迁移动作
    let report = skill::repair_links("web", &logger).unwrap();
    let action_of = |key: &str| {
        report
            .actions
            .iter()
            .find(|item| item.resource == key)
            .map(|item| item.action.clone())
            .unwrap()
    };
    assert_eq!(action_of("agents-md"), "repaired", "指令链接必须被修复");
    assert_eq!(
        action_of("skills"),
        "cleaned-broken-link",
        "技能不再需要链接 → 清理启动器遗留断链"
    );
    assert_eq!(action_of("context-md"), "cleaned-broken-link");

    // 修复结果：指令已链接且可解析；另外两条不再存在
    let agents = home.status_of(ShareResource::AgentsMd);
    assert_eq!(agents.state, ResourceState::Linked, "{}", agents.detail);
    assert!(home.canonical(ShareResource::AgentsMd).exists());
    assert!(!home.view(ShareResource::Skills).exists(), "断链应已清理");
    assert!(!home.view(ShareResource::ContextMd).exists(), "断链应已清理");

    // `~/.agents/agent/` 本身（用户目录）必须保留
    assert!(legacy.is_dir(), "用户目录 ~/.agents/agent 不得被删除");

    // 幂等重跑：不再产生任何变更
    let again = skill::repair_links("web", &logger).unwrap();
    for item in &again.actions {
        assert!(
            item.action == "kept",
            "重跑必须幂等，却产生 {}：{}",
            item.action,
            item.detail
        );
    }
    assert_eq!(home.status_of(ShareResource::AgentsMd).state, ResourceState::Linked);
}

#[test]
fn repair_links_keeps_user_created_links_outside_agents_home() {
    let home = Home::new("repair-keeps-user-link");
    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());
    std::fs::write(home.canonical(ShareResource::ContextMd), "# 词表\n").unwrap();

    // 用户自己建的链接（目标在 agentsHome **之外**）→ 必须保留
    let elsewhere = home.root.join("my-own-vocab.md");
    std::fs::write(&elsewhere, "# 我的词表\n").unwrap();
    let view = home.view(ShareResource::ContextMd);
    create_file_symlink(&elsewhere, &view);
    assert_eq!(
        home.status_of(ShareResource::ContextMd).state,
        ResourceState::Broken
    );

    let report = skill::repair_links("web", &logger).unwrap();
    let context = report
        .actions
        .iter()
        .find(|item| item.resource == "context-md")
        .unwrap();
    assert_eq!(context.action, "kept", "非启动器创建的链接绝不删除");
    assert!(view.exists() || std::fs::symlink_metadata(&view).is_ok(), "用户链接必须保留");
    assert!(elsewhere.is_file(), "用户目标文件必须保留");
}

#[test]
fn repair_links_never_touches_real_files() {
    let home = Home::new("repair-real-files");
    let logger = std::sync::Arc::new(dsh_launcher_lib::core::logging::Logger::init());

    // 用户在 `<dshHome>/AGENTS.md` 放了真实文件
    let view = home.view(ShareResource::AgentsMd);
    std::fs::write(&view, "# 用户手写\n").unwrap();
    std::fs::write(home.canonical(ShareResource::AgentsMd), "# canonical\n").unwrap();

    let report = skill::repair_links("web", &logger).unwrap();
    let agents = report
        .actions
        .iter()
        .find(|item| item.resource == "agents-md")
        .unwrap();
    assert_eq!(agents.action, "kept", "真实文件必须被保留并提示");
    assert_eq!(std::fs::read_to_string(&view).unwrap(), "# 用户手写\n");
}

// ==================== 扫描辅助 ====================

/// 源码根目录：优先用 `CARGO_MANIFEST_DIR`，回退当前工作目录。
fn source_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("src")
}

/// 递归收集 `src/**/*.rs`
fn rust_sources() -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rs(&source_root(), &mut out);
    out.sort();
    out
}

fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().map(|ext| ext == "rs").unwrap_or(false) {
            out.push(path);
        }
    }
}

/// 创建文件符号链接（Windows 需开发者模式；失败时跳过该用例）
#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = std::os::windows::fs::symlink_file(target, link) {
        panic!("创建符号链接失败（需开发者模式/符号链接特权）: {error}");
    }
}

#[cfg(not(windows))]
fn create_file_symlink(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::os::unix::fs::symlink(target, link).unwrap();
}

/// 创建目录链接（Windows 用 junction，免特权；复刻 `skill::create_dir_link`）
#[cfg(windows)]
fn create_dir_link(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let output = std::process::Command::new("cmd")
        .args(["/D", "/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("执行 mklink 失败");
    assert!(
        output.status.success(),
        "创建 junction 失败: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(not(windows))]
fn create_dir_link(target: &Path, link: &Path) {
    if let Some(parent) = link.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::os::unix::fs::symlink(target, link).unwrap();
}
