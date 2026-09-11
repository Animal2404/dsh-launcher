//! MCP 条目模型 —— 受管 MCP 区块的**文本载体**
//!
//! 官方事实（`$DSH_SRC`）：
//! - 一个 MCP server = cordis 配置树中的一行，`name: '@deepseek-ai/dsh-mcp-client'`
//!   （`packages/mcp/mcp-client/src/index.ts:98-135`）；
//! - `config` 为 `StdioConfig | StreamableHttpConfig` 联合，字段权威见
//!   `docs/config-catalog.md:1510-1577`；
//! - patch 层的声明**必须**包在 `- insert:` 里（否则打印 `patch: entry "X" not found`
//!   且不产生任何行）；启停是行级 `disabled` 覆盖；
//! - 插入行 id 沿用官方 README 示例形态 `mcp-<serverName>`。
//!
//! 设计要点（ADR-0006 D4「不透明保真」）：
//! `config` 子树以**逐行原始文本**（`config_raw`）存储与回写，只解析出只读视图
//! 用于校验与展示。`enable` / `disable` / `remove` **绝不重渲染** `config`，
//! 因此 `!!js` 表达式、内嵌注释、引号风格零损失。

use crate::core::plugin::managed::validate_row_id;

/// 受管区块声明段中的一条：`- insert:` 列表内的一行
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDeclare {
    /// 行 id（启动器声明的行固定 `mcp-<serverName>`）
    pub row_id: String,
    /// `config.serverName`（解析视图；缺失/非法形态时无法判定）
    pub server_name: Option<String>,
    /// `config.transport`（解析视图）
    pub transport: Option<McpTransport>,
    /// `config` 子树的**原始文本**（含自身相对缩进；不含 `config:` 键所在行）
    pub config_raw: String,
}

/// transport 取值（官方 `transport` 字段的两个合法值）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

impl McpTransport {
    /// 官方字段字面量
    pub fn as_str(&self) -> &'static str {
        match self {
            McpTransport::Stdio => "stdio",
            McpTransport::StreamableHttp => "streamable-http",
        }
    }

    /// 由官方字段字面量解析
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "stdio" => Some(McpTransport::Stdio),
            "streamable-http" => Some(McpTransport::StreamableHttp),
            _ => None,
        }
    }
}

/// 受管区块定向段中的一条：`- id:` + `disabled:`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDirective {
    /// 目标行 id（必须与某条声明（同层或更早层）的 `id` 一致，见不变量 #2）
    pub row_id: String,
    /// 期望 disabled 值（定向段只写字面量 true/false）
    pub disabled: bool,
}

/// 受管 MCP 区块 = 声明段 + 定向段
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpBlock {
    pub declares: Vec<McpDeclare>,
    pub directives: Vec<McpDirective>,
}

impl McpBlock {
    /// 空区块（无任何段落内容）
    pub fn is_empty(&self) -> bool {
        self.declares.is_empty() && self.directives.is_empty()
    }

    /// 按 `serverName` 查找声明（无 `serverName` 的畸形行不参与匹配）
    pub fn declare_by_server_name(&self, server_name: &str) -> Option<&McpDeclare> {
        self.declares
            .iter()
            .find(|item| item.server_name.as_deref() == Some(server_name))
    }

    /// 按行 id 查找声明
    pub fn declare_by_row_id(&self, row_id: &str) -> Option<&McpDeclare> {
        self.declares.iter().find(|item| item.row_id == row_id)
    }

    /// 按行 id 查找定向条目
    pub fn directive_by_row_id(&self, row_id: &str) -> Option<&McpDirective> {
        self.directives.iter().find(|item| item.row_id == row_id)
    }
}

/// 启动器声明的行 id 一律 `mcp-<serverName>`（与官方 README 示例同形）。
///
/// `serverName` 已由 `validate` 限定为 `^[A-Za-z0-9_-]{1,32}$`，因此生成的
/// 行 id 必然通过 `validate_row_id` 的安全字符检查。
pub fn default_row_id(server_name: &str) -> String {
    format!("mcp-{server_name}")
}

