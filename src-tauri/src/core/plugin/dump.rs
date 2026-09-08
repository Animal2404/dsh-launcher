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
}
