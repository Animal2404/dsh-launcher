//! MCP 状态机（纯函数，可单测）
//!
//! 三态与行级标记的定义、判定规则见 ADR-0006 §State Machine：
//! - **三态**：`missing` / `enabled` / `disabled`（不引入 `error` 态）；
//! - **origin**：`managed`（声明在受管区块内）/ `external`（bundle / profile patch /
//!   home 文件用户区 / `--patch` 给出）；
//! - **行级标记**（不进状态机，只读展示）：`expression`（`disabled` 为 `!!js`，拒绝
//!   覆盖）/ `conflict`（`serverName` 与合成树其它行重复）/ `dangerous`
//!   （`failOnStartupError: true`，可中止 harness 启动）。
//!
//! 本模块不含 IO，供服务层与 CLI/IPC 复用。

use crate::core::mcp::entry::{McpBlock, McpTransport};
use crate::core::plugin::dump::{DisabledValue, McpRow};
use crate::core::plugin::state::{PluginError, PluginErrorKind};
use serde::Serialize;

/// MCP server 三态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpState {
    /// 合成树中不存在该 `serverName` 的行
    Missing,
    /// 存在，且有效 `disabled != true`
    Enabled,
    /// 存在，且有效 `disabled == true`
    Disabled,
}

impl McpState {
    pub fn as_str(&self) -> &'static str {
        match self {
            McpState::Missing => "missing",
            McpState::Enabled => "enabled",
            McpState::Disabled => "disabled",
        }
    }
}

/// 声明来源
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpOrigin {
    /// 声明在受管 MCP 区块内（`remove` 可真删除）
    Managed,
    /// 由 bundle / profile patch / home 用户区 / `--patch` 给出（`remove` 仅撤定向）
    External,
}

/// 行级只读标记
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpMark {
    /// `disabled` 为 `!!js` 表达式 → 启动器拒绝覆盖
    Expression,
    /// `serverName` 与合成树其它行重复（官方在加载期判后者失败）
    Conflict,
    /// `failOnStartupError: true` → 初始连接失败会让整个 harness 启动中止
    Dangerous,
}

/// 单个 MCP server 的视图（CLI / IPC / UI 的唯一形状）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    /// 面向模型的稳定身份（管理标识键）
    pub server_name: String,
    /// 行 id（`origin = managed` 时为启动器声明的 `mcp-<serverName>`）
    pub row_id: String,
    /// 官方 `config.transport`
    pub transport: Option<McpTransportView>,
    pub state: McpState,
    pub origin: McpOrigin,
    /// 来源层：dump 段标签（绝对路径 / bundle 包名）
    pub layer: String,
    /// 只读摘要：stdio 取 `command + args`，streamable-http 取 `url`
    pub summary: String,
    /// 有效 disabled；`null` = `!!js` 表达式（只读）
    pub disabled: Option<bool>,
    pub marks: Vec<McpMark>,
}

/// transport 的序列化形态（cli / IPC 用官方字面量）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransportView {
    Stdio,
    StreamableHttp,
}

impl From<McpTransport> for McpTransportView {
    fn from(value: McpTransport) -> Self {
        match value {
            McpTransport::Stdio => McpTransportView::Stdio,
            McpTransport::StreamableHttp => McpTransportView::StreamableHttp,
        }
    }
}

/// 由合成树事实派生三态（D5）。
///
/// - 无该行 → `missing`；
/// - 有该行且有效 `disabled != true` → `enabled`（无 `disabled` 字段也算启用）；
/// - 有该行且有效 `disabled == true` → `disabled`；
/// - `disabled` 为 `!!js` 表达式 → 状态未知，按**启用**处理（官方语义：表达式求值
///   后才决定是否激活；启动器不猜测结果），并打 `expression` 标记由上层拒绝覆盖。
pub fn derive_state(row: Option<&McpRow>) -> McpState {
    let Some(row) = row else {
        return McpState::Missing;
    };
    match &row.disabled {
        Some(DisabledValue::Bool(true)) => McpState::Disabled,
        Some(DisabledValue::Bool(false)) | Some(DisabledValue::Expression(_)) | None => {
            McpState::Enabled
        }
    }
}

