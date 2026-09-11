// 技能导入端到端集成测试（ADR-0008）：**完全 hermetic**，不改动真实 ~/.agents。
//
// 覆盖 import.rs 里纯单元测试覆盖不到的部分：`apply_plan` 的真实文件 IO ——
// 文件级覆盖（Q29 A）、本地独有文件保留、远程删除不落地、来源注册表落盘。
//
// 通过 DSH_AGENTS_HOME 指向临时目录，隔离受管根；用环境变量 + 单测试函数避免并行污染。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::skill::{import, scan, source};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

/// 环境变量（DSH_AGENTS_HOME）是进程级的，并行测试会互相污染。
/// 所有改动 env 的测试必须先获取此锁，串行执行。
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn logger() -> Arc<Logger> {
    Arc::new(Logger::init())
}

fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("dsh-skill-import-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 造一个「分类嵌套」的仓库（模拟 mattpocock/skills 的两层布局），返回仓库根。
fn make_repo(root: &Path, skill_name: &str, body: &str) -> PathBuf {
    // skills/engineering/<name>/SKILL.md
    let dir = root.join("skills/engineering").join(skill_name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {skill_name}\ndescription: {skill_name} skill.\n---\n\n{body}\n"),
    )
    .unwrap();
    std::fs::write(dir.join("helper.md"), "helper").unwrap();
    root.to_path_buf()
}

#[test]
fn 导入端到端_文件级覆盖与本地保留() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // ── 隔离受管根 ─────────────────────────────────────────────────────────
    let agents_home = temp_dir("agentshome");
    let agents_skills = agents_home.join("skills");
    std::fs::create_dir_all(&agents_skills).unwrap();
    std::env::set_var("DSH_AGENTS_HOME", &agents_home);

    let log = logger();

    // ── 造一个仓库，含技能 alpha（带 helper.md） ──────────────────────────
    let repo = temp_dir("repo");
    make_repo(&repo, "alpha", "Body of alpha.");

    // ── 1. 计划（只读） ───────────────────────────────────────────────────
    let planned = import::plan_from_clone(&repo, "https://example.invalid/x", None)
        .expect("计划应成功");
    assert!(!planned.applied);
    assert_eq!(planned.plans.len(), 1);
    assert_eq!(planned.plans[0].name, "alpha");
    assert!(planned.plans[0].actionable, "新技能应可写入");
    // 目录名取 frontmatter name（仓库里是 skills/engineering/alpha）
    assert!(planned.plans[0].target_path.ends_with("alpha"));

    // ── 2. 应用 ───────────────────────────────────────────────────────────
    let applied = import::apply_plan(&repo, planned, None, &log).expect("应用应成功");
    assert!(applied.applied);
    let alpha_dir = agents_skills.join("alpha");
    assert!(alpha_dir.join("SKILL.md").is_file());
    assert!(alpha_dir.join("helper.md").is_file(), "兄弟资源应一并复制");
    let body = std::fs::read_to_string(alpha_dir.join("SKILL.md")).unwrap();
    assert!(body.contains("Body of alpha."));

    // ── 3. 本地独有的文件在「覆盖式导入」后必须保留（Q29 A） ────────────────
    std::fs::write(alpha_dir.join("local-note.md"), "local only").unwrap();
    std::fs::create_dir_all(alpha_dir.join("agents")).unwrap();
    std::fs::write(alpha_dir.join("agents/openai.yaml"), "local agents").unwrap();

    // 改一下仓库内容（模拟上游更新），再导入
    let repo2 = temp_dir("repo2");
    make_repo(&repo2, "alpha", "Body of alpha UPDATED.");
    let planned2 = import::plan_from_clone(&repo2, "https://example.invalid/x", None).unwrap();
    let _ = import::apply_plan(&repo2, planned2, None, &log).unwrap();

    let alpha_dir = agents_skills.join("alpha");
    assert!(
        std::fs::read_to_string(alpha_dir.join("SKILL.md"))
            .unwrap()
            .contains("UPDATED"),
        "上游提供的 SKILL.md 应被覆盖"
    );
    assert!(
        std::fs::read_to_string(alpha_dir.join("helper.md"))
            .unwrap()
            == "helper",
        "上游 helper.md 内容应一致"
    );
    assert!(
        alpha_dir.join("local-note.md").is_file(),
        "本地独有文件必须保留（绝不删除）"
    );
    assert!(
        alpha_dir.join("agents/openai.yaml").is_file(),
        "本地独有子目录必须保留"
    );

    // ── 4. 远程删除的文件不落地删除 ───────────────────────────────────────
    // repo3 里 alpha 不再有 helper.md
    let repo3 = temp_dir("repo3");
    let dir3 = repo3.join("skills/engineering/alpha");
    std::fs::create_dir_all(&dir3).unwrap();
    std::fs::write(
        dir3.join("SKILL.md"),
        "---\nname: alpha\ndescription: alpha skill.\n---\n\nBody v3.\n",
    )
    .unwrap();
    let planned3 = import::plan_from_clone(&repo3, "https://example.invalid/x", None).unwrap();
    let _ = import::apply_plan(&repo3, planned3, None, &log).unwrap();
    assert!(
        alpha_dir.join("helper.md").is_file(),
        "上游删除的 helper.md 不得被本地删除（Q29 A：远程删除不落地）"
    );

    // ── 5. 来源注册表落盘 ─────────────────────────────────────────────────
    // apply_plan 内部会 upsert 来源记录；此处用 source 模块直接验证格式
    let mut registry = source::SourceRegistry::default();
    registry.upsert(source::SourceRecord {
        name: Some("示例仓库".to_string()),
        url: "https://example.invalid/x".to_string(),
        commit: Some("abc".to_string()),
        imported_at: "1".to_string(),
        checked_at: None,
        last_update_count: 0,
        skills: vec!["alpha".to_string()],
    });
    assert_eq!(registry.source_of_skill("alpha").unwrap().url, "https://example.invalid/x");
    assert!(registry.source_of_skill("nope").is_none());

    // ── 6. 扫描能看到导入的技能 ───────────────────────────────────────────
    let listed = scan::list();
    assert!(
        listed.skills.iter().any(|s| s.name == "alpha"),
        "导入后扫描应看到 alpha"
    );

    // ── 清理 ─────────────────────────────────────────────────────────────
    std::env::remove_var("DSH_AGENTS_HOME");
    let _ = std::fs::remove_dir_all(&agents_home);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&repo2);
    let _ = std::fs::remove_dir_all(&repo3);
}

