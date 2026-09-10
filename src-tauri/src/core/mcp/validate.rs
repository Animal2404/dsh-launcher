//! MCP 校验 —— **只实现官方强制的两条**（ADR-0006 D9）
//!
//! 官方事实（`$DSH_SRC`）：
//! - `serverName` 必须匹配 `^[A-Za-z0-9_-]{1,32}$`
//!   （`packages/mcp/mcp-client/src/index.ts:37-38,116,127`）；
//! - `serverName` 必须唯一，运行时按**注册作用域**强制
//!   （`index.ts:45,152-168`：`activeServerNames: WeakMap<scope, Set<string>>`，
//!   重复时**后加载的实例在加载期抛错，先前实例不受影响**）。
//!
//! **被否**（ADR D9 明确不实现）：`stdio.command` 可解析性探测 —— 官方无此校验，
//! 结果随 PATH 与工具链漂移，归入"用户自查项"。
//!
//! 启动器只能看到 root 作用域的合成树（S1 不确定项已确认），故唯一性校验的范围
//! 是「`--dump-config` 的合成树全量」，并排除自身。

use crate::core::mcp::entry::McpTransport;
use crate::core::plugin::dump::McpRow;
use crate::core::plugin::state::{PluginError, PluginErrorKind};

/// `serverName` 最大长度（官方正则的 `{1,32}`）
pub const SERVER_NAME_MAX_LEN: usize = 32;

/// 校验 `serverName` 是否满足官方正则 `^[A-Za-z0-9_-]{1,32}$`。
///
/// 手写等价实现（不引入 regex 依赖）：长度 1..=32 且字符集为
/// ASCII 字母数字 / `_` / `-`。
pub fn validate_server_name(server_name: &str) -> Result<(), PluginError> {
    if server_name.is_empty() {
        return Err(PluginError::illegal("serverName 不能为空"));
    }
    let length = server_name.chars().count();
    if length > SERVER_NAME_MAX_LEN {
        return Err(PluginError::illegal(format!(
            "serverName {server_name:?} 超长（{length} 字符 > 官方上限 {SERVER_NAME_MAX_LEN}）"
        )));
    }
    if !server_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(PluginError::illegal(format!(
            "serverName {server_name:?} 含非法字符（官方要求 ^[A-Za-z0-9_-]{{1,32}}$）"
        )));
    }
    Ok(())
}

/// 合成树范围内的 `serverName` 唯一性（**排除自身**）。
///
/// `self_row_id` 为本次操作的目标行（`add` 时为将要新增的行 id，`None` 表示不排除
/// 任何行）。冲突时报 `IllegalTransition`(2)，错误信息**必须**含冲突行的来源层
/// （取自 dump 段标签），便于用户定位到底哪一层重复声明。
pub fn validate_unique_server_name(
    server_name: &str,
    rows: &[McpRow],
    self_row_id: Option<&str>,
) -> Result<(), PluginError> {
    let conflicts: Vec<&McpRow> = rows
        .iter()
        .filter(|row| Some(row.row_id.as_str()) != self_row_id)
        .filter(|row| row.server_name().as_deref() == Some(server_name))
        .collect();
    if conflicts.is_empty() {
        return Ok(());
    }
    let detail = conflicts
        .iter()
        .map(|row| format!("行 {}（来源层 {}）", row.row_id, row.section_owner))
        .collect::<Vec<_>>()
        .join("、");
    Err(PluginError::illegal(format!(
        "serverName {server_name} 在合成树中重复：{detail}。官方按注册作用域强制唯一，\
         加载期后出现的实例会失败；请先处理重复声明"
    )))
}

/// transport 的必填字段校验（官方 `config` 的 required 项）。
///
/// - `stdio` 缺 `command` → `IllegalTransition`(2)；
/// - `streamable-http` 缺 `url` → `IllegalTransition`(2)。
pub fn validate_transport_required(
    transport: McpTransport,
    command: Option<&str>,
    url: Option<&str>,
) -> Result<(), PluginError> {
    match transport {
        McpTransport::Stdio => {
            if command.map(|value| value.trim().is_empty()).unwrap_or(true) {
                return Err(PluginError::illegal(
                    "transport 为 stdio 时必须提供 command（官方必填字段）",
                ));
            }
        }
        McpTransport::StreamableHttp => {
            if url.map(|value| value.trim().is_empty()).unwrap_or(true) {
                return Err(PluginError::illegal(
                    "transport 为 streamable-http 时必须提供 url（官方必填字段）",
                ));
            }
        }
    }
    Ok(())
}

/// `add` 的前置校验合集：官方两条 + transport 必填。
pub fn validate_new_declaration(
    server_name: &str,
    transport: McpTransport,
    command: Option<&str>,
    url: Option<&str>,
    tree: &[McpRow],
) -> Result<(), PluginError> {
    validate_server_name(server_name)?;
    validate_transport_required(transport, command, url)?;
    validate_unique_server_name(server_name, tree, None)
}

