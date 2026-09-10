//! `dsh --profile <name> --dump-config` 输出的行级解析
//!
//! dump 由 `renderConfigDump` 生成（deepseek-harness/packages/boot/app-boot/src/index.ts），
//! 形态固定：每个来源段以 `# == <bundle>`（或 `# == <bundle>, patched by <overlay>`）
//! 注释开头，随后是该层的顶层行 `- id: ...` 与其 2 空格缩进行属性。
//!
//! 本模块**只**提取插件启停需要的三件事：段来源、行 id、行 `disabled`。
//! config 块（可能含 `!!js` 标签、嵌套列表）一律忽略，因此不引入 YAML 依赖。
//! 出现列 0 的未知行即判定 dump 格式变化并 fail loud，绝不返回"部分解析结果"。

use std::collections::BTreeMap;

/// 行级 `disabled` 取值
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisabledValue {
    /// 字面量 true/false
    Bool(bool),
    /// `!!js` 表达式（平台相关，禁止启动器覆盖）
    Expression(String),
}

/// 一行配置（顶层条目）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpRow {
    pub id: String,
    pub name: Option<String>,
    pub disabled: Option<DisabledValue>,
}

impl DumpRow {
    /// 是否由表达式控制 disabled（启动器拒绝覆盖）
    pub fn is_expression_controlled(&self) -> bool {
        matches!(self.disabled, Some(DisabledValue::Expression(_)))
    }

    /// 有效启用状态：无 disabled 字段视为启用；表达式视为未知（返回 None）。
    pub fn effective_enabled(&self) -> Option<bool> {
        match &self.disabled {
            None => Some(true),
            Some(DisabledValue::Bool(v)) => Some(!*v),
            Some(DisabledValue::Expression(_)) => None,
        }
    }
}

/// 一个来源段（一个 bundle 层，或用户 patch 层）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpSection {
    /// 段标签第一段（bundle 包名；用户 patch 层为文件路径）
    pub owner: String,
    /// `patched by` 列表（该段的行被哪些层覆盖过）
    pub patched_by: Vec<String>,
    pub rows: Vec<DumpRow>,
}

/// 解析 dump 文本。错误即格式契约破裂，调用方必须降级为只读展示。
pub fn parse_dump(text: &str) -> Result<Vec<DumpSection>, String> {
    let mut sections: Vec<DumpSection> = Vec::new();
    let mut current_section: Option<usize> = None;
    let mut current_row: Option<(usize, usize)> = None;

    for (index, raw) in text.lines().enumerate() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let line_no = index + 1;

        if line.trim().is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix("# == ") {
            let (owner, patched_by) = split_header(header);
            sections.push(DumpSection {
                owner,
                patched_by,
                rows: Vec::new(),
            });
            current_section = Some(sections.len() - 1);
            current_row = None;
            continue;
        }
        if line.starts_with('#') {
            // 普通注释（含启动器自己的 managed 标记）
            continue;
        }
        if !line.starts_with(' ') {
            // 列 0：只允许新的顶层行 `- id: <value>`
            let Some(value) = line.strip_prefix("- id: ") else {
                return Err(format!(
                    "dump 第 {line_no} 行为未知的顶层语法（期望 `- id: ...`）：{}",
                    truncate(line)
                ));
            };
            let Some(section) = current_section else {
                return Err(format!(
                    "dump 第 {line_no} 行出现顶层行但此前没有任何 `# == ` 来源段"
                ));
            };
            sections[section].rows.push(DumpRow {
                id: unquote(value).to_string(),
                name: None,
                disabled: None,
            });
            current_row = Some((section, sections[section].rows.len() - 1));
            continue;
        }
        // 缩进行：只有恰好 2 空格缩进的键值属于"行属性"；更深缩进属于 config。
        if line.starts_with("  ") && !line.starts_with("   ") {
            let Some((section, row)) = current_row else {
                return Err(format!(
                    "dump 第 {line_no} 行出现行属性但没有对应的顶层行：{}",
                    truncate(line)
                ));
            };
            let Some((key, value)) = line.trim_start().split_once(':') else {
                return Err(format!(
                    "dump 第 {line_no} 行不是 `key: value` 形态：{}",
                    truncate(line)
                ));
            };
            match key {
                "name" => {
                    sections[section].rows[row].name = Some(unquote(value.trim()).to_string())
                }
                "disabled" => {
                    sections[section].rows[row].disabled = Some(parse_disabled(value.trim()))
                }
                // config / group / inject / ... 与启停无关，忽略（其嵌套内容缩进更深）
                _ => {}
            }
            continue;
        }
        // 3 空格及以上：config 块内容，忽略。
    }

    Ok(sections)
}

