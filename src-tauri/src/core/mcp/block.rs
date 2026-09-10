//! 受管 MCP 区块：两段式（`insert:` 声明段 + `id`/`disabled:` 定向段）的读写
//!
//! 官方机制事实（`$DSH_SRC`，探针实测见 ADR-0006 §Context 1）：
//! - patch 层的声明**必须**包在 `- insert:` 里 —— 顶层裸 `- id:`+`name:`+`config:`
//!   只会打印 `patch: entry "X" not found`，合成树中不产生任何行；
//! - 启停是行级 `disabled` 覆盖，两种等价写法：① insert 行内写 `disabled`；
//!   ② 另起 `- id: <rowId>` + `disabled` 定向条目。本模块采用 **②**（D2）：
//!   启停只触碰数行，`config` 大块在状态操作中**字节可证不被触碰**；
//! - 定向条目可命中**同层后置条目与全部更早层**（home 层是最后持久层）；
//! - 若目标 `id` 无任何声明在先，dsh 会打印 `entry not found` 告警 →
//!   不变量 #2 要求**写前校验**、`remove` 时两段同步删除。
//!
//! 通用层复用：marker 定位、`[]` 占位符处理、块外逐字节保留、幂等判定、
//! 同文件写锁全部来自 `core::plugin::managed` 的 `read_body` / `apply_family`；
//! 本模块只负责**家族自己的区块体语法**（解析 + 渲染）。

use crate::core::plugin::managed::{self, BlockFamily, BlockOutcome};
use crate::core::mcp::entry::{
    validate_declare, validate_directive, McpBlock, McpDeclare, McpDirective, McpTransport,
};
use std::path::Path;

/// mcp 区块 marker 前缀（版本号参与解析：不识别旧版本的 marker 视为无区块）
pub const MCP_BEGIN_PREFIX: &str = "# >>> dsh-launcher mcp ";
/// mcp 区块结束 marker 前缀
pub const MCP_END_PREFIX: &str = "# <<< dsh-launcher mcp ";
/// 当前区块版本
pub const MCP_MARK_VERSION: &str = "v1";

/// mcp 区块家族（交给通用层按 marker 读写）
pub const MCP: BlockFamily = BlockFamily {
    begin_prefix: MCP_BEGIN_PREFIX,
    end_prefix: MCP_END_PREFIX,
    version: MCP_MARK_VERSION,
    description: "MCP 受管区块",
};

/// mcp 区块起始 marker 完整文本
pub fn mark_begin() -> String {
    MCP.mark_begin()
}
/// mcp 区块结束 marker 完整文本
pub fn mark_end() -> String {
    MCP.mark_end()
}

/// 声明段条目的列缩进（`- id:` 中 `-` 所在列）
const DECLARE_INDENT: &str = "    ";
/// 声明段条目**键**的列缩进（`id` / `name` / `config` 所在列）
const DECLARE_KEY_INDENT: &str = "      ";
/// 声明段条目键相对于条目行的缩进量
const DECLARE_KEY_SHIFT: usize = DECLARE_KEY_INDENT.len() - DECLARE_INDENT.len();

/// `config_raw` 的约定缩进：首层字段相对缩进恒为 2 空格，**除此之外逐字节保留**
/// （缩进层级、内嵌注释、`!!js` 折叠标量、引号风格都不动）。
pub const CONFIG_RAW_BASE_INDENT: usize = 2;

/// 去掉行内注释（` # ` 之后），用于解析 `- id: x  # note`。
fn strip_inline_comment(line: &str) -> &str {
    match line.find(" # ") {
        Some(pos) => &line[..pos],
        None => line,
    }
}

/// 去掉 YAML 单/双引号包裹（js-yaml 会对含 `@`/`/` 的值加引号）。
pub fn unquote(value: &str) -> &str {
    let trimmed = value.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('\'') && trimmed.ends_with('\''))
            || (trimmed.starts_with('"') && trimmed.ends_with('"')))
    {
        return &trimmed[1..trimmed.len() - 1];
    }
    trimmed
}

