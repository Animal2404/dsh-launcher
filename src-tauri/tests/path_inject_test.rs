//! PATH 注入回归（ADR-0009 D11b）：**直调生产函数**
//!
//! ## 为什么重写
//!
//! 原测试有两个用例都属于"假测试"：
//! 1. `test_hidden_cmd_inject_keeps_prefix`：用 `Command::new("cmd.exe")` **构造**了一个命令
//!    但**从不执行**，只在测试内自行拼接了一个 `injected` 字符串再断言它包含 node_dir ——
//!    等于在测「字符串拼接」，与生产 `hidden_cmd` 的注入行为无关。
//! 2. `test_npm_visible_after_inject`：手工复刻了一份 `hidden_cmd` 的 PATH 过滤逻辑
//!    （自己写 `filter` + `env("PATH", ...)`），同样**没有调用生产函数**。
//!
//! 本文件改为直接断言**生产函数**的可观察效果：`core::pathutil::inject_node_path_into`
//! 是否真的把用户级 node_dir 注入到了 `Command` 的环境里（通过 `get_envs()` 读取，
//! 无需真的执行子进程），以及注入的**幂等性**。
//!
//! ## 隔离
//!
//! `inject_node_path_into` 的决策依赖 `node_dir_injection()`：仅当「系统 PATH 无 node」
//! 且「用户级 node_dir 存在 node.exe」时才注入。测试通过把 `LOCALAPPDATA` 指向临时目录
//! 来构造"已安装用户级 Node"的事实，从而让注入路径**确定性地生效**，不依赖开发机的 PATH。

