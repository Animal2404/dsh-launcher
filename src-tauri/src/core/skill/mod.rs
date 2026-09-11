//! 技能模块（ADR-0007 / ADR-0008）
//!
//! 三个互相独立的部分：
//!
//! - **管理**（ADR-0007）：`frontmatter` / `scan` / `manage`，对**用户级技能根**
//!   （rank 400 `<dshHome>/skills`、rank 500 `<agentsHome>/skills`）提供列出、
//!   启用/停用、可恢复删除。写入是**改写官方 `SKILL.md` frontmatter** 的逐行外科手术
//!   —— 这是官方唯一可达的启停路径（官方无技能 CLI/HTTP/Remote/配置键）。
//! - **导入与更新**（ADR-0008）：`source`（来源注册表）、`import`（git 克隆 + 递归
//!   扁平化 + 文件级覆盖）。官方**只发现扫描根顶层一层**，而真实技能仓库普遍分类嵌套
//!   （如 `skills/<分类>/<name>/SKILL.md`），故必须由导入器压平。
//! - **外部打开**（ADR-0008）：`editor`，前端对受管文件只传闭集枚举、路径由 Rust 推导，
//!   故无需改动 Tauri capability。
//! - **共享**（ADR-0005，ADR-0007 D12 退役前端入口）：`sharing` 保留为 CLI 后端与
//!   应急路径。技能走官方 rank 500 原生覆盖，本不需要链接。

pub mod editor;
pub mod frontmatter;
pub mod import;
pub mod manage;
pub mod scan;
pub mod source;
pub mod update;

/// ADR-0005 的共享机制（前端入口已在 v0.8.0 退役，保留 CLI 后端）
pub mod sharing;

// 兼容既有调用点：`cli.rs` 用 `crate::core::skill::<item>` 访问共享机制的公开面。
// ADR-0007 只退役前端 UI，不删除 Rust 后端（共享应急路径仍在）。
pub use sharing::{
    apply, canonical_path, count_skills, link_capable, migrate, repair_links, status, view_path,
    MigrateAction, MigrateReport, ResourceState, ResourceStatus, ShareResource, SkillApplyReport,
    SkillSettings, SkillStatus,
};

/// 管理侧的公开面（供 `commands/skill.rs` 使用）
pub use manage::{DeleteReport, ToggleReport};
pub use scan::{list, SkillEntry, SkillError, SkillList, SkillState, RootStatus};