/// 把段标签拆成 owner 与 patched_by。
fn split_header(header: &str) -> (String, Vec<String>) {
    match header.split_once(", patched by ") {
        None => (header.trim().to_string(), Vec::new()),
        Some((owner, rest)) => (
            owner.trim().to_string(),
            rest.split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect(),
        ),
    }
}

/// 去掉 YAML 单引号包裹（js-yaml 会对含 `@`/`/` 的名字加引号）。
fn unquote(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return &trimmed[1..trimmed.len() - 1];
    }
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        return &trimmed[1..trimmed.len() - 1];
    }
    trimmed
}

/// 解析 `disabled` 值：`true`/`false` 为字面量，其余（`!!js ...`）为表达式。
fn parse_disabled(value: &str) -> DisabledValue {
    match value {
        "true" => DisabledValue::Bool(true),
        "false" => DisabledValue::Bool(false),
        other => DisabledValue::Expression(other.to_string()),
    }
}

fn truncate(line: &str) -> String {
    const MAX: usize = 120;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    let mut out: String = line.chars().take(MAX).collect();
    out.push('…');
    out
}

/// 行 id → 所在段 owner 的索引（同一 id 在多段出现时取最后一段，与 Loader 的 id 覆盖一致）。
pub fn index_by_id(sections: &[DumpSection]) -> BTreeMap<String, (String, DumpRow)> {
    let mut map = BTreeMap::new();
    for section in sections {
        for row in &section.rows {
            map.insert(row.id.clone(), (section.owner.clone(), row.clone()));
        }
    }
    map
}

/// 找出某来源段贡献的全部行 id。
pub fn rows_of_owner(sections: &[DumpSection], owner: &str) -> Vec<String> {
    sections
        .iter()
        .filter(|section| section.owner == owner)
        .flat_map(|section| section.rows.iter().map(|row| row.id.clone()))
        .collect()
}

// ==================== MCP 行提取（ADR-0006） ====================
//
// `parse_dump` 只提取启停需要的三件事（段来源 / 行 id / disabled），config 子树
// 一律忽略。MCP 管理额外需要 config 子树的**原始文本**（D4 不透明保真）与其中的
// 只读字段，故这里对同一份 dump 再走一遍独立扫描：
//
// - 段标签 `# == <label>` 给出「声明来自哪一层」（绝对路径 / bundle 包名）；
// - 顶层 `- id:` 开启新行；`name:` 判定是否官方 MCP 插件行；
// - 恰好 2 空格缩进的 `config:` 之后、所有更深缩进的行 = config 子树原文。
//
// `!!js` 表达式由 dump 原样打印（`renderConfigDump` 不求值），因此 `!!js` 形态的
// `failOnStartupError` 无法判定真假 —— 只对**字面量 `true`** 打危险标记（D10）。

/// 官方 MCP 客户端插件包名（`config-catalog.md:1510`）
pub const MCP_CLIENT_PACKAGE: &str = "@deepseek-ai/dsh-mcp-client";

/// 合成树中的一行 MCP server
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRow {
    /// 行 id（cordis 标识）
    pub row_id: String,
    /// dump 段来源标签（绝对路径 / bundle 包名）—— **声明来自哪一层**
    pub section_owner: String,
    /// 有效 `disabled`（None = 行上无该字段 = 默认启用）
    pub disabled: Option<DisabledValue>,
    /// `config` 子树原始文本（已归一化到首层 2 空格缩进；不含 `config:` 键行）
    pub config_raw: String,
}

impl McpRow {
    /// `config.serverName`（只读视图）
    pub fn server_name(&self) -> Option<String> {
        crate::core::mcp::block::config_scalar(&self.config_raw, "serverName")
    }

    /// `config.transport`（只读视图）
    pub fn transport(&self) -> Option<crate::core::mcp::entry::McpTransport> {
        crate::core::mcp::block::config_scalar(&self.config_raw, "transport")
            .and_then(|value| crate::core::mcp::entry::McpTransport::parse(&value))
    }