/// 前导空格数
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// 是否为空行或注释行
fn is_blank_or_comment(trimmed: &str) -> bool {
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// 把 `config` 子树原文归一化到约定缩进（首层字段 = 2 空格）。
///
/// 相对缩进层级、内嵌注释、`!!js` 折叠标量与引号风格**逐字节保留**，只做整体
/// 缩进平移：先去掉各行的基准缩进，再统一补 `CONFIG_RAW_BASE_INDENT` 个空格。
/// 归一化后「渲染→解析→再渲染」逐字节稳定（幂等的字节基础）。
///
/// 注释行与空行不参与基准缩进计算，但其**实际缩进同样随基准平移**（保持相对位置）。
pub fn normalize_config_raw(raw: &str) -> String {
    let mut lines: Vec<&str> = raw
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    // 只裁掉首尾空行（内部的空行与注释保留）
    while lines.first().map(|line| line.trim().is_empty()).unwrap_or(false) {
        lines.remove(0);
    }
    while lines.last().map(|line| line.trim().is_empty()).unwrap_or(false) {
        lines.pop();
    }
    if lines.is_empty() {
        return String::new();
    }
    let base = lines
        .iter()
        .filter(|line| !is_blank_or_comment(line.trim_start()))
        .map(|line| indent_of(line))
        .min()
        .unwrap_or(0);
    let pad = " ".repeat(CONFIG_RAW_BASE_INDENT);
    lines
        .iter()
        .map(|line| {
            if line.trim().is_empty() {
                return String::new();
            }
            // 与基准同深或更深的行：去掉基准缩进后统一补约定缩进
            let body = if line.len() > base { &line[base..] } else { "" };
            format!("{pad}{body}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// 解析 `config` 子树：返回 `(config_raw, 消费到的行号)`。
///
/// 取「所有属于该 `config:` 键的行」= 紧随其后、缩进**严格大于**键缩进的行，
/// 保留原始相对缩进。这样 `!!js` 折叠标量（dump 会把它重排为多行）与内嵌注释
/// 都被整段搬运，字段嵌套层级不变。
fn take_config_subtree(lines: &[&str], key_index: usize) -> (String, usize) {
    let key_indent = indent_of(lines[key_index]);
    let mut end = key_index + 1;
    while end < lines.len() {
        let trimmed = lines[end].trim_start();
        // 段标签 `# == ` 是硬边界（dump 的 config 体不含它），防止把段头吞进 config
        if trimmed.starts_with("# == ") {
            break;
        }
        if is_blank_or_comment(trimmed) {
            end += 1;
            continue;
        }
        if indent_of(lines[end]) > key_indent {
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

/// 解析受管 MCP 区块体（marker 之间的内容）。
///
/// 不变量 #1：区块**只由两段构成** —— 一个 `- insert:` 声明段 + 零到多个
/// `- id:`/`disabled:` 定向条目；任何其它行都判定为区块契约破裂（拒绝写入）。
pub fn parse(body: &str) -> Result<McpBlock, String> {
    let all: Vec<&str> = body.lines().collect();
    // 跳过前导空行/注释（marker 之后通常紧跟一个换行）
    let mut index = 0usize;
    while index < all.len() && is_blank_or_comment(all[index].trim()) {
        index += 1;
    }

    let mut block = McpBlock::default();

    // ---------- 段 1：`- insert:` 声明段（可缺省：只做定向覆盖的情形） ----------
    if index < all.len() && indent_of(all[index]) == 0 && all[index].trim_start().starts_with("- insert:") {
        index += 1;
        // 逐条声明：`- id: <rowId>` 出现在 4 空格缩进处
        while index < all.len() {
            let line = all[index].strip_suffix('\r').unwrap_or(all[index]);
            let trimmed = line.trim_start();
            if is_blank_or_comment(trimmed) {
                index += 1;
                continue;
            }
            if indent_of(line) != DECLARE_INDENT.len() || !trimmed.starts_with("- id: ") {
                break;
            }
            let row_id = unquote(strip_inline_comment(&trimmed["- id: ".len()..])).to_string();
            // 安全字符检查（启动器拒绝把畸形 id 当作自己的声明）
            managed::validate_row_id(&row_id)?;
            index += 1;

            let mut config_raw = String::new();

            while index < all.len() {
                let entry_line = all[index].strip_suffix('\r').unwrap_or(all[index]);
                let entry_trimmed = entry_line.trim_start();
                if is_blank_or_comment(entry_trimmed) {
                    index += 1;
                    continue;
                }
                let entry_indent = indent_of(entry_line);
                // 回到条目层或更浅 → 本条结束
                if entry_indent <= DECLARE_INDENT.len() {
                    break;
                }
                // 键行必须恰好比条目行深 DECLARE_KEY_SHIFT（6 = 4 + 2）
                if entry_indent != DECLARE_INDENT.len() + DECLARE_KEY_SHIFT {
                    return Err(format!(
                        "mcp 区块声明 {row_id} 出现非法缩进行（键必须缩进 {} 列）：{}",
                        DECLARE_INDENT.len() + DECLARE_KEY_SHIFT,
                        entry_trimmed.trim_end()
                    ));
                }
                if let Some(rest) = entry_trimmed.strip_prefix("name:") {
                    // 官方包名（启动器渲染时固定写上；解析时只做形态校验）
                    let name = unquote(strip_inline_comment(rest)).to_string();
                    if name != MCP_CLIENT_PACKAGE {
                        return Err(format!(
                            "mcp 区块声明 {row_id} 的 name 不是官方 MCP 插件（{name}）"
                        ));
                    }
                    index += 1;
                    continue;
                }
                if let Some(rest) = entry_trimmed.strip_prefix("config:") {
                    if !rest.trim().is_empty() {
                        return Err(format!(
                            "mcp 区块第 {} 行的 config 不是子树形态（不允许 `config: {{...}}`）",
                            index + 1
                        ));
                    }
                    let (raw, next) = take_config_subtree(&all, index);
                    if !config_raw.is_empty() {
                        return Err(format!("mcp 区块声明 {row_id} 出现重复的 config 子树"));
                    }
                    config_raw = normalize_config_raw(&raw);
                    index = next;
                    continue;
                }
                // 其它键不应出现在声明条目层（config 内层已整体搬运）
                return Err(format!(
                    "mcp 区块声明 {row_id} 出现未知键：{}",
                    entry_trimmed.trim_end()
                ));
            }

            // serverName / transport 是 config 子树的**只读视图**（D4：不重渲染）
            let server_name = config_scalar(&config_raw, "serverName");
            let transport =
                config_scalar(&config_raw, "transport").and_then(|value| McpTransport::parse(&value));

            block.declares.push(McpDeclare {
                row_id,
                server_name,
                transport,
                config_raw,
            });
        }
    }

    // ---------- 段 2：`- id:` / `disabled:` 定向段 ----------
    while index < all.len() {
        let line = all[index].strip_suffix('\r').unwrap_or(all[index]);
        let trimmed = line.trim_start();
        if is_blank_or_comment(trimmed) {
            index += 1;
            continue;
        }
        if trimmed.starts_with("- insert:") {
            return Err(
                "mcp 区块出现第二个声明段（不变量 #1：区块只由两段构成，声明段必须在前）"
                    .to_string(),
            );
        }
        // 定向条目：列 0 的 `- id: <rowId>`
        let Some(rest) = trimmed.strip_prefix("- id: ") else {
            return Err(format!(
                "mcp 区块出现非法行（只允许 `- insert:`、4 空格缩进的声明条目、`- id:` 定向条目）：{}",
                trimmed.trim_end()
            ));
        };
        if indent_of(line) != 0 {
            return Err(format!(
                "mcp 区块的定向条目必须列 0 起始：{}",
                trimmed.trim_end()
            ));
        }
        let row_id = unquote(strip_inline_comment(rest)).to_string();
        managed::validate_row_id(&row_id)?;
        index += 1;
        // 紧随的 `  disabled: true|false`
        let mut disabled: Option<bool> = None;
        while index < all.len() {
            let attr_line = all[index].strip_suffix('\r').unwrap_or(all[index]);
            let attr_trimmed = attr_line.trim_start();
            if is_blank_or_comment(attr_trimmed) {
                index += 1;
                continue;
            }
            if indent_of(attr_line) != 2 {
                break;
            }
            let Some(value) = attr_trimmed.strip_prefix("disabled:") else {
                return Err(format!(
                    "mcp 区块定向条目 {row_id} 出现未知属性：{}",
                    attr_trimmed.trim_end()
                ));
            };
            let value = unquote(strip_inline_comment(value)).to_string();
            let parsed = match value.as_str() {
                "true" => true,
                "false" => false,
                other => {
                    return Err(format!(
                        "mcp 区块定向条目 {row_id} 的 disabled 非法（只允许 true/false）：{other:?}"
                    ))
                }
            };
            if disabled.is_some() {
                return Err(format!("mcp 区块定向条目 {row_id} 出现重复的 disabled"));
            }
            disabled = Some(parsed);
            index += 1;
        }
        let Some(disabled) = disabled else {
            return Err(format!("mcp 区块定向条目 {row_id} 缺少 disabled"));
        };
        block.directives.push(McpDirective { row_id, disabled });
    }

    Ok(block)
}

/// 官方 MCP 插件包名（声明段的 `name:` 必须等于它）
pub const MCP_CLIENT_PACKAGE: &str = "@deepseek-ai/dsh-mcp-client";

/// 从 config 子树原文里取一个**顶层标量**（只读视图；不做 YAML 反序列化）。
///
/// 顶层 = 与子树第一条非空行同缩进的那一层（子树的首层字段）。遇到更深缩进
/// （嵌套映射 / 多行标量）或空值即视为该键不是顶层标量，返回 None。
pub fn config_scalar(config_raw: &str, key: &str) -> Option<String> {
    let base_indent = config_raw
        .lines()
        .map(|raw| raw.strip_suffix('\r').unwrap_or(raw))
        .filter(|line| !is_blank_or_comment(line.trim_start()))
        .map(indent_of)
        .next()?;
    let needle = format!("{key}:");
    for raw in config_raw.lines() {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim_start();
        if is_blank_or_comment(trimmed) {
            continue;
        }
        if indent_of(line) != base_indent {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix(&needle) else {
            continue;
        };
        let value = strip_inline_comment(rest).trim();
        if value.is_empty() {
            return None;
        }
        return Some(unquote(value).to_string());
    }
    None
}

/// 渲染受管 MCP 区块体（不含 marker）。
///
/// - 声明段按 `(serverName, rowId)` 字典序；定向段按 `rowId` 字典序（不变量 #4）；
/// - `config` 子树按原始文本整段搬运（只做缩进平移，不重排字段）；
/// - 无内容 → 空串（通用层据此删除区块）。
pub fn render(block: &McpBlock, eol: &str) -> Result<String, String> {
    let mut declares = block.declares.clone();
    // 本启动器声明的行 id 恒为 `mcp-<serverName>`，两条声明不可能同 serverName；
    // 仍以 row_id 兜底排序，保证同一期望态渲染出同一字节序列。
    declares.sort_by(|a, b| {
        (a.server_name.as_deref(), a.row_id.as_str())
            .cmp(&(b.server_name.as_deref(), b.row_id.as_str()))
    });
    for declare in &declares {
        validate_declare(declare)?;
    }
    let mut directives = block.directives.clone();
    directives.sort_by(|a, b| a.row_id.cmp(&b.row_id));
    directives.dedup_by(|a, b| a.row_id == b.row_id);
    for directive in &directives {
        validate_directive(directive)?;
    }

    let mut out = String::new();
    if !declares.is_empty() {
        out.push_str("- insert:");
        out.push_str(eol);
        for declare in &declares {
            out.push_str(&format!("{DECLARE_INDENT}- id: {}{eol}", declare.row_id));
            out.push_str(&format!(
                "{DECLARE_KEY_INDENT}name: {}{eol}",
                managed::yaml_quote(MCP_CLIENT_PACKAGE)
            ));
            out.push_str(&format!("{DECLARE_KEY_INDENT}config:{eol}"));
            for line in declare.config_raw.lines() {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if line.trim().is_empty() {
                    continue;
                }
                out.push_str(DECLARE_KEY_INDENT);
                out.push_str(line);
                out.push_str(eol);
            }
        }
    }
    for directive in &directives {
        out.push_str(&format!("- id: {}{eol}", directive.row_id));
        out.push_str(&format!(
            "  disabled: {}{eol}",
            if directive.disabled { "true" } else { "false" }
        ));
    }
    Ok(out)
}

/// 读取受管 MCP 区块（无区块返回 `None`；marker 异常返回 Err）。
pub fn read(path: &Path) -> Result<Option<McpBlock>, String> {
    let Some(body) = managed::read_body(path, MCP)? else {
        return Ok(None);
    };
    if body.trim().is_empty() {
        return Ok(Some(McpBlock::default()));
    }
    Ok(Some(parse(&body)?))
}

/// 把受管 MCP 区块写入文件（只动本区块，块外逐字节保留）。
///
/// - 空区块且无区块 → `Unchanged`（不创建文件）；
/// - 空区块且有区块 → 删除区块；
/// - 渲染结果与现有区块一致 → `Unchanged`（不落盘，幂等）。
pub fn apply(path: &Path, block: &McpBlock) -> Result<BlockOutcome, String> {
    let family = MCP;
    let begin = family.mark_begin();
    let end = family.mark_end();
    // 行尾风格与本文件既有内容一致（通用层同样按此判定，这里先读一次用于渲染）
    let eol = current_eol(path);
    let body = if block.is_empty() {
        None
    } else {
        Some(render(block, eol)?)
    };
    let _ = (&begin, &end);
    managed::apply_family(path, family, body.as_deref())
}

/// 读取文件当前行尾风格（默认 LF）；文件不存在时返回 LF。
fn current_eol(path: &Path) -> &'static str {
    match std::fs::read_to_string(path) {
        Ok(content) if content.contains("\r\n") => "\r\n",
        _ => "\n",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::mcp::entry::{default_row_id, McpDeclare, McpDirective};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-launcher-mcp-block-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("cordis.patch.yml")
    }

    fn declare(server: &str, config_raw: &str) -> McpDeclare {
        McpDeclare {
            row_id: default_row_id(server),
            server_name: Some(server.to_string()),
            transport: config_scalar(config_raw, "transport")
                .and_then(|value| McpTransport::parse(&value)),
            config_raw: config_raw.to_string(),
        }
    }

    /// 真实形态的 config 子树（含内嵌注释与 `!!js` 折叠标量，取自本机实测 dump）。
    /// `config_raw` 的约定缩进：首层字段 2 空格；其余层级按真实 dump 的相对深度。
    ///
    /// 注：显式用 `concat!` 拼接，避免 Rust `\` 续行吞掉行首空格。
    const GITHUB_CONFIG: &str = concat!(
        "  serverName: github\n",
        "  transport: streamable-http\n",
        "  url: https://api.githubcopilot.com/mcp/\n",
        "  headers:\n",
        "    # 官方建议：密钥不落盘 YAML\n",
        "    Authorization: !!js >-\n",
        "      (() => { try { const t = process.getBuiltinModule('node:child_process')",
        ".execSync('gh auth token').toString().trim(); if (t) return `Bearer ${t}` }",
        " catch {} return `Bearer ''` })()\n",
        "  toolCallTimeoutMs: 60000\n",
        "  failOnStartupError: false",
    );

    const SHADCN_CONFIG: &str = concat!(
        "  serverName: shadcn\n",
        "  transport: stdio\n",
        "  command: shadcn\n",
        "  args: [mcp]\n",
        "  cwd: !!js process.cwd()",
    );

    #[test]
    fn test_normalize_config_raw_shifts_indent_and_stays_stable() {
        // 4 空格基准 → 平移到 2 空格基准，相对层级与内嵌注释保留
        let shifted = concat!(
            "    serverName: a\n",
            "    headers:\n",
            "      # 注释\n",
            "      Authorization: !!js >-\n",
            "        expr()\n",
            "    timeout: 1",
        );
        let expected = concat!(
            "  serverName: a\n",
            "  headers:\n",
            "    # 注释\n",
            "    Authorization: !!js >-\n",
            "      expr()\n",
            "  timeout: 1",
        );
        assert_eq!(normalize_config_raw(shifted), expected);
        // 幂等：再次归一化不变（渲染→解析→再渲染 的字节基础）
        let normalized = normalize_config_raw(shifted);
        assert_eq!(normalize_config_raw(&normalized), normalized);
        // 已符合约定缩进时逐字节不变
        assert_eq!(normalize_config_raw(SHADCN_CONFIG), SHADCN_CONFIG);
        assert_eq!(normalize_config_raw(GITHUB_CONFIG), GITHUB_CONFIG);
        // 少于约定缩进的基准被补齐
        assert_eq!(normalize_config_raw("serverName: a"), "  serverName: a");
        // 首尾空行被裁掉；空内容得到空串
        assert_eq!(normalize_config_raw("\n\n  a: 1\n\n"), "  a: 1");
        assert_eq!(normalize_config_raw("   \n\n"), "");
    }

    #[test]
    fn test_render_parse_roundtrip_is_byte_stable() {
        let block = McpBlock {
            declares: vec![declare("shadcn", SHADCN_CONFIG), declare("github", GITHUB_CONFIG)],
            directives: vec![
                McpDirective {
                    row_id: "mcp-github".into(),
                    disabled: false,
                },
                McpDirective {
                    row_id: "mcp-shadcn".into(),
                    disabled: true,
                },
            ],
        };
        let rendered = render(&block, "\n").unwrap();
        let reparsed = parse(&rendered).unwrap();
        // 渲染 → 解析 → 再渲染 必须逐字节一致（幂等的字节基础）
        assert_eq!(render(&reparsed, "\n").unwrap(), rendered);
        // 声明段按 (serverName, rowId) 字典序：github 在 shadcn 之前
        assert!(
            rendered.find("mcp-github").unwrap() < rendered.find("mcp-shadcn").unwrap(),
            "{rendered}"
        );
        // 定向段按 rowId 字典序
        let dir_start = rendered.find("- id: mcp-github\n  disabled").unwrap();
        let dir_shadcn = rendered.find("- id: mcp-shadcn\n  disabled").unwrap();
        assert!(dir_start < dir_shadcn);
        // `!!js` 折叠标量与内嵌注释逐字节保留
        assert!(rendered.contains("Authorization: !!js >-"), "{rendered}");
        assert!(rendered.contains("# 官方建议：密钥不落盘 YAML"));
        assert!(rendered.contains("cwd: !!js process.cwd()"));
    }

    #[test]
    fn test_config_subtree_moved_verbatim() {
        let block = McpBlock {
            declares: vec![declare("github", GITHUB_CONFIG)],
            directives: vec![],
        };
        let rendered = render(&block, "\n").unwrap();
        let reparsed = parse(&rendered).unwrap();
        // config 子树原文字节一致（含 `!!js`、内嵌注释、引号风格）
        assert_eq!(reparsed.declares[0].config_raw, GITHUB_CONFIG);
        // 只读视图能取到官方字段
        assert_eq!(reparsed.declares[0].server_name.as_deref(), Some("github"));
        assert_eq!(
            reparsed.declares[0].transport,
            Some(McpTransport::StreamableHttp)
        );
        // 声明必须包在 insert 里（官方 patch 层语义）
        assert!(rendered.starts_with("- insert:\n"), "{rendered}");
    }

    #[test]
    fn test_directive_only_block_has_no_insert_segment() {
        // 只对用户手写（external）行做定向覆盖：区块不应出现 insert 段
        let block = McpBlock {
            declares: vec![],
            directives: vec![McpDirective {
                row_id: "mcp-github".into(),
                disabled: true,
            }],
        };
        let rendered = render(&block, "\n").unwrap();
        assert_eq!(rendered, "- id: mcp-github\n  disabled: true\n");
        assert!(!rendered.contains("insert"));
        assert_eq!(parse(&rendered).unwrap(), block);
    }

    #[test]
    fn test_apply_appends_block_and_preserves_outside_bytes() {
        let path = temp_path("append");
        let user = concat!(
            "# ~/.dsh/cordis.patch.yml — 用户手写机器级 patch 层\n",
            "# 用户自己的注释\n",
            "\n",
            "- insert:\n",
            "    - id: mcp-github\n",
            "      name: '@deepseek-ai/dsh-mcp-client'\n",
            "      config:\n",
            "        serverName: github\n",
            "        transport: streamable-http\n",
            "        url: https://user-written.example/mcp\n",
        );
        std::fs::write(&path, user).unwrap();

        let block = McpBlock {
            declares: vec![declare("shadcn", SHADCN_CONFIG)],
            directives: vec![McpDirective {
                row_id: "mcp-shadcn".into(),
                disabled: false,
            }],
        };
        assert_eq!(apply(&path, &block).unwrap(), BlockOutcome::Written);

        let content = std::fs::read_to_string(&path).unwrap();
        // 块外逐字节保留（用户手写段完整在前）
        assert!(content.starts_with(user), "块外字节必须不变");
        assert!(content.contains(&mark_begin()));
        assert!(content.contains(&mark_end()));
        // 幂等：再次写 → Unchanged，且字节不变
        let before = std::fs::read(&path).unwrap();
        assert_eq!(apply(&path, &block).unwrap(), BlockOutcome::Unchanged);
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn test_apply_updates_only_block_and_keeps_user_edit_after_it() {
        let path = temp_path("update");
        let block = McpBlock {
            declares: vec![declare("shadcn", SHADCN_CONFIG)],
            directives: vec![McpDirective {
                row_id: "mcp-shadcn".into(),
                disabled: false,
            }],
        };
        apply(&path, &block).unwrap();
        // 用户在区块之后追加内容
        let with_user_edit = std::fs::read_to_string(&path).unwrap() + "\n# 用户追加在最后\n";
        std::fs::write(&path, &with_user_edit).unwrap();

        let changed = McpBlock {
            directives: vec![McpDirective {
                row_id: "mcp-shadcn".into(),
                disabled: true,
            }],
            ..block.clone()
        };
        assert_eq!(apply(&path, &changed).unwrap(), BlockOutcome::Written);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# 用户追加在最后"));
        assert!(content.contains("  disabled: true"));
        // config 子树在状态操作中字节不变
        assert!(content.contains("    cwd: !!js process.cwd()"));
    }

    #[test]
    fn test_remove_managed_block_deletes_block_and_restores_placeholder() {
        let path = temp_path("remove");
        std::fs::write(&path, "# 只有注释\n[]\n").unwrap();
        let block = McpBlock {
            declares: vec![declare("shadcn", SHADCN_CONFIG)],
            directives: vec![],
        };
        apply(&path, &block).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.lines().any(|line| line.trim() == "[]"), "{content}");

        // 空区块 → 删区块，且补回 `[]`（否则 dsh 解析空文档报错）
        assert_eq!(apply(&path, &McpBlock::default()).unwrap(), BlockOutcome::Written);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains(&mark_begin()));
        assert!(content.trim_end().ends_with("[]"), "{content}");
        // 幂等
        assert_eq!(apply(&path, &McpBlock::default()).unwrap(), BlockOutcome::Unchanged);
    }

    #[test]
    fn test_broken_markers_fail_loud_with_zero_write() {
        let path = temp_path("broken");
        // 只有起始 marker（不成对）
        let broken = format!("{}\n- id: mcp-a\n  disabled: true\n", mark_begin());
        std::fs::write(&path, &broken).unwrap();
        let err = apply(
            &path,
            &McpBlock {
                declares: vec![],
                directives: vec![McpDirective {
                    row_id: "mcp-a".into(),
                    disabled: false,
                }],
            },
        )
        .unwrap_err();
        assert!(err.contains("不成对") || err.contains("缺少结束"), "{err}");
        // 零写入
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);

        // marker 重复
        let duplicated = format!("{}\n{}\n{}\n", mark_begin(), mark_begin(), mark_end());
        std::fs::write(&path, &duplicated).unwrap();
        let read_err = read(&path).unwrap_err();
        assert!(read_err.contains("不成对") || read_err.contains("重复"), "{read_err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), duplicated);

        // 顶层非 YAML 数组
        let not_array = temp_path("not-array");
        std::fs::write(&not_array, "root: {}\n").unwrap();
        let err = apply(
            &not_array,
            &McpBlock {
                declares: vec![declare("a", "    serverName: a\n    transport: stdio\n    command: a")],
                directives: vec![],
            },
        )
        .unwrap_err();
        assert!(err.contains("不是 YAML 数组"), "{err}");
        assert_eq!(std::fs::read_to_string(&not_array).unwrap(), "root: {}\n");
    }

    #[test]
    fn test_parse_rejects_invariant_violations() {
        // 不变量 #1：区块只由两段构成
        let err = parse("config:\n  x: 1\n").unwrap_err();
        assert!(err.contains("非法行"), "{err}");        // 第二个声明段
        let two_inserts = "- insert:\n    - id: mcp-a\n      name: '@deepseek-ai/dsh-mcp-client'\n      config:\n        serverName: a\n- insert:\n";
        let err = parse(two_inserts).unwrap_err();
        assert!(err.contains("第二个声明段"), "{err}");
        // 定向条目缺少 disabled
        let no_disabled = "- id: mcp-a\n";
        let err = parse(no_disabled).unwrap_err();
        assert!(err.contains("缺少 disabled"), "{err}");
        // disabled 非布尔
        let bad_disabled = "- id: mcp-a\n  disabled: !!js true\n";
        let err = parse(bad_disabled).unwrap_err();
        assert!(err.contains("非法"), "{err}");
        // 非官方包名
        let wrong_name = "- insert:\n    - id: mcp-a\n      name: other-package\n      config:\n        serverName: a\n";
        let err = parse(wrong_name).unwrap_err();
        assert!(err.contains("不是官方 MCP 插件"), "{err}");
        // 流式 config（不允许）
        let inline_config = "- insert:\n    - id: mcp-a\n      name: '@deepseek-ai/dsh-mcp-client'\n      config: { serverName: a }\n";
        let err = parse(inline_config).unwrap_err();
        assert!(err.contains("子树形态"), "{err}");
    }

    #[test]
    fn test_render_rejects_unsafe_row_id() {
        let block = McpBlock {
            declares: vec![McpDeclare {
                row_id: "mcp-a\n- id: evil".into(),
                server_name: Some("a".into()),
                transport: Some(McpTransport::Stdio),
                config_raw: "    serverName: a\n    transport: stdio\n    command: a".into(),
            }],
            directives: vec![],
        };
        let err = render(&block, "\n").unwrap_err();
        assert!(err.contains("非法行 id"), "{err}");
    }

    #[test]
    fn test_render_rejects_empty_config_raw() {
        let block = McpBlock {
            declares: vec![McpDeclare {
                row_id: "mcp-a".into(),
                server_name: Some("a".into()),
                transport: Some(McpTransport::Stdio),
                config_raw: "   ".into(),
            }],
            directives: vec![],
        };
        assert!(render(&block, "\n").unwrap_err().contains("config 子树为空"));
    }

    #[test]
    fn test_crlf_is_preserved() {
        let path = temp_path("crlf");
        std::fs::write(&path, "# 注释\r\n[]\r\n").unwrap();
        let block = McpBlock {
            declares: vec![declare("shadcn", SHADCN_CONFIG)],
            directives: vec![],
        };
        apply(&path, &block).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\r\n"));
        assert!(content.starts_with("# 注释\r\n"));
        // 渲染时用 CRLF：marker 之间不含裸 LF
        let block_start = content.find(&mark_begin()).unwrap();
        let block_end = content.find(&mark_end()).unwrap();
        assert!(!content[block_start..block_end].contains("\n\n"));
        // 重新读回仍能解析
        assert_eq!(read(&path).unwrap().unwrap().declares.len(), 1);
    }

    #[test]
    fn test_read_without_block_is_none() {
        let path = temp_path("none");
        std::fs::write(&path, "# 只有注释\n[]\n").unwrap();
        assert!(read(&path).unwrap().is_none());
        // 文件不存在也是 None
        assert!(read(&temp_path("missing")).unwrap().is_none());
    }

    #[test]
    fn test_config_scalar_is_read_only_view() {
        // 嵌套字段不算顶层标量
        assert_eq!(config_scalar(GITHUB_CONFIG, "toolCallTimeoutMs").as_deref(), Some("60000"));
        assert_eq!(config_scalar(GITHUB_CONFIG, "failOnStartupError").as_deref(), Some("false"));
        assert_eq!(config_scalar(GITHUB_CONFIG, "Authorization"), None);
        // 值内联注释被去掉
        assert_eq!(
            config_scalar("    serverName: github  # 名字", "serverName").as_deref(),
            Some("github")
        );
        assert_eq!(config_scalar("    serverName:\n", "serverName"), None);
    }
}
