//! 核心逻辑模块：进程、端口、配置、日志、插件、技能
//!
//! 分层约定（ADR-0005）：
//! - `profile.rs` 是唯一调用 `dsh`/`pnpm` 与读写 profile 文件的适配器；
//! - `dshhome.rs` 是唯一的 DSH_HOME/agents home 解析实现；
//! - `plugin/*` 是插件状态机与注册表；`skill.rs` 是技能共享资源管理。

pub mod command;
pub mod config;
pub mod dshhome;
pub mod events;
pub mod github;
pub mod logging;
pub mod pathutil;
pub mod plugin;
pub mod port;
pub mod process;
pub mod profile;
pub mod skill;
pub mod stream;
pub mod text;
pub mod toolchain;
pub mod tray;