/// 校验一条声明可安全落盘：行 id 合法 + `serverName` 存在。
///
/// 调用方（服务层）负责先跑官方两条校验（`validate.rs`）；本函数只做
/// **写入安全**检查，防止畸形行 id 破坏区块语法。
pub fn validate_declare(declare: &McpDeclare) -> Result<(), String> {
    validate_row_id(&declare.row_id)?;
    if declare.config_raw.trim().is_empty() {
        return Err(format!(
            "声明 {} 的 config 子树为空（官方要求 config 必填）",
            declare.row_id
        ));
    }
    Ok(())
}

/// 校验一条定向条目可安全落盘。
pub fn validate_directive(directive: &McpDirective) -> Result<(), String> {
    validate_row_id(&directive.row_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_row_id_matches_official_example_shape() {
        assert_eq!(default_row_id("github"), "mcp-github");
        assert_eq!(default_row_id("my_server-2"), "mcp-my_server-2");
        // 生成的 id 必须通过写入安全检查
        assert!(validate_row_id(&default_row_id("a")).is_ok());
    }

    #[test]
    fn test_transport_parse_roundtrip() {
        for transport in [McpTransport::Stdio, McpTransport::StreamableHttp] {
            assert_eq!(McpTransport::parse(transport.as_str()), Some(transport));
        }
        // 官方只有这两个取值
        assert_eq!(McpTransport::parse("sse"), None);
        assert_eq!(McpTransport::parse(""), None);
        assert_eq!(McpTransport::parse("Stdio"), None);
    }

    #[test]
    fn test_block_lookups() {
        let block = McpBlock {
            declares: vec![
                McpDeclare {
                    row_id: "mcp-github".into(),
                    server_name: Some("github".into()),
                    transport: Some(McpTransport::StreamableHttp),
                    config_raw: "serverName: github".into(),
                },
                McpDeclare {
                    row_id: "mcp-odd".into(),
                    server_name: None,
                    transport: None,
                    config_raw: "x: 1".into(),
                },
            ],
            directives: vec![McpDirective {
                row_id: "mcp-github".into(),
                disabled: true,
            }],
        };
        assert_eq!(
            block.declare_by_server_name("github").unwrap().row_id,
            "mcp-github"
        );
        assert!(block.declare_by_server_name("missing").is_none());
        // 无 serverName 的畸形行不可被 serverName 命中
        assert!(block.declare_by_server_name("").is_none());
        assert_eq!(block.declare_by_row_id("mcp-odd").unwrap().row_id, "mcp-odd");
        assert_eq!(
            block.directive_by_row_id("mcp-github").unwrap().disabled,
            true
        );
        assert!(block.directive_by_row_id("nope").is_none());
        assert!(!block.is_empty());
        assert!(McpBlock::default().is_empty());
    }

    #[test]
    fn test_validate_declare_requires_safe_row_id_and_config() {
        let good = McpDeclare {
            row_id: "mcp-a".into(),
            server_name: Some("a".into()),
            transport: Some(McpTransport::Stdio),
            config_raw: "serverName: a".into(),
        };
        assert!(validate_declare(&good).is_ok());

        let bad_id = McpDeclare {
            row_id: "bad id".into(),
            ..good.clone()
        };
        assert!(validate_declare(&bad_id).unwrap_err().contains("非法行 id"));

        let unsafe_id = McpDeclare {
            row_id: "a'b".into(),
            ..good.clone()
        };
        assert!(validate_declare(&unsafe_id).unwrap_err().contains("不安全字符"));

        let empty_config = McpDeclare {
            config_raw: "   ".into(),
            ..good
        };
        assert!(validate_declare(&empty_config)
            .unwrap_err()
            .contains("config 子树为空"));
    }

    #[test]
    fn test_validate_directive_rejects_unsafe_row_id() {
        assert!(validate_directive(&McpDirective {
            row_id: "mcp-a".into(),
            disabled: false
        })
        .is_ok());
        assert!(validate_directive(&McpDirective {
            row_id: "a\nb".into(),
            disabled: false
        })
        .is_err());
    }
}
