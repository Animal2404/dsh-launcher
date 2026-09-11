// 技能写操作集成测试（ADR-0007）：**完全 hermetic**（不改动真实 ~/.agents）。
//
// 通过 DSH_HOME / DSH_AGENTS_HOME 环境变量把受管根指向临时目录，从而端到端覆盖
// `manage::set_enabled` / `manage::delete` 的真实文件 IO 路径 —— 备份、原子写、复验、
// 身份校验、回收站 —— 这些是纯函数单元测试覆盖不到的部分。
//
// 注意：环境变量是进程级的，故本文件**只放一个测试函数**，避免并行测试互相污染。

use dsh_launcher_lib::core::logging::Logger;
use dsh_launcher_lib::core::skill::{manage, scan, SkillError};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 构造一个合法的技能文件内容（CRLF，与真实数据一致）
fn skill_text(name: &str, extra: &str) -> String {
    format!("---\r\nname: {name}\r\ndescription: Test skill for {name}.\r\n{extra}---\r\n\r\nBody of {name}.\r\n")
}

fn write_skill(dir: &Path, name: &str, extra: &str) -> PathBuf {
    let skill_dir = dir.join(name);
    std::fs::create_dir_all(&skill_dir).unwrap();
    let file = skill_dir.join("SKILL.md");
    std::fs::write(&file, skill_text(name, extra)).unwrap();
    file
}

/// 临时目录（不引入 tempfile 依赖：用进程 id + 纳秒构造唯一路径）
fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("dsh-skill-test-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn logger() -> Arc<Logger> {
    // 集成测试无 Tauri AppHandle；`Logger::init()` 是既有测试的统一构造方式
    // （事件推送在无 handle 时静默丢弃）。
    Arc::new(Logger::init())
}

