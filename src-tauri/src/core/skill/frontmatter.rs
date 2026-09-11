//! SKILL.md frontmatter 的**逐行外科手术**（纯函数，零 IO、零依赖）
//!
//! 事实依据（deepseek-harness，逐条标注官方出处）：
//! - frontmatter 界定：首行去掉一个尾随 `\r` 后须**恰好等于** `---`；闭合符是其
//!   后第一行同样等于 `---` 的行。见 `packages/skill/skill-filesystem/src/index.ts:909-935`。
//! - 启停语义：`disable-model-invocation: true` 才关闭模型面（`!== true` 即允许），
//!   见同文件 `:992-1002`。**缺省即允许**，故「启用」= 删键。
//! - 布尔接受 YAML 布尔、`1`/`0` 与大小写不敏感的 `true`/`false`/`yes`/`no`/`on`/`off`，
//!   见 `:1010-1029`。
//! - legacy camelCase 键（`disableModelInvocation` 等）被**显式拒绝**，见 `:993-995,1004-1008`。
//!   写入器只写规范 kebab-case 键，否则整个技能会被官方丢弃（仅一条 warn）。
//!
//! 设计铁律（ADR-0007 D8/D9）：
//! 1. **只动目标键那一行**，其余字节逐字节保留（CRLF、BOM 缺失状态、无末尾换行、
//!    块标量、纯多行标量、嵌套映射、非 ASCII、行内注释一律不碰）。
//! 2. 插入点固定为**闭合 `---` 行之前、第 0 列**。绝不插在 `description:` 之后 ——
//!    真实数据存在 `>-`/`>` 块标量与纯多行标量，插进去会改变 YAML 语义。
//! 3. 任何歧义（无 frontmatter / 未闭合 / 键重复 / 值非裸布尔）**一律拒绝**，绝不猜测。
//! 4. 目标状态已达成时返回 [`Toggle::Unchanged`]，调用方**不写盘**（幂等）。

/// 规范键名（官方 kebab-case；写 camelCase 会让技能被官方丢弃）
const KEY_DISABLE_MODEL_INVOCATION: &str = "disable-model-invocation";

/// 外科手术的具名失败原因。每个变体对应一个明确、可向用户解释的拒绝条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// 首行不是 `---`（没有 frontmatter）
    NoFrontmatter,
    /// 找不到闭合 `---`（frontmatter 未闭合）
    Unterminated,
    /// 目标键在同一 frontmatter 内出现多于一次（歧义，不知该改哪个）
    DuplicateKey,
    /// 目标键的值不是裸 `true`/`false`（例如 `yes`/`"true"`/`1`/空值/块标量）
    NonPlainBoolean,
}

impl WriteError {
    /// 面向用户的中文原因说明（用于 UI 与日志）
    pub fn message(self) -> &'static str {
        match self {
            WriteError::NoFrontmatter => "文件缺少 YAML frontmatter（首行不是 ---）",
            WriteError::Unterminated => "frontmatter 未闭合（找不到结束的 ---）",
            WriteError::DuplicateKey => "frontmatter 中 disable-model-invocation 出现多次，无法确定修改哪一处",
            WriteError::NonPlainBoolean => "disable-model-invocation 的值不是裸 true/false，不猜测其含义",
        }
    }
}

/// 只读判定出的启用状态（对应 ADR-0007 状态机）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisableState {
    /// 无该键 → 官方默认允许模型调用
    Unset,
    /// 键存在且解析为 `false` → 显式允许
    Enabled,
    /// 键存在且解析为 `true` → 停用
    Disabled,
    /// 键重复，或值非裸布尔 → 只读，拒绝一切写操作
    Conflict,
    /// 无 frontmatter / 未闭合 → 只读
    Unreadable,
}

/// 写入结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toggle {
    /// 目标状态已达成，**无需写盘**（幂等空操作）
    Unchanged,
    /// 需要写入的新全文
    Rewritten(String),
}

