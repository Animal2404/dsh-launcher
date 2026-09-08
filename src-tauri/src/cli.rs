//! 无 GUI CLI（同一二进制）：`dsh-launcher plugin ...` / `dsh-launcher skill ...`
//!
//! 与 Tauri IPC **共用同一 core**，因此 CLI 与 GUI 的状态机、幂等、回滚语义完全一致；
//! CLI 存在的意义是让端到端验收可脚本化（无 GUI 环境也能跑集成测试）。
//!
//! 退出码见 ADR-0005：0 成功/空操作；2 非法转换；3 未找到；4 忙；5 校验失败；
//! 6 能力缺失；7 受管区块冲突；8 dsh 未安装。

use crate::core::config::AppConfig;
use crate::core::dshhome::MANAGED_PROFILE;
use crate::core::logging::Logger;
use crate::core::plugin::{self, state::PluginError};
use crate::core::plugin::spec::Origin;
use crate::core::process::ProcessManager;
use crate::core::skill;
use serde::Serialize;
use std::sync::Arc;

/// 是否应进入 CLI 模式（第一个参数是已知子命令/帮助）
pub fn is_cli_invocation(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("plugin") | Some("skill") | Some("--help") | Some("-h") | Some("--version") | Some("-V")
    )
}

/// CLI 帮助
const HELP: &str = "\
dsh-launcher — deepseek-harness 启动器与管理器

用法：
  dsh-launcher                              启动图形界面
  dsh-launcher plugin list [--json]
  dsh-launcher plugin install <spec> [--origin upstream|in-house|unknown]
  dsh-launcher plugin enable <package>
  dsh-launcher plugin disable <package>
  dsh-launcher plugin uninstall <package>
  dsh-launcher plugin sync [--check] [--package <p>]
  dsh-launcher plugin repair [--package <p>]
  dsh-launcher skill status [--json]
  dsh-launcher skill apply [--mode auto|link|config] [--resource skills|agents-md|context-md]
  dsh-launcher skill migrate [--dry-run]

说明：
  插件按 ID（包名）独立管理生命周期；启停写 profile 的受管 patch 区块并由 dsh 热重载，
  装卸走官方 `dsh plugin --profile web ...` 通道并在需要时自动重启 dsh。
  技能共享把 ~/.agents/agent 作为唯一真源，链接到 ~/.dsh（或降级为 home 层配置）。
";

/// CLI 入口
pub fn run(args: &[String]) -> i32 {
    attach_parent_console();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        return 0;
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("dsh-launcher {}", env!("CARGO_PKG_VERSION"));
        return 0;
    }

    let logger = Arc::new(Logger::init());
    let process = Arc::new(ProcessManager::new(Arc::clone(&logger)));
    let port = AppConfig::load().port;
    if port != 0 && crate::core::port::is_port_in_use(port) {
        process.adopt_running(port);
    }

    let Some(group) = args.first() else {
        print!("{HELP}");
        return 0;
    };
    match group.as_str() {
        "plugin" => run_plugin(&args[1..], &logger, &process),
        "skill" => run_skill(&args[1..], &logger),
        other => {
            eprintln!("未知子命令: {other}\n");
            print!("{HELP}");
            2
        }
    }
}