#[test]
fn 技能写操作端到端() {
    // ── 隔离环境：受管根指向临时目录 ──────────────────────────────────────
    let dsh_home = temp_dir("dshhome");
    let agents_home = temp_dir("agentshome");
    let agents_skills = agents_home.join("skills");
    let dsh_skills = dsh_home.join("skills");
    std::fs::create_dir_all(&agents_skills).unwrap();
    std::fs::create_dir_all(&dsh_skills).unwrap();
    std::env::set_var("DSH_HOME", &dsh_home);
    std::env::set_var("DSH_AGENTS_HOME", &agents_home);

    // 备份根也隔离（避免污染真实 LOCALAPPDATA）
    let backup_home = temp_dir("localappdata");
    std::env::set_var("LOCALAPPDATA", &backup_home);

    let log = logger();

    // ── 1. 扫描：两个根都被识别，条目状态正确 ──────────────────────────────
    let a = write_skill(&agents_skills, "alpha", "");
    // beta 初始即停用：用于验证扫描把已停用技能读成 disabled
    write_skill(&dsh_skills, "beta", "disable-model-invocation: true\r\n");
    // 同名技能放在低 rank 根（400）里，应被 rank 500 的同名者覆盖
    let a_low = write_skill(&dsh_skills, "alpha", "");
    // 平铺 .md 技能
    std::fs::write(dsh_skills.join("flat-one.md"), skill_text("flat-one", "")).unwrap();

    let listed = scan::list();
    let names: Vec<&str> = listed.skills.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"alpha"), "应含 alpha：{names:?}");
    assert!(names.contains(&"beta"), "应含 beta：{names:?}");
    assert!(names.contains(&"flat-one"), "应含平铺技能：{names:?}");
    assert_eq!(listed.roots.len(), 2, "应有两个受管根");
    assert_eq!(listed.disabled_count, 1, "beta 应为已停用");

    // rank 500 的 alpha 生效；rank 400 的 alpha 被覆盖
    let alpha_agents = listed
        .skills
        .iter()
        .find(|s| s.name == "alpha" && s.path == a.display().to_string())
        .expect("应找到 agentsHome 的 alpha");
    let alpha_dsh = listed
        .skills
        .iter()
        .find(|s| s.name == "alpha" && s.path == a_low.display().to_string())
        .expect("应找到 DSH_HOME 的 alpha");
    assert_eq!(alpha_agents.rank, 500);
    assert_eq!(alpha_dsh.rank, 400);
    assert!(
        alpha_agents.overridden_by.is_none(),
        "rank 500 的 alpha 应生效"
    );
    assert_eq!(
        alpha_dsh.overridden_by.as_deref(),
        Some("alpha"),
        "rank 400 的 alpha 应被同名高 rank 覆盖"
    );

    // ── 2. 停用：写入 true，原字节被备份 ──────────────────────────────────
    let original_a = std::fs::read_to_string(&a).unwrap();
    let report = manage::set_enabled(&a, "alpha", false, &log).expect("停用应成功");
    assert!(report.changed, "首次停用应发生写入");
    let after_disable = std::fs::read_to_string(&a).unwrap();
    assert!(after_disable.contains("disable-model-invocation: true\r\n"));
    assert_eq!(
        scan::frontmatter_state(&after_disable),
        "disabled",
        "写入后应为停用"
    );
    // 备份存在且内容等于原始字节
    let backup = report.backup.expect("应返回备份路径");
    assert_eq!(
        std::fs::read_to_string(&backup).unwrap(),
        original_a,
        "备份必须逐字节等于写入前内容"
    );

    // ── 3. 幂等：再次停用不写盘 ───────────────────────────────────────────
    let again = manage::set_enabled(&a, "alpha", false, &log).expect("重复停用应成功");
    assert!(!again.changed, "已是停用状态应返回 changed=false");

    // ── 4. 启用：删键并逐字节回到原状 ─────────────────────────────────────
    let enabled = manage::set_enabled(&a, "alpha", true, &log).expect("启用应成功");
    assert!(enabled.changed, "从停用切启用应写入");
    assert_eq!(
        std::fs::read_to_string(&a).unwrap(),
        original_a,
        "停用 → 启用 必须逐字节回到原状（ADR-0007 不变量 2）"
    );

    // ── 5. 身份校验：name 不符即拒绝 ──────────────────────────────────────
    let mismatch = manage::set_enabled(&a, "not-alpha", false, &log);
    match mismatch {
        Err(SkillError::NotManaged(detail)) => {
            assert!(detail.contains("已变为"), "应报告名字已变：{detail}");
        }
        other => panic!("name 不符应被拒绝，实际 {other:?}"),
    }
    // 拒绝后文件必须未被改动
    assert_eq!(std::fs::read_to_string(&a).unwrap(), original_a);

    // ── 6. 身份校验：路径不在受管根内即拒绝 ───────────────────────────────
    let outside = temp_dir("outside").join("gamma");
    std::fs::create_dir_all(&outside).unwrap();
    let outside_file = outside.join("SKILL.md");
    std::fs::write(&outside_file, skill_text("gamma", "")).unwrap();
    match manage::set_enabled(&outside_file, "gamma", false, &log) {
        Err(SkillError::NotManaged(detail)) => {
            assert!(detail.contains("不在受管的用户级技能根内"), "实际：{detail}");
        }
        other => panic!("根外路径应被拒绝，实际 {other:?}"),
    }

    // ── 7. 拒绝歧义：非裸布尔不可改写 ────────────────────────────────────
    let weird = write_skill(&agents_skills, "weird", "disable-model-invocation: yes\r\n");
    match manage::set_enabled(&weird, "weird", false, &log) {
        Err(SkillError::Frontmatter(_)) => {}
        other => panic!("非裸布尔应被拒绝，实际 {other:?}"),
    }
    // 拒绝后文件字节不变
    assert!(std::fs::read_to_string(&weird)
        .unwrap()
        .contains("disable-model-invocation: yes"));

    // ── 8. 删除：移入回收站且原目录消失 ──────────────────────────────────
    let delete_target = write_skill(&agents_skills, "doomed", "");
    let doomed_dir = delete_target.parent().unwrap().to_path_buf();
    let report = manage::delete(&delete_target, "doomed", &log).expect("删除应成功");
    assert!(!doomed_dir.exists(), "原技能目录应消失");
    assert!(
        Path::new(&report.trashed_to).is_dir(),
        "回收站目标应存在：{}",
        report.trashed_to
    );
    // 回收站位于**所属根**之下
    assert!(
        report
            .trashed_to
            .starts_with(&agents_skills.display().to_string()),
        "回收站应在所属根内：{}",
        report.trashed_to
    );
    // 回收站里的内容完好
    assert!(Path::new(&report.trashed_to).join("SKILL.md").is_file());

    // ── 9. 删除后重扫：该技能不再出现在列表中 ────────────────────────────
    let after_delete = scan::list();
    assert!(
        !after_delete.skills.iter().any(|s| s.name == "doomed"),
        "已删除技能不应再被列出"
    );
    assert!(after_delete.trash_count >= 1, "回收站应有残留计数");
    // 回收站目录本身不被当作技能
    assert!(
        !after_delete.skills.iter().any(|s| s.name == ".trash"),
        "回收站不应被识别为技能"
    );

    // ── 10. 平铺 .md 技能也能启停 ────────────────────────────────────────
    let flat = dsh_skills.join("flat-one.md");
    let flat_report = manage::set_enabled(&flat, "flat-one", false, &log).expect("平铺技能应可停用");
    assert!(flat_report.changed);
    assert!(std::fs::read_to_string(&flat)
        .unwrap()
        .contains("disable-model-invocation: true\r\n"));

    // ── 清理 ─────────────────────────────────────────────────────────────
    std::env::remove_var("DSH_HOME");
    std::env::remove_var("DSH_AGENTS_HOME");
    std::env::remove_var("LOCALAPPDATA");
    let _ = std::fs::remove_dir_all(&dsh_home);
    let _ = std::fs::remove_dir_all(&agents_home);
    let _ = std::fs::remove_dir_all(&backup_home);
    let _ = std::fs::remove_dir_all(outside.parent().unwrap());
}