    /// `config.failOnStartupError` 是否为**字面量** `true`（危险：可中止 harness 启动）
    pub fn fail_on_startup_error_is_literal_true(&self) -> bool {
        matches!(
            crate::core::mcp::block::config_scalar(&self.config_raw, "failOnStartupError").as_deref(),
            Some("true")
        )
    }

    /// 只读摘要：stdio 取 `command + args`，streamable-http 取 `url`
    pub fn summary(&self) -> String {
        let scalar = |key: &str| crate::core::mcp::block::config_scalar(&self.config_raw, key);
        match self.transport() {
            Some(crate::core::mcp::entry::McpTransport::StreamableHttp) => {
                scalar("url").unwrap_or_else(|| "-".to_string())
            }
            Some(crate::core::mcp::entry::McpTransport::Stdio) => {
                let command = scalar("command").unwrap_or_else(|| "-".to_string());
                // `args` 的官方形态是 YAML 流式序列（`[mcp]`）或块序列，只做只读展示
                match args_render(&self.config_raw) {
                    Some(args) if !args.is_empty() => format!("{command} {}", args.join(" ")),
                    _ => command,
                }
            }
            None => "-".to_string(),
        }
    }

    /// `disabled` 是否为 `!!js` 表达式（启动器拒绝覆盖）
    pub fn is_expression_controlled(&self) -> bool {
        matches!(self.disabled, Some(DisabledValue::Expression(_)))
    }
}

/// 解析 `args` 的只读视图（支持官方两种 YAML 序列形态，不做完整 YAML 反序列化）。
///
/// - 流式：`args: [mcp]` / `args: []`
/// - 块序列：
///   ```yaml
///   args:
///     - Y:/
///   ```
fn args_render(config_raw: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = config_raw
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let base = lines
        .iter()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.is_empty() && !trimmed.starts_with('#')
        })
        .map(|line| line.len() - line.trim_start_matches(' ').len())
        .min()
        .unwrap_or(0);
    for (index, line) in lines.iter().enumerate() {
        if line.len() - line.trim_start_matches(' ').len() != base {
            continue;
        }
        let Some(rest) = line.trim_start().strip_prefix("args:") else {
            continue;
        };
        let inline = rest.trim();
        if !inline.is_empty() {
            // 流式序列：去掉方括号后按逗号切
            let inner = inline
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .unwrap_or(inline);
            return Some(
                inner
                    .split(',')
                    .map(|item| item.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
                    .filter(|item| !item.is_empty())
                    .collect(),
            );
        }
        // 块序列：紧随其后、更深的 `- ` 行
        let mut args = Vec::new();
        for follow in lines.iter().skip(index + 1) {
            let follow_indent = follow.len() - follow.trim_start_matches(' ').len();
            if follow_indent <= base {
                break;
            }
            if let Some(item) = follow.trim_start().strip_prefix("- ") {
                args.push(
                    item.trim()
                        .trim_matches(|c| c == '\'' || c == '"')
                        .to_string(),
                );
            }
        }
        return Some(args);
    }
    None
}

