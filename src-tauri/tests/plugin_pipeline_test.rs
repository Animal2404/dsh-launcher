//! 插件管理集成测试（跨模块，不依赖真实 dsh）
//!
//! 覆盖 ADR-0005 的可离线验证部分：
//! 1. dump 解析 → 状态派生 → 非法转换拒绝（纯函数链路）；
//! 2. 受管区块写入/幂等/块外保留/卸载清理（managed + 文件系统）；
//! 3. 注册表持久化与「磁盘为事实源」的清理语义（registry + 文件系统）。

use dsh_launcher_lib::core::plugin::dump::{self, DisabledValue};
use dsh_launcher_lib::core::plugin::managed::{self, BlockOutcome, ManagedEntry};
use dsh_launcher_lib::core::plugin::registry::{PluginRecord, Registry};
use dsh_launcher_lib::core::plugin::state::{self, PluginAction, PluginErrorKind, PluginState, RowState};

/// 真实 dump 片段（取自本机 `dsh --profile web --dump-config`）
const DUMP: &str = r#"# == @deepseek-ai/dsh-base
- id: timer
  name: '@deepseek-ai/cordis-plugin-timer'
- id: hmr
  name: '@deepseek-ai/cordis-plugin-hmr'
  disabled: true
  config:
    root:
      - .
- id: bash-sandbox
  name: '@deepseek-ai/dsh-bash-sandbox'
  disabled: !!js process.platform === 'win32'
# == dsh-cost-meter
- id: cost-meter
  name: dsh-cost-meter
# == dshmarket
- id: dsh-market
  name: dshmarket
  disabled: true
# == @linxin666/dsh-client-ui-skill-explorer
- id: ui-skill-explorer
  name: '@linxin666/dsh-client-ui-skill-explorer'
"#;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "dsh-launcher-it-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn dump_to_state_pipeline() {
    let sections = dump::parse_dump(DUMP).expect("dump 必须可解析");
    let index = dump::index_by_id(&sections);

    // 表达式控制的行：状态未知，启停必须被拒绝
    let bash = index.get("bash-sandbox").unwrap();
    assert!(bash.1.is_expression_controlled());
    assert_eq!(bash.1.effective_enabled(), None);

    // dshmarket 默认禁用 → disabled
    let market_rows = dump::rows_of_owner(&sections, "dshmarket");
    assert_eq!(market_rows, vec!["dsh-market"]);
    let market_state = state::derive_state(
        true,
        true,
        true,
        &[RowState::Disabled],
    );
    assert_eq!(market_state, PluginState::Disabled);
    assert!(state::validate(PluginAction::Enable, market_state, &[RowState::Disabled]).is_ok());

    // cost-meter 无 disabled → enabled
    let cost_state = state::derive_state(true, true, true, &[RowState::Enabled]);
    assert_eq!(cost_state, PluginState::Enabled);
    // 重复启用 → 合法（服务层返回 unchanged）
    assert!(state::validate(PluginAction::Enable, cost_state, &[RowState::Enabled]).is_ok());
    // 对未安装插件启停 → 非法
    let err = state::validate(PluginAction::Disable, PluginState::Uninstalled, &[]).unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
    // 对全表达式行启停 → 非法
    let err = state::validate(PluginAction::Enable, PluginState::Enabled, &[RowState::Expression])
        .unwrap_err();
    assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
}