/// frontmatter 的行视图：把文本切成「frontmatter 内的键行索引」与「闭合符位置」
struct Front {
    /// 各行的起始字节偏移（含行尾符）
    line_starts: Vec<usize>,
    /// 各行的结束字节偏移（不含行尾符）
    line_ends: Vec<usize>,
    /// 闭合 `---` 行的索引
    closing: usize,
}

/// 把文本切成行边界。
///
/// 行尾可以是 `\r\n` 或 `\n`；行内容的结束位置**不含**行尾符，故 `\r` 天然被排除在
/// 内容之外，比较时无需特殊处理。孤立 `\r`（老 Mac 行尾）不被当作行分隔（官方解析器
/// 亦然：它用 `indexOf('\n')` 切行，只额外剥一个尾随 `\r`）。
fn line_starts_and_ends(text: &str) -> (Vec<usize>, Vec<usize>) {
    let bytes = text.as_bytes();
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            // 行内容 = [start, i) 去掉可能的尾随 \r
            let mut end = i;
            if end > start && bytes[end - 1] == b'\r' {
                end -= 1;
            }
            starts.push(start);
            ends.push(end);
            start = i + 1;
        }
        i += 1;
    }
    // 末行（可能无行尾符）
    if start < bytes.len() || starts.is_empty() {
        starts.push(start);
        ends.push(bytes.len());
    }
    (starts, ends)
}

/// 定位 frontmatter。返回 `Err` 表示结构性歧义。
fn locate_front(text: &str) -> Result<Front, WriteError> {
    let (line_starts, line_ends) = line_starts_and_ends(text);
    // 首行须恰好是 `---`
    if line_starts.is_empty() {
        return Err(WriteError::NoFrontmatter);
    }
    let first = &text[line_starts[0]..line_ends[0]];
    if first != "---" {
        return Err(WriteError::NoFrontmatter);
    }
    // 闭合符 = 其后第一行恰好是 `---`
    for idx in 1..line_starts.len() {
        if &text[line_starts[idx]..line_ends[idx]] == "---" {
            return Ok(Front {
                line_starts,
                line_ends,
                closing: idx,
            });
        }
    }
    Err(WriteError::Unterminated)
}