/// 提取合成树中**全部**官方 MCP 行（跨所有来源层，段顺序即合成顺序）。
///
/// 段标签相同、但 `- insert:` 内声明与顶层裸 `- id:` 都可能出现 —— 后者在 patch
/// 层不产生行，但 bundle 层是合法的行清单，故一视同仁地提取。
pub fn mcp_rows(text: &str) -> Vec<McpRow> {
    let mut rows: Vec<McpRow> = Vec::new();
    // 与 rows 索引对齐：该行是否已确认为官方 MCP 插件行
    let mut is_mcp: Vec<bool> = Vec::new();
    let mut section_owner = String::new();
    let mut current: Option<usize> = None;

    let mut index = 0usize;
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    while index < lines.len() {
        let line = lines[index];
        if line.trim().is_empty() {
            index += 1;
            continue;
        }
        if let Some(header) = line.strip_prefix("# == ") {
            section_owner = split_header(header).0;
            current = None;
            index += 1;
            continue;
        }
        if line.starts_with('#') {
            index += 1;
            continue;
        }
        // 顶层行（列 0）：`- id: <rowId>`
        if !line.starts_with(' ') {
            if let Some(rest) = line.strip_prefix("- id: ") {
                rows.push(McpRow {
                    row_id: unquote(rest).to_string(),
                    section_owner: section_owner.clone(),
                    disabled: None,
                    config_raw: String::new(),
                });
                is_mcp.push(false);
                current = Some(rows.len() - 1);
            }
            index += 1;
            continue;
        }
        // 恰好 2 空格缩进 = 行属性
        if line.starts_with("  ") && !line.starts_with("   ") {
            let trimmed = line.trim_start();
            if let Some((key, value)) = trimmed.split_once(':') {
                match key.trim() {
                    "name" if current.is_some() => {
                        let row = current.unwrap();
                        if unquote(value.trim()) == MCP_CLIENT_PACKAGE {
                            is_mcp[row] = true;
                        }
                    }
                    "disabled" if current.is_some() => {
                        let row = current.unwrap();
                        rows[row].disabled = Some(parse_disabled(value.trim()));
                    }
                    "config" if current.is_some() => {
                        let row = current.unwrap();
                        let (raw, next) = take_config_subtree(&lines, index);
                        rows[row].config_raw =
                            crate::core::mcp::block::normalize_config_raw(&raw);
                        index = next;
                        continue;
                    }
                    _ => {}
                }
            }
            index += 1;
            continue;
        }
        index += 1;
    }

    // 只保留「已确认是官方 MCP 插件」的行，并同步过滤标记
    let mut keep = is_mcp.iter().copied();
    rows.retain(|_| keep.next().unwrap_or(false));
    rows
}

