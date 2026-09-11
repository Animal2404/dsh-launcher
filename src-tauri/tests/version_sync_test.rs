//! 版本一致性防回归（ADR-0009 D1/D2）
//!
//! 与 `scripts/check-version-sync.mjs` **互为独立实现**：脚本用正则解析，本测试用
//! 逐行/JSON 解析。两者独立的意义是——若脚本自身的解析写错（例如正则漏配某一处），
//! 本测试仍能独立发现漂移，不会出现"两侧同时放过"。
//!
//! 背景：本仓库曾出现 `package-lock.json` 停在 `0.7.1`、其余四处 `0.9.0` 的漂移，
//! 直到发版时 `bump-version.mjs` 的一致性保护 `exit(1)` 才暴露，直接阻断发布。
//! 本测试让该漂移在 `cargo test` 阶段即可见。

use std::path::{Path, PathBuf};

/// 仓库根（`src-tauri/..`）
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 应有上级目录")
        .to_path_buf()
}

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap_or_else(|e| panic!("读取 {} 失败: {e}", file.display()))
}

/// 从 `key = "value"` 形态的行中取值（逐行匹配，不依赖正则）
fn line_value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let trimmed = line.trim();
        let rest = trimmed.strip_prefix(key)?.trim_start();
        let rest = rest.strip_prefix('=')?.trim();
        Some(rest.trim_matches('"').to_string())
    })
}

/// Cargo.lock 中 `name = "dsh-launcher"` 段紧随的 `version`
fn cargo_lock_version(content: &str) -> Option<String> {
    let mut lines = content.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() == "name = \"dsh-launcher\"" {
            let next = lines.peek()?;
            return line_value(next, "version");
        }
    }
    None
}

#[test]
fn 版本号在五文件六落点保持一致() {
    let root = repo_root();

    let pkg: serde_json::Value =
        serde_json::from_str(&read(&root.join("package.json"))).expect("package.json 应为合法 JSON");
    let lock: serde_json::Value = serde_json::from_str(&read(&root.join("package-lock.json")))
        .expect("package-lock.json 应为合法 JSON");
    let tauri: serde_json::Value = serde_json::from_str(&read(&root.join("src-tauri/tauri.conf.json")))
        .expect("tauri.conf.json 应为合法 JSON");

    let points: Vec<(&str, Option<String>)> = vec![
        (
            "package.json",
            pkg.get("version").and_then(|v| v.as_str()).map(str::to_string),
        ),
        (
            "package-lock.json (root version)",
            lock.get("version").and_then(|v| v.as_str()).map(str::to_string),
        ),
        (
            "package-lock.json (packages[''])",
            lock.get("packages")
                .and_then(|p| p.get(""))
                .and_then(|p| p.get("version"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
        ),
        (
            "src-tauri/Cargo.toml",
            line_value(&read(&root.join("src-tauri/Cargo.toml")), "version"),
        ),
        (
            "src-tauri/tauri.conf.json",
            tauri.get("version").and_then(|v| v.as_str()).map(str::to_string),
        ),
        (
            "src-tauri/Cargo.lock",
            cargo_lock_version(&read(&root.join("src-tauri/Cargo.lock"))),
        ),
    ];

    // 任一落点缺失即失败（不允许静默跳过）
    for (label, value) in &points {
        assert!(value.is_some(), "版本落点 {label} 未解析到版本号");
    }

    let distinct: std::collections::BTreeSet<&str> =
        points.iter().filter_map(|(_, v)| v.as_deref()).collect();

    assert_eq!(
        distinct.len(),
        1,
        "版本号不同步（ADR-0009 D1）。请以 package.json 为权威对齐后重跑；\n实际值: {points:#?}"
    );

    // 与 package.json 显式对照（可读性：失败时直接看到期望值）
    let expected = pkg.get("version").and_then(|v| v.as_str()).unwrap_or_default();
    for (label, value) in &points {
        assert_eq!(
            value.as_deref(),
            Some(expected),
            "版本落点 {label} 与 package.json ({expected}) 不一致"
        );
    }
}