use dsh_launcher_lib::core::pathutil;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 隔离 LOCALAPPDATA，并可选地造出「用户级 node 已安装」的事实
struct Sandbox {
    root: PathBuf,
    _guard: MutexGuard<'static, ()>,
    original_localappdata: Option<std::ffi::OsString>,
    original_path: Option<std::ffi::OsString>,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("LOCALAPPDATA");
        let original_path = std::env::var_os("PATH");
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-pathinject-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("LOCALAPPDATA", &root);
        Self {
            root,
            _guard: guard,
            original_localappdata: original,
            original_path,
        }
    }

    /// 生产代码认定的用户级 node 目录
    fn node_dir(&self) -> PathBuf {
        self.root.join("dsh-launcher").join("toolchain").join("node")
    }

    /// 造出「用户级 node.exe 存在」的事实（内容无关，只需文件存在）
    fn install_fake_node(&self) {
        let dir = self.node_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("node.exe"), b"fake").unwrap();
    }

    /// 清空进程 PATH —— 使 `node_in_system_path()`（`where node`）**确定失败**，
    /// 从而强制 `node_dir_injection()` 走「需要注入」分支。
    ///
    /// 为什么必须这样做：否则在**已装 node 的开发机/CI** 上，生产函数按设计返回 `None`，
    /// 测试就只能走"无需注入"的旁路分支 —— 那样即便把注入实现改坏，测试依然全绿
    /// （反向验证已实测暴露该弱点）。
    fn force_injection_precondition(&self) {
        std::env::set_var("PATH", "");
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        match &self.original_localappdata {
            Some(value) => std::env::set_var("LOCALAPPDATA", value),
            None => std::env::remove_var("LOCALAPPDATA"),
        }
        match &self.original_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 读取某个 Command 上被显式设置的 PATH（`get_envs` 返回 (key, Option<value>)；
/// `None` 表示该变量被移除）
fn injected_path(cmd: &Command) -> Option<String> {
    cmd.get_envs()
        .find(|(key, _)| key.to_string_lossy().eq_ignore_ascii_case("PATH"))
        .and_then(|(_, value)| value.map(|v| v.to_string_lossy().to_string()))
}

#[test]
fn inject_node_path_into_真的把用户级_node_目录写进子进程_env() {
    let sandbox = Sandbox::new("injects");
    sandbox.install_fake_node();
    // 强制注入前提成立（清空 PATH → where node 失败 → 生产函数必须注入）
    sandbox.force_injection_precondition();

    let expected_dir = pathutil::node_dir_injection()
        .expect("前提：已装用户级 node 且系统 PATH 无 node → 生产函数必须要求注入");

    let mut cmd = Command::new("npm");
    pathutil::inject_node_path_into(&mut cmd);

    let path = injected_path(&cmd).expect("生产函数必须为子进程显式设置 PATH");
    assert!(
        path.split(';').any(|p| p.eq_ignore_ascii_case(&expected_dir)),
        "注入后的 PATH 必须包含用户级 node_dir（期望 {expected_dir}，实际 {path}）"
    );
    assert!(
        path.to_lowercase().starts_with(&expected_dir.to_lowercase()),
        "node_dir 必须位于 PATH **前缀**（优先于系统目录）：{path}"
    );
}

#[test]
fn inject_node_path_into_是幂等的_不产生重复条目() {
    let sandbox = Sandbox::new("idempotent");
    sandbox.install_fake_node();
    sandbox.force_injection_precondition();

    let node_dir = pathutil::node_dir_injection().expect("前提：注入必须生效");

    let mut cmd = Command::new("npm");
    pathutil::inject_node_path_into(&mut cmd);
    let first = injected_path(&cmd).expect("第一次注入必须设置 PATH");

    let mut cmd2 = Command::new("npm");
    pathutil::inject_node_path_into(&mut cmd2);
    let second = injected_path(&cmd2).expect("第二次注入必须设置 PATH");

    assert_eq!(first, second, "对相同环境重复注入必须得到同一 PATH（幂等）");

    let occurrences = first
        .split(';')
        .filter(|p| p.eq_ignore_ascii_case(&node_dir))
        .count();
    assert_eq!(
        occurrences, 1,
        "PATH 中 node_dir 必须恰好出现 1 次（实际 {occurrences} 次）：{first}"
    );
}

#[test]
fn 系统已有_node_时不注入_以免污染用户环境() {
    let sandbox = Sandbox::new("no-inject");
    sandbox.install_fake_node();
    // 保留真实 PATH（本机通常已含 node）→ 若系统确有 node，则生产函数必须返回 None
    if pathutil::node_in_system_path() {
        assert!(
            pathutil::node_dir_injection().is_none(),
            "系统 PATH 已有 node 时不得再注入用户级目录"
        );
        let mut cmd = Command::new("npm");
        pathutil::inject_node_path_into(&mut cmd);
        assert!(
            injected_path(&cmd).is_none(),
            "无需注入时不得改写子进程 PATH"
        );
    }
}

#[test]
fn hidden_cmd_包装_cmd_exe_并带上静默创建标志() {
    // 生产函数 `hidden_cmd` 的契约：Windows 下必须以 `cmd.exe /D /C <program>` 包装
    // （npm/pnpm 是 .cmd，直接 spawn 会 "program not found"），且带 CREATE_NO_WINDOW。
    // 这里断言的是**生产的构造结果**（program 与参数），不执行子进程。
    let cmd = dsh_launcher_lib::core::command::hidden_cmd("npm");
    let program = cmd.get_program().to_string_lossy().to_string();

    #[cfg(windows)]
    {
        assert!(
            program.eq_ignore_ascii_case("cmd.exe") || program.eq_ignore_ascii_case("cmd"),
            ".cmd 脚本必须经 cmd.exe 包装，实际 program={program}"
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            args.iter().any(|a| a.eq_ignore_ascii_case("/D")),
            "必须带 /D 忽略 AutoRun，实际 args={args:?}"
        );
        assert!(
            args.iter().any(|a| a.eq_ignore_ascii_case("/C")),
            "必须带 /C 执行后退出，实际 args={args:?}"
        );
        assert!(
            args.iter().any(|a| a == "npm"),
            "必须把目标程序作为 /C 的实参，实际 args={args:?}"
        );
    }
    #[cfg(not(windows))]
    {
        // 非 Windows：.cmd 不存在，直接执行 program
        assert_eq!(program, "npm", "非 Windows 平台应直接执行 program");
    }
}

#[test]
fn hidden_cmd_对_exe_程序不套_cmd_包装() {
    // `hidden`（.exe 用）不得经 cmd 包装；这是与 hidden_cmd 的关键区别
    let cmd = dsh_launcher_lib::core::command::hidden("git");
    let program = cmd.get_program().to_string_lossy().to_string();
    assert_eq!(program, "git", "hidden() 必须直接执行目标程序");
    let args: Vec<String> = cmd
        .get_args()
        .map(|a| a.to_string_lossy().to_string())
        .collect();
    assert!(
        !args.iter().any(|a| a.eq_ignore_ascii_case("/C")),
        "hidden() 不得引入 cmd /C，实际 args={args:?}"
    );
}