/// 取 `config:` 键行之后、所有更深缩进的行（保留原始缩进）。
///
/// **段标签 `# == ` 是硬边界**：dump 的 config 体不可能包含它，而段标签在原文里
/// 没有缩进；若只按"缩进更深"判断，把 config 子树与后续段之间的空行算进来后，
/// 下一个 `# == ` 段头就会被当成 config 的续行吞掉 —— 之后所有行的 `section_owner`
/// 都会停留在**上一个**段，来源层归属整体错位。
fn take_config_subtree(lines: &[&str], key_index: usize) -> (String, usize) {
    let key_indent = lines[key_index].len() - lines[key_index].trim_start_matches(' ').len();
    let mut end = key_index + 1;
    while end < lines.len() {
        let trimmed = lines[end].trim_start();
        if trimmed.starts_with("# == ") {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            end += 1;
            continue;
        }
        let indent = lines[end].len() - lines[end].trim_start_matches(' ').len();
        if indent > key_indent {
            end += 1;
            continue;
        }
        break;
    }
    let raw = lines[key_index + 1..end]
        .iter()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    (raw.trim_end().to_string(), end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实 dump 片段（取自本机 `--profile web` 实测输出，含多行 bundle 与表达式 disabled）
    const FIXTURE: &str = r#"# == @deepseek-ai/dsh-base
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
  config:
    timeoutMs: 60000
# == @deepseek-ai/dsh-base, patched by @michengai/dsh-archive-manager
- id: session-projection-cache
  name: '@deepseek-ai/dsh-session-projection-cache'
  config:
    writeEveryEvents: 200
  disabled: true
# == dsh-find-plugin
- id: find-dsh-plugin
  name: dsh-find-plugin
# == @linxin666/dsh-client-ui-task-board
- id: ui-task-board
  name: '@linxin666/dsh-client-ui-task-board'
"#;

    #[test]
    fn test_parse_real_fixture() {
        let sections = parse_dump(FIXTURE).expect("fixture 必须可解析");
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].owner, "@deepseek-ai/dsh-base");
        assert!(sections[0].patched_by.is_empty());
        assert_eq!(sections[1].patched_by, vec!["@michengai/dsh-archive-manager"]);
        assert_eq!(sections[3].owner, "@linxin666/dsh-client-ui-task-board");

        let base = &sections[0].rows;
        assert_eq!(base.len(), 3);
        assert_eq!(base[0].id, "timer");
        assert_eq!(
            base[0].name.as_deref(),
            Some("@deepseek-ai/cordis-plugin-timer")
        );
        assert_eq!(base[0].disabled, None);
        assert_eq!(base[0].effective_enabled(), Some(true));
        assert_eq!(base[1].disabled, Some(DisabledValue::Bool(true)));
        assert_eq!(base[1].effective_enabled(), Some(false));
        assert!(base[2].is_expression_controlled());
        assert_eq!(base[2].effective_enabled(), None);
    }

    #[test]
    fn test_index_and_owner_rows() {
        let sections = parse_dump(FIXTURE).unwrap();
        let index = index_by_id(&sections);
        assert_eq!(index.get("timer").unwrap().0, "@deepseek-ai/dsh-base");
        // 同一 id 出现在后面的段时以后者为准（Loader 的 id 覆盖语义）
        assert_eq!(
            index.get("session-projection-cache").unwrap().0,
            "@deepseek-ai/dsh-base"
        );
        assert_eq!(
            rows_of_owner(&sections, "@linxin666/dsh-client-ui-task-board"),
            vec!["ui-task-board"]
        );
    }

    #[test]
    fn test_unknown_top_level_line_fails_loud() {
        let text = "# == x\n- id: a\n  name: b\nsurprise: 1\n";
        let err = parse_dump(text).unwrap_err();
        assert!(err.contains("未知的顶层语法"), "{err}");
    }

    #[test]
    fn test_row_without_section_fails_loud() {
        let err = parse_dump("- id: a\n").unwrap_err();
        assert!(err.contains("没有任何"), "{err}");
    }

    #[test]
    fn test_empty_dump_is_ok() {
        assert!(parse_dump("").unwrap().is_empty());
        assert!(parse_dump("\n\n").unwrap().is_empty());
    }

    // ==================== MCP 行提取（ADR-0006） ====================

    /// 本机实测的 MCP 段（`dsh --profile web --dump-config` 输出片段，含 `!!js` 与内嵌注释）
    const MCP_SECTION: &str = concat!(
        "# == @deepseek-ai/dsh-base\n",
        "- id: timer\n",
        "  name: '@deepseek-ai/cordis-plugin-timer'\n",
        "# == C:\\Users\\Administrator\\.dsh\\cordis.patch.yml\n",
        "- id: mcp-github\n",
        "  name: '@deepseek-ai/dsh-mcp-client'\n",
        "  config:\n",
        "    serverName: github\n",
        "    transport: streamable-http\n",
        "    url: https://api.githubcopilot.com/mcp/\n",
        "    headers:\n",
        "      Authorization: !!js >-\n",
        "        (() => { try { const t = gh(); if (t) return `Bearer ${t}` } catch {} return '' })()\n",
        "    toolCallTimeoutMs: 60000\n",
        "    failOnStartupError: false\n",
        "- id: mcp-shadcn\n",
        "  name: '@deepseek-ai/dsh-mcp-client'\n",
        "  config:\n",
        "    serverName: shadcn\n",
        "    transport: stdio\n",
        "    command: shadcn\n",
        "    args:\n",
        "      - mcp\n",
        "    cwd: !!js process.cwd()\n",
        "- id: mcp-playwright\n",
        "  name: '@deepseek-ai/dsh-mcp-client'\n",
        "  disabled: !!js process.platform === 'win32'\n",
        "  config:\n",
        "    serverName: playwright\n",
        "    transport: stdio\n",
        "    command: playwright-mcp\n",
        "    args: []\n",
        "    failOnStartupError: true\n",
    );

    #[test]
    fn test_mcp_rows_extracts_only_official_mcp_rows() {
        let rows = mcp_rows(MCP_SECTION);
        // `timer` 不是 MCP 行，被排除
        assert_eq!(
            rows.iter().map(|row| row.row_id.as_str()).collect::<Vec<_>>(),
            vec!["mcp-github", "mcp-shadcn", "mcp-playwright"]
        );
        // 段标签 = 来源层（绝对路径），直接给出"声明来自哪一层"
        for row in &rows {
            assert_eq!(
                row.section_owner,
                "C:\\Users\\Administrator\\.dsh\\cordis.patch.yml"
            );
        }
    }

    #[test]
    fn test_mcp_rows_config_raw_is_normalized_and_verbatim() {
        let rows = mcp_rows(MCP_SECTION);
        let github = &rows[0];
        // 首层字段归一到 2 空格；更深层级与 `!!js` 折叠标量逐字节保留（相对深度不变）
        assert_eq!(
            github.config_raw,
            concat!(
                "  serverName: github\n",
                "  transport: streamable-http\n",
                "  url: https://api.githubcopilot.com/mcp/\n",
                "  headers:\n",
                "    Authorization: !!js >-\n",
                "      (() => { try { const t = gh(); if (t) return `Bearer ${t}` } catch {} return '' })()\n",
                "  toolCallTimeoutMs: 60000\n",
                "  failOnStartupError: false",
            )
        );
        // 只读视图
        assert_eq!(github.server_name().as_deref(), Some("github"));
        assert_eq!(
            github.transport(),
            Some(crate::core::mcp::entry::McpTransport::StreamableHttp)
        );
        assert!(!github.fail_on_startup_error_is_literal_true());
        assert_eq!(github.summary(), "https://api.githubcopilot.com/mcp/");
    }

    #[test]
    fn test_mcp_rows_summary_for_stdio_and_block_args() {
        let rows = mcp_rows(MCP_SECTION);
        let shadcn = &rows[1];
        assert_eq!(shadcn.summary(), "shadcn mcp");
        let playwright = &rows[2];
        // 流式空数组 → 只有 command
        assert_eq!(playwright.summary(), "playwright-mcp");
    }

    #[test]
    fn test_mcp_rows_dangerous_and_expression_flags() {
        let rows = mcp_rows(MCP_SECTION);
        // 字面量 true → 危险徽章
        assert!(rows[2].fail_on_startup_error_is_literal_true());
        // `!!js` 形态的 disabled → expression 标记（拒绝覆盖）
        assert!(rows[2].is_expression_controlled());
        assert_eq!(rows[2].disabled, Some(DisabledValue::Expression(
            "!!js process.platform === 'win32'".to_string()
        )));
        // 无 disabled 字段 → 默认启用
        assert_eq!(rows[0].disabled, None);
        assert!(!rows[0].is_expression_controlled());
    }

    #[test]
    fn test_mcp_rows_dangerous_flag_not_set_for_expression_form() {
        // `failOnStartupError: !!js ...` 无法在 dump 层判真 → 不得打危险标记
        let text = concat!(
            "# == home\n",
            "- id: mcp-x\n",
            "  name: '@deepseek-ai/dsh-mcp-client'\n",
            "  config:\n",
            "    serverName: x\n",
            "    transport: stdio\n",
            "    command: x\n",
            "    failOnStartupError: !!js process.env.YES === '1'\n",
        );
        let rows = mcp_rows(text);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].fail_on_startup_error_is_literal_true());
    }

    #[test]
    fn test_mcp_rows_empty_when_no_mcp() {
        assert!(mcp_rows("").is_empty());
        assert!(mcp_rows(FIXTURE).is_empty());
    }

    /// 真实 `dsh --profile web --dump-config` 全量输出（隔离 DSH_HOME，576 行）
    const REAL_DUMP: &str = include_str!("../../../tests/fixtures-real-dump.txt");

    #[test]
    fn test_mcp_rows_real_dump_attributes_home_patch_layer() {
        // 先确认"最后一次段头"确实是 home patch 文件
        let last_header = REAL_DUMP
            .lines()
            .filter(|line| line.starts_with("# == "))
            .next_back()
            .unwrap()
            .to_string();
        assert!(
            last_header.contains("cordis.patch.yml"),
            "最后一次段头应为 home patch 文件：{last_header}"
        );
        let header_pos = REAL_DUMP.find(&last_header).unwrap();
        assert!(
            REAL_DUMP[header_pos..].contains("mcp-github"),
            "mcp 行必须位于最后一次段头之后"
        );
        let rows = mcp_rows(REAL_DUMP);
        assert_eq!(rows.len(), 4, "应提取 4 条 mcp-client 行");
        let expected = "C:\\Users\\ADMINI~1\\AppData\\Local\\Temp\\adr0006-e2e\\dsh-home\\cordis.patch.yml";
        for row in &rows {
            assert_eq!(
                row.section_owner, expected,
                "行 {} 的来源层必须是 home patch 文件（实际 {}）",
                row.row_id, row.section_owner
            );
        }
        // 段头 540 行处开始 → 这 4 行都在该段的覆盖区间内
        assert!(REAL_DUMP.lines().count() > 540);
        assert_eq!(rows[0].server_name().as_deref(), Some("github"));
        assert_eq!(rows[3].server_name().as_deref(), Some("filesystem"));
    }
}