/// 解析一个「裸布尔」标量。非裸布尔返回 `None`（不猜测、不规范化）。
///
/// 严格限定为 `true` / `false` 两个字面量：官方接受 `yes`/`on`/`1` 等形式，但本写入器
/// **永不**接受它们 —— 遇到即拒绝（ADR-0007 D9），以免在用户文件里引入风格混用。
fn parse_bare_bool(raw: &str) -> Option<bool> {
    match raw {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// 从一行中提取 `key:` 形态的**第 0 列**键名与值（已 trim 首尾空白）。
///
/// 只认第 0 列（无前导空白）的 `key:` —— 缩进行属于块标量内容或嵌套映射，绝不参与匹配。
/// 这保证嵌套 `metadata:` 里的同名键不会被误当作顶层键。
fn top_level_key_value(line: &str) -> Option<(&str, &str)> {
    // 第 0 列才有资格
    if line.starts_with(' ') || line.starts_with('\t') {
        return None;
    }
    let colon = line.find(':')?;
    let key = &line[..colon];
    // 键名不得含空白（避免把 `a: b: c` 这类误判）；官方键都是 kebab-case
    if key.is_empty() || key.chars().any(|c| c.is_whitespace()) {
        return None;
    }
    let value = line[colon + 1..].trim();
    Some((key, value))
}

/// 统计 frontmatter 内目标键出现的行索引。
fn find_key_lines(text: &str, front: &Front) -> Vec<usize> {
    let mut hits = Vec::new();
    // 仅扫描 frontmatter 体内（跳过首行分隔符与闭合符本身）
    for idx in 1..front.closing {
        let line = &text[front.line_starts[idx]..front.line_ends[idx]];
        if let Some((key, _)) = top_level_key_value(line) {
            if key == KEY_DISABLE_MODEL_INVOCATION {
                hits.push(idx);
            }
        }
    }
    hits
}

/// 只读判定当前状态（宽容：列出用；不因单个坏技能而整体失败）
pub fn disable_state(text: &str) -> DisableState {
    let Ok(front) = locate_front(text) else {
        return DisableState::Unreadable;
    };
    let hits = find_key_lines(text, &front);
    match hits.len() {
        0 => DisableState::Unset,
        1 => {
            let line = &text[front.line_starts[hits[0]]..front.line_ends[hits[0]]];
            match top_level_key_value(line).and_then(|(_, v)| parse_bare_bool(v)) {
                Some(true) => DisableState::Disabled,
                Some(false) => DisableState::Enabled,
                // 非裸布尔（yes/1/"true"/空值/…）→ 冲突，只读
                None => DisableState::Conflict,
            }
        }
        _ => DisableState::Conflict,
    }
}

/// 该行行尾符的字节数（0 = 末行无行尾符）
fn line_ending_len_at(text: &str, front: &Front, idx: usize) -> usize {
    let end = front.line_ends[idx];
    let bytes = text.as_bytes();
    match bytes.get(end) {
        Some(&b'\r') if bytes.get(end + 1) == Some(&b'\n') => 2,
        Some(&b'\n') => 1,
        _ => 0,
    }
}

/// 该行使用的行尾符（用于插入新行时沿用文件风格）。默认 CRLF（本机实测 93/93）。
fn line_ending_at(text: &str, front: &Front, idx: usize) -> &'static str {
    let start = front.line_starts[idx];
    let end = front.line_ends[idx];
    if text[start..end].len() < text[start..].len() && text.as_bytes().get(end) == Some(&b'\r') {
        "\r\n"
    } else if text.as_bytes().get(end) == Some(&b'\n') {
        "\n"
    } else {
        // 末行无行尾符：沿用文件内首个行尾，否则 CRLF（Windows）
        if text.contains("\r\n") {
            "\r\n"
        } else if text.contains('\n') {
            "\n"
        } else {
            "\r\n"
        }
    }
}

/// 外科手术核心：把 `disable-model-invocation` 设置为期望状态。
///
/// - `disable == true`（停用）：键不存在则**插入**于闭合 `---` 之前；已为 `true` → `Unchanged`；
///   为 `false` → **原位替换**该行。
/// - `disable == false`（启用）：键不存在或已为 `false` → `Unchanged`；为 `true` → **删除**该行。
///
/// 除目标行外，**其余字节逐字节保留**。
pub fn set_disable_model_invocation(text: &str, disable: bool) -> Result<Toggle, WriteError> {
    let front = locate_front(text)?;
    let hits = find_key_lines(text, &front);

    // 键重复 → 一律拒绝（不猜该改哪一处）
    if hits.len() > 1 {
        return Err(WriteError::DuplicateKey);
    }

    if let Some(&idx) = hits.first() {
        let line = &text[front.line_starts[idx]..front.line_ends[idx]];
        let current = top_level_key_value(line)
            .and_then(|(_, v)| parse_bare_bool(v))
            .ok_or(WriteError::NonPlainBoolean)?;
        if current == disable {
            return Ok(Toggle::Unchanged);
        }
        if disable {
            // 原位替换为 `true`，保留原行尾符
            let ending = line_ending_at(text, &front, idx);
            let replacement = format!("{KEY_DISABLE_MODEL_INVOCATION}: true{ending}");
            let mut out = String::with_capacity(text.len() + replacement.len());
            out.push_str(&text[..front.line_starts[idx]]);
            out.push_str(&replacement);
            out.push_str(&text[front.line_ends[idx] + ending.len()..]);
            return Ok(Toggle::Rewritten(out));
        }
        // 启用且键为 `true` → **整行删除**（连同它自己的那一个行尾符）。
        //
        // ADR-0007 D7：启用 = 删除该键。理由是官方语义「缺省即允许」—— 留下
        // `disable-model-invocation: false` 与删键语义相同，但删键可让「停用→启用」
        // 精确回到原字节（不变量 2），也让从未动过的文件保持官方默认形态。
        //
        // 只跳过该行**自身**的行尾符，绝不多删后续空行（字节级精确）。
        let skip = line_ending_len_at(text, &front, idx);
        let mut out = String::with_capacity(text.len());
        out.push_str(&text[..front.line_starts[idx]]);
        out.push_str(&text[front.line_ends[idx] + skip..]);
        return Ok(Toggle::Rewritten(out));
    }

    // 键不存在
    if !disable {
        // 启用且无键 → 官方默认即允许，无需写盘
        return Ok(Toggle::Unchanged);
    }
    // 停用且无键 → 插入到闭合 `---` 行之前、第 0 列
    //
    // 插入点选择依据（ADR-0007 D8）：列 0 的行天然终结块标量（`>-`/`>`）与嵌套映射，
    // 因此插在闭合符之前绝不会落进任何标量体或映射体内。
    let closing_start = front.line_starts[front.closing];
    let ending = line_ending_at(text, &front, front.closing);
    let mut out = String::with_capacity(text.len() + 40);
    out.push_str(&text[..closing_start]);
    out.push_str(KEY_DISABLE_MODEL_INVOCATION);
    out.push_str(": true");
    out.push_str(ending);
    out.push_str(&text[closing_start..]);
    Ok(Toggle::Rewritten(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造带 CRLF 的最简技能文本
    fn crlf(fm: &str) -> String {
        format!("---\r\n{fm}---\r\n\r\nBody line.\r\n")
    }

    #[test]
    fn 无_frontmatter_被拒绝() {
        assert_eq!(
            set_disable_model_invocation("# just a doc\n", true),
            Err(WriteError::NoFrontmatter)
        );
        assert_eq!(disable_state("# just a doc\n"), DisableState::Unreadable);
    }

    #[test]
    fn 未闭合_被拒绝() {
        let text = "---\nname: x\ndescription: y\n";
        assert_eq!(
            set_disable_model_invocation(text, true),
            Err(WriteError::Unterminated)
        );
        assert_eq!(disable_state(text), DisableState::Unreadable);
    }

    #[test]
    fn 键重复_被拒绝() {
        let text = "---\nname: x\nname: y\n---\n\nB\n";
        // name 不是目标键，不该误报；改用目标键重复
        assert_eq!(
            set_disable_model_invocation(text, true),
            Ok(Toggle::Rewritten(
                "---\nname: x\nname: y\ndisable-model-invocation: true\n---\n\nB\n".to_string()
            ))
        );
        let dup = "---\nname: x\ndisable-model-invocation: true\ndisable-model-invocation: false\n---\n\nB\n";
        assert_eq!(
            set_disable_model_invocation(dup, true),
            Err(WriteError::DuplicateKey)
        );
        assert_eq!(disable_state(dup), DisableState::Conflict);
    }

    #[test]
    fn 非裸布尔_被拒绝() {
        for bad in ["yes", "no", "on", "off", "1", "0", "\"true\"", "True", ""] {
            let text = format!("---\nname: x\ndescription: y\ndisable-model-invocation: {bad}\n---\n\nB\n");
            assert_eq!(
                set_disable_model_invocation(&text, true),
                Err(WriteError::NonPlainBoolean),
                "值 {bad:?} 应被拒绝"
            );
            assert_eq!(
                disable_state(&text),
                DisableState::Conflict,
                "值 {bad:?} 应为 conflict"
            );
        }
    }

    #[test]
    fn 启用且无键_不写盘() {
        let text = crlf("name: x\r\ndescription: y\r\n");
        assert_eq!(disable_state(&text), DisableState::Unset);
        assert_eq!(
            set_disable_model_invocation(&text, false),
            Ok(Toggle::Unchanged)
        );
    }

    #[test]
    fn 停用且无键_插到闭合符前且保留_crlf() {
        let text = crlf("name: x\r\ndescription: y\r\n");
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(&text, true) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\r\nname: x\r\ndescription: y\r\ndisable-model-invocation: true\r\n---\r\n\r\nBody line.\r\n"
        );
        // 写后状态可解析为 disabled
        assert_eq!(disable_state(&out), DisableState::Disabled);
        // 关键：非目标行的行尾全部保持 CRLF
        assert_eq!(out.matches("\r\n").count(), text.matches("\r\n").count() + 1);
        assert!(!out.contains("\n\n") || !out.replace("\r\n", "").contains('\n'));
    }

    #[test]
    fn 停用已是_true_幂等() {
        let text = crlf("name: x\r\n disable-me: keep\r\ndisable-model-invocation: true\r\n");
        assert_eq!(disable_state(&text), DisableState::Disabled);
        assert_eq!(
            set_disable_model_invocation(&text, true),
            Ok(Toggle::Unchanged)
        );
    }

    #[test]
    fn 启用为_true_删键且可逆回到原字节() {
        let original = crlf("name: x\r\ndescription: y\r\n");
        let Ok(Toggle::Rewritten(disabled)) = set_disable_model_invocation(&original, true) else {
            panic!("应重写");
        };
        let Ok(Toggle::Rewritten(reenabled)) = set_disable_model_invocation(&disabled, false) else {
            panic!("应重写");
        };
        // ADR-0007 不变量 2：停用 → 启用 后逐字节回到操作前
        assert_eq!(reenabled, original);
    }

    #[test]
    fn 删键不多删后续空行() {
        // 键后面紧跟一个空行：删键必须只删该行自身，空行原样保留
        let text = "---\r\nname: x\r\ndescription: y\r\ndisable-model-invocation: true\r\n\r\nmetadata:\r\n  a: b\r\n---\r\n\r\nBody.\r\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, false) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\r\nname: x\r\ndescription: y\r\n\r\nmetadata:\r\n  a: b\r\n---\r\n\r\nBody.\r\n"
        );
    }

    #[test]
    fn 删键在末行无行尾符时仍正确() {
        // 极端形态：目标键是 frontmatter 最后一个内容行且其后无行尾符（不可能出现在
        // 合法 frontmatter 里因为闭合符必须存在，但契约上必须不 panic 且不损坏）
        let text = "---\r\nname: x\r\ndescription: y\r\ndisable-model-invocation: true\r\n---";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, false) else {
            panic!("应重写");
        };
        assert_eq!(out, "---\r\nname: x\r\ndescription: y\r\n---");
    }

    #[test]
    fn 显式_false_切到停用是原位替换() {
        let text = crlf("name: x\r\ndescription: y\r\ndisable-model-invocation: false\r\n");
        assert_eq!(disable_state(&text), DisableState::Enabled);
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(&text, true) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\r\nname: x\r\ndescription: y\r\ndisable-model-invocation: true\r\n---\r\n\r\nBody line.\r\n"
        );
    }

    #[test]
    fn 原位替换保留键序与其余字节() {
        let text = crlf("name: x\r\ndescription: y\r\ndisable-model-invocation: false\r\nlicense: MIT\r\n");
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(&text, true) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\r\nname: x\r\ndescription: y\r\ndisable-model-invocation: true\r\nlicense: MIT\r\n---\r\n\r\nBody line.\r\n"
        );
    }

    #[test]
    fn 显式_false_时启用是幂等空操作() {
        // 显式 `false` 在官方语义里等同「允许模型调用」，故再次启用属已达目标状态 →
        // `Unchanged`。这里**刻意不改写**为删键：用户自己写的 `false` 是合法风格，
        // 无正当理由不应被启动器抹掉（ADR-0007 D8「只动必须动的那一行」）。
        let text = crlf("name: x\r\ndescription: y\r\ndisable-model-invocation: false\r\n");
        assert_eq!(disable_state(&text), DisableState::Enabled);
        assert_eq!(
            set_disable_model_invocation(&text, false),
            Ok(Toggle::Unchanged)
        );
    }

    #[test]
    fn 块标量之后插入不破坏标量() {
        // 真实形态（本机 gpui-bench / gpui-test）：description 用 `>-` 块标量
        let text = "---\r\nname: gpui-bench\r\ndescription: >-\r\n  Line one.\r\n  Line two.\r\n---\r\n\r\nBody.\r\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        // 插入点必须在缩进续行之后、闭合符之前 —— 否则会落进标量体
        assert_eq!(
            out,
            "---\r\nname: gpui-bench\r\ndescription: >-\r\n  Line one.\r\n  Line two.\r\ndisable-model-invocation: true\r\n---\r\n\r\nBody.\r\n"
        );
        assert_eq!(out.matches("Line two.").count(), 1);
    }

    #[test]
    fn 纯多行标量之后插入不破坏标量() {
        // 真实形态（本机 composition-patterns）：description 空值 + 缩进续行
        let text = "---\r\nname: vercel-composition-patterns\r\ndescription:\r\n  First.\r\n  Second.\r\n---\r\n\r\nBody.\r\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\r\nname: vercel-composition-patterns\r\ndescription:\r\n  First.\r\n  Second.\r\ndisable-model-invocation: true\r\n---\r\n\r\nBody.\r\n"
        );
    }

    #[test]
    fn 嵌套映射里的同名键不被误认为顶层键() {
        // 真实形态（本机 rust-skills）：metadata 嵌套
        let text = "---\r\nname: rust-skills\r\ndescription: >\r\n  Big.\r\nmetadata:\r\n  author: x\r\n  disable-model-invocation: false\r\n---\r\n\r\nBody.\r\n";
        // 顶层无该键 → Unset（缩进的那一行不算）
        assert_eq!(disable_state(text), DisableState::Unset);
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        // 嵌套那行原样保留，新键插在闭合符之前
        assert!(out.contains("  disable-model-invocation: false\r\n"));
        assert!(out.contains("disable-model-invocation: true\r\n---\r\n"));
    }

    #[test]
    fn 无末尾换行的文件不被改写尾部() {
        // 真实形态（本机 web-artifacts-builder）：最后一行无行尾符
        let text = "---\r\nname: web-artifacts-builder\r\ndescription: y\r\n---\r\n\r\nBody ends here.";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        assert!(out.ends_with("Body ends here."));
        assert!(!out.ends_with("\r\n"));
    }

    #[test]
    fn lf_文件沿用_lf() {
        let text = "---\nname: x\ndescription: y\n---\n\nBody.\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        assert_eq!(
            out,
            "---\nname: x\ndescription: y\ndisable-model-invocation: true\n---\n\nBody.\n"
        );
        assert!(!out.contains('\r'));
    }

    #[test]
    fn 非_ascii_与行内注释不受影响() {
        let text = "---\r\nname: shadcn\r\ndescription: Manages components — adding stuff\r\nlicense: MIT # keep me\r\n---\r\n\r\nBody.\r\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        assert!(out.contains("Manages components — adding stuff"));
        assert!(out.contains("license: MIT # keep me"));
    }

    #[test]
    fn 首行前后空白不算_frontmatter() {
        assert_eq!(
            set_disable_model_invocation(" ---\nname: x\n---\n", true),
            Err(WriteError::NoFrontmatter)
        );
        assert_eq!(
            set_disable_model_invocation("----\nname: x\n----\n", true),
            Err(WriteError::NoFrontmatter)
        );
    }

    #[test]
    fn crlf_与_lf_混排时各自保留() {
        let text = "---\nname: x\r\ndescription: y\n---\n\nBody.\n";
        let Ok(Toggle::Rewritten(out)) = set_disable_model_invocation(text, true) else {
            panic!("应重写");
        };
        // 插入行沿用闭合符那一行的行尾（LF）
        assert_eq!(
            out,
            "---\nname: x\r\ndescription: y\ndisable-model-invocation: true\n---\n\nBody.\n"
        );
    }
}