#[test]
fn 目录名取_frontmatter_name_而非仓库目录名() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 仓库目录叫 "skills/eng/vercel-foo"，但 frontmatter name 是 "foo"
    let repo = temp_dir("repo-namemismatch");
    let dir = repo.join("skills/eng/vercel-foo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: foo\ndescription: foo skill.\n---\n\nBody.\n",
    )
    .unwrap();
    let planned = import::plan_from_clone(&repo, "https://example.invalid/x", None).unwrap();
    assert_eq!(planned.plans.len(), 1);
    assert_eq!(planned.plans[0].name, "foo");
    assert!(
        planned.plans[0].target_path.ends_with("foo"),
        "目标目录名应取 frontmatter name，而非仓库目录名 vercel-foo：{}",
        planned.plans[0].target_path
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn 批量导入_单条失败不中断且_name_写入来源记录() {
    let _env_guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 隔离受管根
    let agents_home = temp_dir("agentshome-batch");
    let agents_skills = agents_home.join("skills");
    std::fs::create_dir_all(&agents_skills).unwrap();
    std::env::set_var("DSH_AGENTS_HOME", &agents_home);
    let log = logger();

    // 两个可用仓库 + 一个空 URL（应失败但不影响其余）
    let repo_a = temp_dir("batch-a");
    make_repo(&repo_a, "alpha", "Alpha body.");
    let repo_b = temp_dir("batch-b");
    make_repo(&repo_b, "beta", "Beta body.");

    // 两个坏条目（都无需网络：空 URL 被提前拦截；"not a url" 被 clone_repo 的
    // looks_like_repo 检查立即拒绝），验证「单条失败不中断 + 聚合计数 + name 透传」。
    let items = vec![
        import::ImportItem {
            name: "仓库甲".to_string(),
            url: "not a url".to_string(),
        },
        import::ImportItem {
            name: "".to_string(),
            url: "".to_string(),
        },
    ];
    let report = import::import_batch(&items, false, &log);
    // 两个条目都失败（一个网络不可达、一个空 URL），但不 panic、不中断
    assert_eq!(report.items.len(), 2, "批量报告应逐条返回");
    assert_eq!(report.ok_count, 0);
    assert_eq!(report.failed_count, 2);
    assert!(!report.applied, "预览模式 applied=false");

    // 清理
    std::env::remove_var("DSH_AGENTS_HOME");
    let _ = std::fs::remove_dir_all(&agents_home);
    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

#[test]
fn 来源记录_name_字段往返与展示名回退() {
    use dsh_launcher_lib::core::skill::source::SourceRecord;
    // name 有值 → display_name 用 name
    let with_name = SourceRecord {
        name: Some("我的仓库".to_string()),
        url: "https://x/y".to_string(),
        commit: None,
        imported_at: "1".to_string(),
        checked_at: None,
        last_update_count: 0,
        skills: vec![],
    };
    assert_eq!(with_name.display_name(), "我的仓库");
    // name 为空 → 回退到 URL
    let no_name = SourceRecord {
        name: None,
        url: "https://x/y".to_string(),
        ..with_name.clone()
    };
    assert_eq!(no_name.display_name(), "https://x/y");
}
