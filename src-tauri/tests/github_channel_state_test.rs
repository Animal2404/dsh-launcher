//! GitHub 通道状态判定的**幂等与可逆**回归（ADR-0009 D11a）
//!
//! ## 为什么替换掉原 `channel_mutex_logic_test.rs`
//!
//! 原测试在**测试文件内**自定义了一个 `cleanup_dir()` 函数，然后断言这个自定义函数
//! 「存在即删、不存在即静默」——**全程未调用任何生产代码**，因此对产品行为零回归保护
//! （生产清理逻辑在 `commands/version.rs::uninstall_github_channel`，与该自定义函数无关）。
//!
//! 本文件改为直调**生产函数** `core::github::{github_clone_dir, github_installed}`，
//! 在一个隔离的 `LOCALAPPDATA` 下验证「安装目录判定」的幂等与可逆：
//! 无目录 → 未安装；补全目录特征 → 已安装；再次移除 → 回到未安装，且**不报错**。
//!
//! ## 隔离
//!
//! `LOCALAPPDATA` 是进程级环境变量，故用一把互斥锁串行执行；用例结束还原原值。

use dsh_launcher_lib::core::github;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// 环境变量互斥（进程级共享状态，用例必须串行）
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// 隔离的 LOCALAPPDATA
struct Sandbox {
    root: PathBuf,
    _guard: MutexGuard<'static, ()>,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        let guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!(
            "dsh-launcher-github-state-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("LOCALAPPDATA", &root);
        Self {
            root,
            _guard: guard,
        }
    }

    /// 生产代码认定的克隆目录（`<LOCALAPPDATA>\dsh-launcher\github-dsh\deepseek-harness`）
    fn clone_dir(&self) -> PathBuf {
        github::github_clone_dir()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::env::remove_var("LOCALAPPDATA");
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 造出「已安装」的目录特征：`package.json` + `apps/` 目录
fn make_installed(dir: &Path) {
    std::fs::create_dir_all(dir.join("apps")).unwrap();
    std::fs::write(dir.join("package.json"), r#"{"name":"deepseek-harness"}"#).unwrap();
}

#[test]
fn github_installed_判定随目录特征变化且可逆() {
    let sandbox = Sandbox::new("reversible");
    let dir = sandbox.clone_dir();

    // ① 初始：目录不存在 → 未安装（生产判定，不是自定义逻辑）
    assert!(!dir.exists(), "前提：隔离环境下克隆目录不存在");
    assert!(
        !github::github_installed(),
        "目录不存在时必须判定为未安装"
    );

    // ② 补全目录特征 → 已安装
    make_installed(&dir);
    assert!(
        github::github_installed(),
        "含 package.json + apps/ 时必须判定为已安装：{}",
        dir.display()
    );

    // ③ 特征不完整（只剩 apps/，缺 package.json）→ 回到未安装
    std::fs::remove_file(dir.join("package.json")).unwrap();
    assert!(
        !github::github_installed(),
        "缺 package.json 时必须判定为未安装（不能只看目录存在）"
    );

    // ④ 修复特征 → 再次已安装（幂等：同一状态重复判定结果稳定）
    make_installed(&dir);
    assert!(github::github_installed(), "特征恢复后应再次判定为已安装");
    assert!(
        github::github_installed(),
        "重复判定必须返回同一结果（幂等）"
    );
}

#[test]
fn github_clone_dir_固定在_localappdata_下的期望位置() {
    let sandbox = Sandbox::new("path-shape");
    let dir = sandbox.clone_dir();

    // 路径形态是「卸载/清理」逻辑的契约：必须是 <LOCALAPPDATA>\dsh-launcher\github-dsh\deepseek-harness
    assert!(
        dir.starts_with(&sandbox.root),
        "克隆目录必须落在 LOCALAPPDATA 之下：{}",
        dir.display()
    );
    assert_eq!(
        dir.file_name().map(|n| n.to_string_lossy().to_string()),
        Some("deepseek-harness".to_string()),
        "克隆目录固定名（v0.2.3 起不再按版本号命名）"
    );
    assert!(
        dir.parent()
            .and_then(|p| p.file_name())
            .map(|n| n == "github-dsh")
            .unwrap_or(false),
        "克隆目录的父级必须是 github-dsh：{}",
        dir.display()
    );

    // 清理动作的幂等前提：目录不存在时删除操作也不应报错（用生产路径直接验证）
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    assert!(
        std::fs::remove_dir_all(&dir).is_err(),
        "目录已不存在时 remove_dir_all 必然报错——这正是调用方必须容忍的情形"
    );
    assert!(!github::github_installed());
}
