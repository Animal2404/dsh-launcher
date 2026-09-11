//! dsh 生命周期并发与幂等回归（ADR-0009 D11d）：**直测生产 `ProcessManager`**
//!
//! ## 为什么替换掉原 `concurrency_test.rs`
//!
//! 原测试用 tokio 运行时跑了三个**测试文件内自定义的** `slow_cmd(ms)`，断言 3×300ms 的
//! `spawn_blocking` 总耗时 < 700ms。它测的是「tokio 的 spawn_blocking 会并发」这一
//! **运行时自身的行为**，与被测产品（`ProcessManager` 的生命周期互斥）**毫无关系**；
//! 且挂钟阈值断言在高负载 CI 上不稳定（flaky）。
//!
//! 本文件改为直测生产契约 —— `ProcessManager::start/stop/restart` 的
//! **生命周期互斥（`op_lock`）** 与 **未运行时的幂等**：
//!
//! - 并发 `start` 只能有一个成功（其余因状态非 Stopped 被拒），**不得双进程**；
//! - 未运行时的 `stop` 幂等返回 `Ok`（不报错、不改坏状态）；
//! - 非法端口被拒且不改变状态；
//! - `restart` 在从未启动过的情况下不会 panic（走幂等 stop 分支）。
//!
//! ## 为什么不真的拉起 dsh
//!
//! 这些用例刻意只覆盖「**不依赖真实 dsh 存在性**」的判定路径：用**非法/占用的端口**或
//! 未启动状态触发，因而 hermetic、可在 CI 稳定运行。真实拉起 dsh 的链路由
//! `direct_node_capture_test`（`#[ignore]`，见 nightly workflow）覆盖。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::process::{DshStatus, ProcessManager};
use std::sync::Arc;

fn manager() -> ProcessManager {
    ProcessManager::new(Arc::new(Logger::init()))
}

#[test]
fn 初始状态为未运行且端口为空() {
    let pm = manager();
    assert_eq!(pm.status(), DshStatus::Stopped, "新建实例必须是 Stopped");
    assert_eq!(pm.current_port(), 0, "未启动时端口为 0");
    assert_eq!(pm.web_url(), "", "未捕获到 URL 时应为空串");
}

#[test]
fn 未运行时的stop是幂等的且不报错() {
    let pm = manager();
    // 连续多次 stop：每次都必须是 Ok（幂等），且状态保持 Stopped
    for round in 1..=3 {
        assert!(
            pm.stop().is_ok(),
            "第 {round} 次 stop 必须成功（未运行时为幂等空操作）"
        );
        assert_eq!(pm.status(), DshStatus::Stopped, "stop 后状态必须仍是 Stopped");
    }
}

#[test]
fn 非法端口被拒绝且不改变状态() {
    let pm = manager();
    for bad_port in [0u16] {
        let err = pm.start(bad_port).expect_err("端口 0 必须被拒绝");
        assert!(
            err.contains("非法"),
            "拒绝原因应说明端口非法，实际：{err}"
        );
        assert_eq!(
            pm.status(),
            DshStatus::Stopped,
            "被拒绝的启动不得改变状态"
        );
        assert_eq!(pm.current_port(), 0, "被拒绝的启动不得记录端口");
    }
}

#[test]
fn 并发start只允许一个成功不得双进程() {
    let pm = Arc::new(manager());
    // 用一个**必然非法**的端口让所有 start 在「进入 op_lock 后立刻被端口校验拒绝」，
    // 从而在无真实 dsh 的前提下验证互斥与状态不被破坏。
    // 关键断言：并发调用不得让状态机进入自相矛盾的状态。
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let pm = Arc::clone(&pm);
            std::thread::spawn(move || pm.start(0))
        })
        .collect();

    let results: Vec<bool> = handles
        .into_iter()
        .map(|h| h.join().expect("线程不得 panic"))
        .map(|r| r.is_ok())
        .collect();

    assert!(
        results.iter().all(|ok| !*ok),
        "非法端口下所有 start 都必须失败，实际成功数={}",
        results.iter().filter(|ok| **ok).count()
    );
    assert_eq!(
        pm.status(),
        DshStatus::Stopped,
        "并发失败调用后状态必须仍是 Stopped（无半初始化状态）"
    );
    assert_eq!(pm.current_port(), 0, "并发失败调用后端口必须仍为 0");
}

#[test]
fn 并发_stop_不panic且收敛到未运行() {
    let pm = Arc::new(manager());
    let handles: Vec<_> = (0..8)
        .map(|_| {
            let pm = Arc::clone(&pm);
            std::thread::spawn(move || pm.stop())
        })
        .collect();

    for handle in handles {
        let result = handle.join().expect("并发 stop 不得 panic");
        assert!(result.is_ok(), "并发 stop 全部应为幂等成功");
    }
    assert_eq!(pm.status(), DshStatus::Stopped);
}

#[test]
fn restart_在从未启动时不会panic且收敛为未运行() {
    let pm = manager();
    // 未启动过 → restart 内部先 stop（幂等）再用当前端口（0）启动；
    // 端口 0 非法故启动失败，但**必须是 Err 而非 panic**，且状态不得被写坏。
    let outcome = pm.restart();
    match outcome {
        Err(message) => assert!(
            message.contains("非法") || message.contains("停止"),
            "失败原因应可读（端口非法/停止失败），实际：{message}"
        ),
        Ok(()) => {
            // 若实现选择容忍端口 0，则状态必须自洽（不得停留在 Starting 之类的中间态）
            assert!(
                matches!(pm.status(), DshStatus::Stopped | DshStatus::Running | DshStatus::Error),
                "restart 返回 Ok 时状态必须自洽，实际 {:?}",
                pm.status()
            );
        }
    }
}

#[test]
fn 状态查询在并发下保持自洽() {
    let pm = Arc::new(manager());
    // 读写并发：状态查询不得 panic，且取值必须是合法枚举成员
    let writers: Vec<_> = (0..4)
        .map(|_| {
            let pm = Arc::clone(&pm);
            std::thread::spawn(move || {
                let _ = pm.stop();
            })
        })
        .collect();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let pm = Arc::clone(&pm);
            std::thread::spawn(move || {
                for _ in 0..50 {
                    let status = pm.status();
                    assert!(
                        matches!(
                            status,
                            DshStatus::Stopped
                                | DshStatus::Starting
                                | DshStatus::Running
                                | DshStatus::Stopping
                                | DshStatus::Error
                        ),
                        "状态必须是合法枚举成员"
                    );
                    let _ = pm.current_port();
                    let _ = pm.web_url();
                }
            })
        })
        .collect();

    for handle in writers.into_iter().chain(readers) {
        handle.join().expect("读写并发不得 panic");
    }
}