/// 校验结果是否为"官方两条校验"之外的错误类型（供服务层归纳）。
pub fn is_validation_error(error: &PluginError) -> bool {
    error.kind == PluginErrorKind::IllegalTransition
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::plugin::dump::DisabledValue;

    fn mcp_row(row_id: &str, server_name: &str, owner: &str) -> McpRow {
        McpRow {
            row_id: row_id.into(),
            section_owner: owner.into(),
            disabled: None,
            config_raw: format!("  serverName: {server_name}\n  transport: stdio\n  command: x"),
        }
    }

    #[test]
    fn test_server_name_length_boundaries() {
        // 1 / 32 / 33 字符边界
        assert!(validate_server_name("a").is_ok());
        let len32 = "a".repeat(32);
        assert!(validate_server_name(&len32).is_ok());
        let len33 = "a".repeat(33);
        let err = validate_server_name(&len33).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
        assert!(err.message.contains("超长"));
        // 空
        assert!(validate_server_name("").unwrap_err().message.contains("不能为空"));
    }

    #[test]
    fn test_server_name_character_set() {
        for good in ["github", "my_server-2", "A1", "-", "_"] {
            assert!(validate_server_name(good).is_ok(), "{good} 应合法");
        }
        for bad in ["a b", "a.b", "a/b", "mcp:github", "a\tb", "服务", "a\nb", "github!"] {
            let err = validate_server_name(bad).unwrap_err();
            assert_eq!(err.kind, PluginErrorKind::IllegalTransition, "{bad}");
            assert!(err.message.contains("非法字符"), "{bad}: {}", err.message);
        }
    }

    #[test]
    fn test_unique_server_name_with_layer_in_message() {
        let tree = vec![
            mcp_row("mcp-github", "github", "C:\\Users\\u\\.dsh\\cordis.patch.yml"),
            mcp_row("user-row", "shadcn", "C:\\Users\\u\\.dsh\\profiles\\web\\cordis.patch.yml"),
        ];
        // 无冲突
        assert!(validate_unique_server_name("playwright", &tree, None).is_ok());
        // 冲突：错误信息含冲突行 id 与来源层
        let err = validate_unique_server_name("github", &tree, None).unwrap_err();
        assert_eq!(err.kind, PluginErrorKind::IllegalTransition);
        assert!(err.message.contains("mcp-github"));
        assert!(err.message.contains("C:\\Users\\u\\.dsh\\cordis.patch.yml"));
        // 排除自身后不冲突（启停/校验已有行时用）
        assert!(validate_unique_server_name("github", &tree, Some("mcp-github")).is_ok());
    }

    #[test]
    fn test_unique_detects_multiple_conflicts() {
        let tree = vec![
            mcp_row("mcp-a", "dup", "bundle-a"),
            mcp_row("mcp-b", "dup", "bundle-b"),
        ];
        let err = validate_unique_server_name("dup", &tree, None).unwrap_err();
        assert!(err.message.contains("bundle-a"));
        assert!(err.message.contains("bundle-b"));
    }

    #[test]
    fn test_transport_required_fields() {
        // stdio 必须 command
        let err = validate_transport_required(McpTransport::Stdio, None, Some("https://x")).unwrap_err();
        assert!(err.message.contains("command"));
        let err = validate_transport_required(McpTransport::Stdio, Some("   "), None).unwrap_err();
        assert!(err.message.contains("command"));
        assert!(validate_transport_required(McpTransport::Stdio, Some("shadcn"), None).is_ok());
        // streamable-http 必须 url
        let err =
            validate_transport_required(McpTransport::StreamableHttp, Some("shadcn"), None).unwrap_err();
        assert!(err.message.contains("url"));
        assert!(
            validate_transport_required(McpTransport::StreamableHttp, None, Some("https://x")).is_ok()
        );
    }

    #[test]
    fn test_validate_new_declaration_combines_official_rules() {
        let tree = vec![mcp_row("mcp-github", "github", "home")];
        // 合法
        assert!(validate_new_declaration(
            "playwright",
            McpTransport::Stdio,
            Some("playwright-mcp"),
            None,
            &tree
        )
        .is_ok());
        // 非法名字优先报错
        assert!(validate_new_declaration(
            "bad name",
            McpTransport::Stdio,
            Some("x"),
            None,
            &tree
        )
        .is_err());
        // 缺必填字段
        assert!(
            validate_new_declaration("playwright", McpTransport::Stdio, None, None, &tree).is_err()
        );
        // 重复
        let err = validate_new_declaration(
            "github",
            McpTransport::Stdio,
            Some("x"),
            None,
            &tree,
        )
        .unwrap_err();
        assert!(err.message.contains("重复"));
    }

    #[test]
    fn test_expression_row_does_not_break_uniqueness() {
        // 表达式 disabled 不影响 serverName 提取与唯一性判定
        let mut row = mcp_row("mcp-a", "a", "home");
        row.disabled = Some(DisabledValue::Expression("!!js x".into()));
        let tree = vec![row];
        assert!(validate_unique_server_name("a", &tree, None).is_err());
        assert!(is_validation_error(&validate_unique_server_name("a", &tree, None).unwrap_err()));
    }
}