fn run_plugin(args: &[String], logger: &Arc<Logger>, process: &Arc<ProcessManager>) -> i32 {
    let json = has_flag(args, "--json");
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("plugin 需要动作：list|install|enable|disable|uninstall|sync|repair");
        return 2;
    };
    match action {
        "list" => match plugin::list(MANAGED_PROFILE, logger) {
            Ok(list) => {
                if json {
                    return print_json(&list);
                } else {
                    if let Some(reason) = &list.degraded_reason {
                        eprintln!("注意：{reason}");
                    }
                    if list.plugins.is_empty() {
                        println!("（无受管插件）");
                    }
                    for item in &list.plugins {
                        println!(
                            "{:<44} {:<11} {:<10} {}{}",
                            item.package,
                            item.state.as_str(),
                            item.origin_str(),
                            item.version.clone().unwrap_or_else(|| "-".to_string()),
                            if item.protected { "  [受保护]" } else { "" }
                        );
                    }
                }
                0
            }
            Err(error) => report(error),
        },
        "install" => {
            let Some(spec) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin install <spec> [--origin ...]");
                return 2;
            };
            let origin = match flag_value(args, "--origin").as_deref() {
                None | Some("") => None,
                Some("upstream") => Some(Origin::Upstream),
                Some("in-house") => Some(Origin::InHouse),
                Some("unknown") => Some(Origin::Unknown),
                Some(other) => {
                    eprintln!("未知来源: {other}");
                    return 2;
                }
            };
            match plugin::install(MANAGED_PROFILE, &spec, origin, logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "enable" | "disable" => {
            let Some(package) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin {action} <package>");
                return 2;
            };
            let enabled = action == "enable";
            match plugin::set_state(MANAGED_PROFILE, &package, enabled, logger) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "uninstall" => {
            let Some(package) = positional(args, 1) else {
                eprintln!("用法: dsh-launcher plugin uninstall <package>");
                return 2;
            };
            match plugin::uninstall(MANAGED_PROFILE, &package, logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "sync" => {
            let apply = !has_flag(args, "--check");
            let only = flag_value(args, "--package");
            match plugin::sync(MANAGED_PROFILE, apply, only.as_deref(), logger, Some(process)) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        for item in &report.items {
                            println!(
                                "{:<44} {:<8} {} → {}  {}",
                                item.package,
                                item.result,
                                item.from.clone().unwrap_or_else(|| "-".to_string()),
                                item.to.clone().unwrap_or_else(|| "-".to_string()),
                                item.message
                            );
                        }
                        if !report.applied {
                            println!("（仅检查；加 --check 之外不带该参数即执行同步）");
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "repair" => {
            let only = flag_value(args, "--package");
            match plugin::repair(MANAGED_PROFILE, only.as_deref(), logger, Some(process)) {
                Ok(result) => {
                    if json {
                        return print_json(&result);
                    } else {
                        println!("{}", result.message);
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        other => {
            eprintln!("未知 plugin 动作: {other}");
            2
        }
    }
}

fn run_skill(args: &[String], logger: &Arc<Logger>) -> i32 {
    let json = has_flag(args, "--json");
    let Some(action) = args.first().map(String::as_str) else {
        eprintln!("skill 需要动作：status|apply|migrate");
        return 2;
    };
    match action {
        "status" => {
            let status = skill::status();
            if json {
                return print_json(&status);
            } else {
                println!("共享真源: {}", status.canonical_root);
                println!(
                    "DSH_HOME: {}    agentsHome: {}",
                    status.dsh_home, status.agents_home
                );
                println!(
                    "链接能力: {}    生效模式: {}    偏好: {}    技能数: {}",
                    if status.link_capable { "可用" } else { "不可用" },
                    status.active_mode,
                    status.preferred_mode,
                    status.skill_count
                );
                for item in &status.resources {
                    println!(
                        "  {:<12} {:<9} {}",
                        item.resource,
                        format!("{:?}", item.state).to_lowercase(),
                        item.detail
                    );
                }
            }
            0
        }
        "apply" => {
            let mode = flag_value(args, "--mode").unwrap_or_else(|| "auto".to_string());
            let resource = flag_value(args, "--resource");
            match skill::apply(MANAGED_PROFILE, &mode, resource.as_deref(), logger) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        println!("{}", report.message);
                        for item in &report.resources {
                            println!(
                                "  {:<12} {:<9} {}",
                                item.resource,
                                format!("{:?}", item.state).to_lowercase(),
                                item.detail
                            );
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        "migrate" => {
            let dry_run = has_flag(args, "--dry-run");
            match skill::migrate(dry_run, logger) {
                Ok(report) => {
                    if json {
                        return print_json(&report);
                    } else {
                        println!(
                            "{}",
                            if dry_run {
                                "迁移预演（未落盘）："
                            } else {
                                "迁移结果："
                            }
                        );
                        for item in &report.actions {
                            println!("  {:<12} {:<12} {}", item.resource, item.action, item.detail);
                        }
                    }
                    0
                }
                Err(error) => report(error),
            }
        }
        other => {
            eprintln!("未知 skill 动作: {other}");
            2
        }
    }
}

fn report(error: PluginError) -> i32 {
    eprintln!("错误: {}", error.message);
    error.kind.exit_code()
}

fn print_json<T: Serialize>(value: &T) -> i32 {
    match serde_json::to_string_pretty(value) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(error) => {
            eprintln!("序列化输出失败: {error}");
            1
        }
    }
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.get(index + 1).cloned()
}

/// 取第 n 个非选项参数（跳过 `--flag` 与其值）。
fn positional(args: &[String], n: usize) -> Option<String> {
    let mut values: Vec<&String> = Vec::new();
    let mut skip_next = false;
    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg.starts_with("--") {
            // 只有带值的已知选项才跳过下一个参数
            if matches!(
                arg.as_str(),
                "--origin" | "--package" | "--mode" | "--resource"
            ) {
                skip_next = true;
            }
            continue;
        }
        values.push(arg);
    }
    values.get(n).map(|value| (*value).clone())
}

/// release 构建是 windows 子系统（无控制台）：附加到父进程控制台后 CLI 输出才可见。
///
/// 关键点：`AttachConsole` 会改写本进程的标准句柄，因此必须**先**保存启动时继承的
/// stdout/stderr（管道或重定向文件），附加之后再恢复；两者都无效（终端直跑）时才用
/// `CONOUT$` 兜底。这样三种调用方式（终端直跑 / `>` 重定向 / `|` 管道）都能拿到输出。
fn attach_parent_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Console::{
            AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
            STD_OUTPUT_HANDLE,
        };
        // SAFETY: 只影响本进程的控制台附加与标准句柄；失败时保持原状
        unsafe {
            let saved = [
                (STD_OUTPUT_HANDLE, GetStdHandle(STD_OUTPUT_HANDLE).ok()),
                (STD_ERROR_HANDLE, GetStdHandle(STD_ERROR_HANDLE).ok()),
            ];
            let _ = AttachConsole(ATTACH_PARENT_PROCESS);
            for (id, handle) in saved {
                match handle {
                    Some(h) if !h.0.is_null() => {
                        let _ = SetStdHandle(id, h);
                    }
                    _ => {
                        if let Ok(file) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
                            let _ = SetStdHandle(id, HANDLE(file.as_raw_handle() as *mut _));
                            // 句柄需与进程同生命周期：泄漏这一个 File 是有意的
                            std::mem::forget(file);
                        }
                    }
                }
            }
        }
    }
    #[cfg(not(all(windows, not(debug_assertions))))]
    {
        // debug 构建带控制台；非 Windows 平台无需处理
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn test_is_cli_invocation() {
        assert!(is_cli_invocation(&args(&["plugin", "list"])));
        assert!(is_cli_invocation(&args(&["skill", "status"])));
        assert!(is_cli_invocation(&args(&["--help"])));
        assert!(!is_cli_invocation(&args(&["--web-gui"])));
        assert!(!is_cli_invocation(&[]));
    }

    #[test]
    fn test_flag_helpers() {
        let list = args(&["install", "pkg", "--origin", "upstream", "--json"]);
        assert_eq!(positional(&list, 1).as_deref(), Some("pkg"));
        assert_eq!(flag_value(&list, "--origin").as_deref(), Some("upstream"));
        assert!(has_flag(&list, "--json"));
        assert_eq!(flag_value(&list, "--package"), None);
    }

    #[test]
    fn test_positional_skips_option_values() {
        let list = args(&["sync", "--package", "dshmarket"]);
        assert_eq!(positional(&list, 0).as_deref(), Some("sync"));
        assert_eq!(positional(&list, 1), None);
    }
}