/// 有效 disabled 的只读视图（`!!js` 表达式 → None）
pub fn effective_disabled(row: Option<&McpRow>) -> Option<bool> {
    match row.and_then(|row| row.disabled.as_ref()) {
        Some(DisabledValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

/// `origin` 判定：行 id 出现在受管区块的**声明段**内 → `managed`，否则 `external`（D6）。
pub fn derive_origin(block: &McpBlock, row_id: &str) -> McpOrigin {
    if block.declare_by_row_id(row_id).is_some() {
        McpOrigin::Managed
    } else {
        McpOrigin::External
    }
}

/// 行级标记（只读展示，不进状态机）
pub fn derive_marks(row: &McpRow, duplicate_of_server_name: bool) -> Vec<McpMark> {
    let mut marks = Vec::new();
    if row.is_expression_controlled() {
        marks.push(McpMark::Expression);
    }
    if duplicate_of_server_name {
        marks.push(McpMark::Conflict);
    }
    if row.fail_on_startup_error_is_literal_true() {
        marks.push(McpMark::Dangerous);
    }
    marks
}

// ============================ 非法转换 ============================

/// 用户请求的启停动作
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAction {
    Enable,
    Disable,
}

impl McpAction {
    pub fn desired_disabled(&self) -> bool {
        matches!(self, McpAction::Disable)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            McpAction::Enable => "enable",
            McpAction::Disable => "disable",
        }
    }
}

/// 校验 `enable` / `disable` 是否合法（D5 / §State Machine 3）。
///
/// - 目标不存在 → `NotFound`(3)（无对象，**不是**幂等空操作）；
/// - 目标行 `disabled` 为 `!!js` 表达式 → `IllegalTransition`(2)（拒绝覆盖）。
///
/// 目标状态与期望一致**不**算非法：由服务层返回 `unchanged`（幂等）。
pub fn validate_transition(
    server_name: &str,
    row: Option<&McpRow>,
    action: McpAction,
) -> Result<(), PluginError> {
    let Some(row) = row else {
        return Err(PluginError::not_found(format!(
            "MCP server {server_name} 不存在（合成树中没有该 serverName 的行），无法 {}",
            action.as_str()
        )));
    };
    if row.is_expression_controlled() {
        return Err(PluginError::illegal(format!(
            "MCP server {server_name}（行 {}）的 disabled 由 !!js 表达式控制，启动器拒绝覆盖",
            row.row_id
        )));
    }
    Ok(())
}

/// 校验新增声明的目标状态（`add` 的前置）。
///
/// `existing` 为合成树中同 `serverName` 的行：存在即非法（D9 唯一性），
/// 错误信息**必须**含冲突行的来源层（取自 dump 段标签）。
pub fn validate_add_target(server_name: &str, existing: Option<&McpRow>) -> Result<(), PluginError> {
    if let Some(row) = existing {
        return Err(PluginError::illegal(format!(
            "MCP server {server_name} 已存在（行 {}，来源层 {}），不能重复添加",
            row.row_id, row.section_owner
        )));
    }
    Ok(())
}

/// 期望态与现状一致的判据（幂等：`unchanged`，零落盘）。
pub fn same_directive(block: &McpBlock, row_id: &str, desired_disabled: bool) -> bool {
    matches!(
        block.directive_by_row_id(row_id),
        Some(directive) if directive.disabled == desired_disabled
    )
}

/// 未登记的 `PluginErrorKind` 兜底（本模块不新增退出码，只引用既有分级）。
pub fn conflict(message: impl Into<String>) -> PluginError {
    PluginError::new(PluginErrorKind::ManagedBlockConflict, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::entry::{McpDeclare, McpDirective};

    fn row_with(disabled: Option<DisabledValue>) -> McpRow {
        McpRow {
            row_id: "mcp-a".into(),
            section_owner: "C:\\Users\\u\\.dsh\\cordis.patch.yml".into(),
            disabled,
            config_raw: "  serverName: a\n  transport: stdio\n  command: a".into(),
        }
    }

    #[test]
    fn test_derive_state_three_states() {
        // missing：合成树无该行
        assert_eq!(derive_state(None), McpState::Missing);
        // enabled：无 disabled 字段 / 明确 false / 表达式（结果未知，按启用）
        assert_eq!(derive_state(Some(&row_with(None))), McpState::Enabled);
        assert_eq!(
            derive_state(Some(&row_with(Some(DisabledValue::Bool(false))))),
            McpState::Enabled
        );
        assert_eq!(
            derive_state(Some(&row_with(Some(DisabledValue::Expression(
                "!!js process.platform === 'win32'".into()
            ))))),
            McpState::Enabled
        );
        // disabled：明确 true
        assert_eq!(
            derive_state(Some(&row_with(Some(DisabledValue::Bool(true))))),
            McpState::Disabled
        );
        // 不引入 error 态
        assert_eq!(McpState::Missing.as_str(), "missing");
    }

    #[test]
    fn test_effective_disabled_view() {
        assert_eq!(effective_disabled(None), None);
        assert_eq!(effective_disabled(Some(&row_with(None))), None);
        assert_eq!(
            effective_disabled(Some(&row_with(Some(DisabledValue::Bool(true))))),
            Some(true)
        );
        // 表达式 → null（前端只读）
        assert_eq!(
            effective_disabled(Some(&row_with(Some(DisabledValue::Expression(
                "!!js x".into()
            ))))),
            None
        );
    }

    #[test]
    fn test_derive_origin() {
        let block = McpBlock {
            declares: vec![McpDeclare {
                row_id: "mcp-a".into(),
                server_name: Some("a".into()),
                transport: Some(McpTransport::Stdio),
                config_raw: "  serverName: a".into(),
            }],
            directives: vec![McpDirective {
                row_id: "mcp-a".into(),
                disabled: true,
            }],
        };
        assert_eq!(derive_origin(&block, "mcp-a"), McpOrigin::Managed);
        // 未在声明段内（可能是用户手写行）→ external
        assert_eq!(derive_origin(&block, "user-row"), McpOrigin::External);
        // 只有定向覆写、无声明 → 仍是 external（声明在别处）
        let only_directive = McpBlock {
            declares: vec![],
            directives: vec![McpDirective {
                row_id: "mcp-a".into(),
                disabled: true,
            }],
        };
        assert_eq!(derive_origin(&only_directive, "mcp-a"), McpOrigin::External);
    }

    #[test]
    fn test_derive_marks() {
        let mut row = row_with(Some(DisabledValue::Expression("!!js x".into())));
        row.config_raw = "  serverName: a\n  transport: stdio\n  command: a\n  failOnStartupError: true".into();
        let marks = derive_marks(&row, true);
        assert!(marks.contains(&McpMark::Expression));
        assert!(marks.contains(&McpMark::Conflict));
        assert!(marks.contains(&McpMark::Dangerous));
        // 无标记
        assert!(derive_marks(&row_with(None), false).is_empty());
    }

    #[test]
    fn test_validate_transition_rules() {
        // 目标不存在 → NotFound(3)，不是幂等空操作
        let err = validate_transition("a", None, McpAction::Disable).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::NotFound);
        assert_eq!(err.kind.exit_code(), 3);
        // 表达式行 → IllegalTransition(2)
        let expr = row_with(Some(DisabledValue::Expression("!!js x".into())));
        let err = validate_transition("a", Some(&expr), McpAction::Enable).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
        assert_eq!(err.kind.exit_code(), 2);
        // 正常行 → 合法（同状态由服务层判 unchanged）
        assert!(validate_transition("a", Some(&row_with(None)), McpAction::Enable).is_ok());
        assert!(validate_transition(
            "a",
            Some(&row_with(Some(DisabledValue::Bool(true)))),
            McpAction::Enable
        )
        .is_ok());
    }

    #[test]
    fn test_validate_add_target_reports_conflict_layer() {
        assert!(validate_add_target("a", None).is_ok());
        let existing = row_with(None);
        let err = validate_add_target("a", Some(&existing)).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
        // 错误信息必须含冲突行的来源层
        assert!(err.message.contains("mcp-a"), "{}", err.message);
        assert!(
            err.message.contains("C:\\Users\\u\\.dsh\\cordis.patch.yml"),
            "{}",
            err.message
        );
    }

    #[test]
    fn test_same_directive_idempotency_judgement() {
        let block = McpBlock {
            declares: vec![],
            directives: vec![McpDirective {
                row_id: "mcp-a".into(),
                disabled: true,
            }],
        };
        assert!(same_directive(&block, "mcp-a", true));
        assert!(!same_directive(&block, "mcp-a", false));
        assert!(!same_directive(&block, "mcp-b", true));
    }
}