#[test]
fn managed_block_lifecycle_on_profile_patch() {
    let dir = temp_dir("profile");
    let patch = dir.join("cordis.patch.yml");
    // dsh 初始化 profile 时的模板内容
    let header = "# Your patch layer for this dsh profile, applied after every bundle layer:\n";
    std::fs::write(&patch, format!("{header}[]\n")).unwrap();

    // 禁用 dshmarket（其唯一行 dsh-market）
    let disable = vec![ManagedEntry::new("dsh-market", true, Some("dshmarket".into()))];
    assert_eq!(managed::apply_block(&patch, &disable).unwrap(), BlockOutcome::Written);
    let content = std::fs::read_to_string(&patch).unwrap();
    // 注释保留；`[]` 占位符必须被替换（否则是两个 YAML 文档，dsh 解析失败）
    assert!(content.starts_with(header), "块外注释必须保留");
    assert!(!content.lines().any(|line| line.trim() == "[]"), "{content}");
    assert!(content.contains("- id: dsh-market"));
    assert!(content.contains("  disabled: true"));

    // 幂等：重复禁用不落盘
    let before = std::fs::read(&patch).unwrap();
    assert_eq!(managed::apply_block(&patch, &disable).unwrap(), BlockOutcome::Unchanged);
    assert_eq!(std::fs::read(&patch).unwrap(), before);

    // 再启用：写 disabled: false
    let enable = vec![ManagedEntry::new("dsh-market", false, Some("dshmarket".into()))];
    assert_eq!(managed::apply_block(&patch, &enable).unwrap(), BlockOutcome::Written);
    let content = std::fs::read_to_string(&patch).unwrap();
    assert!(content.contains("  disabled: false"));
    assert!(!content.contains("  disabled: true"));

    // 追加第二个插件，验证字典序与共存
    let both = managed::upsert(
        managed::read_block(&patch).unwrap(),
        &[ManagedEntry::new("cost-meter", true, Some("dsh-cost-meter".into()))],
        &[],
    );
    assert_eq!(managed::apply_block(&patch, &both).unwrap(), BlockOutcome::Written);
    let content = std::fs::read_to_string(&patch).unwrap();
    assert!(content.find("cost-meter").unwrap() < content.find("dsh-market").unwrap());

    // 卸载 dsh-cost-meter：只清理它的行，dshmarket 的受管条目保留
    let pruned = managed::upsert(
        managed::read_block(&patch).unwrap(),
        &[],
        &["cost-meter".to_string()],
    );
    assert_eq!(managed::apply_block(&patch, &pruned).unwrap(), BlockOutcome::Written);
    let content = std::fs::read_to_string(&patch).unwrap();
    assert!(!content.contains("cost-meter"));
    assert!(content.contains("dsh-market"));
    // 用户原始注释仍在
    assert!(content.starts_with(header));

    // 全部清空 → 区块删除且文件仍是合法空 patch
    assert_eq!(managed::apply_block(&patch, &[]).unwrap(), BlockOutcome::Written);
    let content = std::fs::read_to_string(&patch).unwrap();
    assert!(!content.contains("dsh-launcher managed"));
    assert!(content.trim_end().ends_with("[]"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn managed_block_rejects_broken_markers_and_bad_ids() {
    let dir = temp_dir("broken");
    let patch = dir.join("cordis.patch.yml");
    std::fs::write(
        &patch,
        "# >>> dsh-launcher managed v1 — 由启动器维护，请勿手工编辑 >>>\n- id: a\n",
    )
    .unwrap();
    let err = managed::apply_block(&patch, &[ManagedEntry::new("a", true, None)]).unwrap_err();
    assert!(err.contains("不成对") || err.contains("缺少结束"), "{err}");
    // 非法行 id 必须拒绝
    let err = managed::apply_block(&patch, &[ManagedEntry::new("bad id", true, None)]).unwrap_err();
    assert!(err.contains("非法行 id"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn registry_persists_origin_and_prunes_missing() {
    let dir = temp_dir("registry");
    let path = dir.join("plugins.json");

    let mut registry = Registry::empty("web");
    let mut record = PluginRecord::new("dsh-cost-meter");
    record.origin = dsh_launcher_lib::core::plugin::spec::Origin::Upstream;
    record.rows = vec!["cost-meter".to_string()];
    record.desired = Some("enabled".to_string());
    registry.upsert(record);
    let mut local = PluginRecord::new("my-plugin");
    local.origin = dsh_launcher_lib::core::plugin::spec::Origin::InHouse;
    registry.upsert(local);
    registry.save_to(&path).unwrap();

    let loaded = Registry::load_from(&path, "web");
    assert_eq!(loaded.plugins.len(), 2);
    assert_eq!(
        loaded.find("my-plugin").unwrap().origin,
        dsh_launcher_lib::core::plugin::spec::Origin::InHouse
    );

    // 磁盘为事实源：清理掉不再存在的依赖记录
    let mut pruned = loaded.clone();
    pruned.plugins.retain(|item| item.package == "dsh-cost-meter");
    pruned.save_to(&path).unwrap();
    let reloaded = Registry::load_from(&path, "web");
    assert_eq!(reloaded.plugins.len(), 1);
    assert!(reloaded.find("my-plugin").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dump_expression_disabled_is_readonly() {
    // 表达式 disabled 不得被解析成布尔
    let sections = dump::parse_dump(DUMP).unwrap();
    let index = dump::index_by_id(&sections);
    let row = &index.get("bash-sandbox").unwrap().1;
    match &row.disabled {
        Some(DisabledValue::Expression(expr)) => assert!(expr.contains("process.platform")),
        other => panic!("期望表达式，实际 {other:?}"),
    }
}
