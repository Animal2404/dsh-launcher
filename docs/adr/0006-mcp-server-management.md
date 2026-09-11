# ADR-0006 — 三类能力的官方合规收敛：MCP Server 管理（Part A）与插件/技能合规修复（Part B）

- **状态**：Accepted（`0.7.0` 于 2026-09-10 实施完成；本文于 2026-09-12 **据代码与被引用锚点重建**）
- **日期**：2026-09-10（原始决策）／2026-09-12（重建归档）
- **范围**：`dsh-launcher` 新增 **MCP Server 管理**（Part A），并对既有**插件管理**与**技能共享**做官方合规核验与修复（Part B）。**不修改 `deepseek-harness` 任何代码**；未新增退出码；未引入新依赖。
- **事实依据**：`deepseek-harness` 检出 `C:\Users\Administrator\AppData\Local\dsh-launcher\github-dsh\deepseek-harness`（下称 `$DSH_SRC`）。MCP 机制经本机探针实测（见 §Context 1），插件/技能机制沿用 ADR-0005 与 ADR-0007 的既有结论。
- **重建说明**：本文件此前**只有编号与引用、没有正文**（`docs/adr/` 中缺 0006，而全仓有 49 处按 `§章节` 与 `D编号` 引用它，见 ADR-0009 P2-22/D15）。现按代码注释、`CHANGELOG.md:238-311` 的 0.7.0 施工记录、以及 `CONTEXT.md:68-80` 的词表**反向重建**为权威文本；**每个锚点都标注其代码落点**，可逐条核对，**不含任何无法在代码中定位的断言**。
- **取代/关联**：D17 修订 ADR-0005 D8 的真源锚点（由 `~/.agents/agent` 改为官方 `agentsHome` 根）；D18 取代 ADR-0005 的白名单纪律，登记为**唯一例外**；ADR-0007 随后退役了 ADR-0005 的技能共享前端入口。

---

## Context

### 1. MCP 的官方机制（探针实测，非推断）

| # | 机制事实 | 影响 |
| --- | --- | --- |
| 1 | 一个 MCP server = cordis 配置树中的一行，`name: '@deepseek-ai/dsh-mcp-client'`，其 `config.serverName` 是面向模型的命名空间（工具名 `mcp__<serverName>__<tool>`） | 管理标识键选 `serverName` 而非行 id |
| 2 | patch 层的声明**必须**包在 `- insert:` 里；顶层裸 `- id:`+`name:`+`config:` 只打印 `patch: entry "X" not found`，合成树中不产生行 | 声明段必须是 `- insert:` 列表 |
| 3 | 启停是**行级 `disabled` 覆盖**，两种等价写法：① insert 行内写 `disabled`；② 另起 `- id: <rowId>` + `disabled` 定向条目 | 本 ADR 选 **②**（见 D2） |
| 4 | 定向条目可命中**同层后置条目与全部更早层**；机器级 `$DSH_HOME/cordis.patch.yml` 是**最后持久层** | 机器级定向覆盖对全层有效 |
| 5 | 若目标 `id` 无任何声明在先，dsh 打印 `entry not found` 告警 | 要求写前校验 + `remove` 两段同步删除（不变量 #2） |
| 6 | **前置** `@deepseek-ai/dsh-mcp-client` **不是** profile 的 `dependencies` 成员、也**不在** `dsh.profile.bundles` 中；它由 dsh 安装经符号链接提供于 `$DSH_HOME/profiles/node_modules/` | 前置判定只能靠**文件系统解析探测** |
| 7 | `dsh plugin --profile web why @deepseek-ai/dsh-mcp-client` 输出为空且退出码 0（本机实测） | **官方 `why` 通道不能**作为前置判定信号 |
| 8 | 官方**不存在**列 MCP 状态或列工具的 CLI；`--dump-config` **不启动插件**、只证明**配置合成层**；`pluginInventory/list` 为 Remote-only 且不含 `serverName` 与 `mcp__*` 工具名 | 决定 D12 的验证口径与 §Testing 3.3 的可观测性边界 |
| 9 | `failOnStartupError` 官方默认 `false`（失败仅 warn）；设为 `true` 时初始连接失败会让该 fiber FAILED，进而使**整个 harness 启动中止** | 危险字段，见 D10 |

代码落点：机制 1/2/3 → `core/mcp/block.rs:1-15`；机制 4/5 → `core/mcp/block.rs:5-11`、`core/mcp/mod.rs:1-21`；机制 6/7 → `core/mcp/prereq.rs:1-12`；机制 8 → `core/mcp/mod.rs:8`、`CONTEXT.md:80`；机制 9 → `core/plugin/dump.rs:243-249`、`CONTEXT.md:79`。

### 2. Part B 的三处合规缺口（0.7.0 前现场）

- **A1/A2（P0）技能共享真源锚点错误**：真源曾锚在 `~/.agents/agent`，**既非官方扫描根、又让 `<agentsHome>/agent/skills` 落空**。现场实测 `~/.dsh` 三条链接**全部断链**、真源目录为空，而官方根有 93 个有效技能 → 技能共享**实际未生效**、用户全局指令**未被 dsh 读取**。
- **A3/P6 备份落点碰撞**：备份按 `file_name()` 落盘，而 `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` 在**同一 profile 目录下同名不同义** → 三个备份互相覆盖，回滚会写入**另一个文件的备份内容**。
- **A5 验证口径过宽**：曾把 `--dump-config` 当作运行态生效证据。

---

## Decision

| # | 决策 | 关键理由 |
| --- | --- | --- |
| **D1** | MCP 管理落点为机器级 `$DSH_HOME/cordis.patch.yml` 的**受管 MCP 区块**（marker `dsh-launcher mcp v1`），**两段式**：`- insert:` 声明段 + `- id:`/`disabled:` 定向段；块外逐字节保留 | 机器级是最后持久层，定向可命中全层；块外保留保护用户手写内容 |
| **D2** | 启停采用**定向条目**写法（而非 insert 行内 `disabled`） | 启停只触碰数行，`config` 大块在状态操作中**字节可证不被触碰** |
| **D3** | 管理标识键 = `config.serverName`（非行 id）；启动器声明的行 id 固定为 `mcp-<serverName>` | `serverName` 是面向模型的稳定身份；行 id 沿用官方 README 示例形态 |
| **D4** | `config` 子树**不透明保真**：以逐行原始文本（`config_raw`）存取；`enable`/`disable`/`remove` **绝不重渲染** `config` | `!!js` 表达式、内嵌注释、引号风格、未建模字段零损失 |
| **D5** | 状态机**三态**（`missing`/`enabled`/`disabled`），**不引入 `error` 态**；`disabled` 为 `!!js` 时按**启用**处理（不猜测表达式结果） | 官方语义：表达式求值后才决定是否激活 |
| **D6** | `origin` 二分：行 id 出现在受管区块**声明段**内 → `managed`；否则 `external`（bundle / profile patch / home 用户区 / `--patch`） | 决定 `remove` 的语义分支 |
| **D7** | `remove` 语义二分且**消息必须区分**：`managed` 真删除（声明 + 定向）；`external` **仅撤销定向覆盖**（声明仍在，恢复默认启用） | 否则用户会以为删掉了服务器，其实只是撤销了覆盖 |
| **D8** | 受管区块通用层：`core/plugin/managed.rs` 抽出「按 marker 家族读写」通用层（marker 前缀/版本/渲染器参数化），managed / shared / **mcp** 三家族共用 | 定位、`[]` 占位符、块外保留、幂等判定只实现一次 |
| **D9** | 校验**只实现官方强制的两条**：`serverName` 匹配 `^[A-Za-z0-9_-]{1,32}$`，且在同一**注册作用域**内唯一。**明确不实现** `stdio.command` 可解析性探测 | 官方无此校验，结果随 PATH/工具链漂移，归入用户自查项 |
| **D10** | 危险字段 `failOnStartupError`：`list` 对该行为**字面量 `true`** 打危险徽章；结构化新增通道**不暴露**该字段（仅 `--raw-config` 原始通道可表达） | 可中止整个 harness 启动 |
| **D11** | 三类变更（list 之外）**都不重启 dsh** | 受管 profile `web` 为 `patchReload: "live"`，官方就地热重载 |
| **D12** | 写后校验口径：**写入正确性** = 重新 `--dump-config` 后目标行的 `disabled` 与期望一致；**块外内容未变** = 块外指纹前后一致；**运行态是否真正生效无官方 CLI 可查，不在此断言** | 见 D12 精确契约 |
| **D13** | CLI：`dsh-launcher mcp list\|add\|remove\|enable\|disable [--json]`；`add` 支持全部官方字段选项；**零新增退出码**，复用 0/2/3/6/7/8 | CLI 与 GUI 共用同一 core，便于脚本化验收 |
| **D14** | 底部入口改为四按钮 `MCP \| 插件 \| 技能 \| 设置`（MCP 最左，顺序固定） | 与既有三按钮组同构 |
| **D15** | 新增备份目录 `%LOCALAPPDATA%\dsh-launcher\backups\mcp\<serverName>\<ts>\`；**不新增任何持久化状态文件** | MCP 期望态与现状均由受管区块 + 合成树表达，磁盘即事实源 |
| **D16** | 前置缺失**只影响 `add`**，不阻塞 `list`（`list` 仍返回完整列表 + 前置横幅，`add` 以 `CapabilityMissing`(6) 拒绝，UI 引导至插件页） | MCP 页不重复建设装卸入口 |
| **D17** | **（Part B）技能共享真源改锚官方 `agentsHome` 根**：技能 = `<agentsHome>/skills`（rank 500，**原生覆盖 → 不需要任何链接**）；指令 = `<agentsHome>/AGENTS.md`；`<dshHome>/CONTEXT.md` **不再建链接**（dsh 不读） | 修复前三条链接全断链、共享未生效 |
| **D18** | **（Part B）失败回滚写入收敛为唯一一处** `rollback_after_failed_official_op()`，显式登记为启动器只写 profile 文件白名单的**唯一例外**，且其后**必跟**一次官方通道 `dsh plugin … install` 收敛 | 官方无「回退到任意历史 lock 状态」的能力 |

### D1/D2/D3 精确契约（区块形态与标识）

受管区块体**只由两段构成**（不变量 #1）：

```yaml
# >>> dsh-launcher mcp v1 — 由启动器维护，请勿手工编辑 >>>
- insert:
    - id: mcp-<serverName>
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        <serverName / transport / command|url / …>
- id: mcp-<serverName>
  disabled: true|false
# <<< dsh-launcher mcp v1 <<<
```

- 声明段条目：`- id:` 位于 **4 空格**缩进；键（`id`/`name`/`config`）位于 **6 空格**；
  `name` 必须恒等于官方包名，否则解析即报错。
- 定向段条目：**列 0** 起始的 `- id:` + **2 空格**的 `disabled:`。
- 区块出现**第二个 `- insert:`** → 判契约破裂并拒绝写入。
- `config_raw` 的约定缩进：首层字段相对缩进恒为 **2 空格**，**除此之外逐字节保留**
  （缩进层级、内嵌注释、`!!js` 折叠标量、引号风格都不动）。
- 渲染顺序：声明段按 `(serverName, rowId)` 字典序、定向段按 `rowId` 字典序 → 同一期望态
  渲染出**同一字节序列**（幂等的字节基础）。
- 行 id 安全性：只允许 ASCII 字母数字与 `._-@/`（长度 ≤ 200），渲染时一律 YAML 单引号包裹，
  阻断换行/引号/`: `/`#`/流式符号逃逸。

代码落点：`core/mcp/block.rs:47-56,163-336,374-427`（解析/渲染/缩进常量）、
`core/mcp/entry.rs:19-124`（条目模型 + `validate_declare`/`validate_directive`）、
`core/plugin/managed.rs:129-146`（`validate_row_id` / `yaml_quote`）。

### D4 精确契约（不透明保真）

- `McpDeclare.config_raw` 存 `config` 子树**原始文本**（不含 `config:` 键行本身）。
- 只读视图由 `config_scalar(config_raw, key)` 取**顶层标量**：与子树首条非空行**同缩进**的
  `key: value`；遇到更深缩进（嵌套映射/多行标量）或空值即视为「该键不是顶层标量」。
- `enable`/`disable`/`remove` 只改定向段或删除整条声明，**不经过渲染器改写 `config`**。
- 验收（字节可证）：`test_repeated_disable_is_unchanged_and_config_bytes_untouched`、
  `test_config_subtree_moved_verbatim`（`config_raw` 含 `!!js` 与内嵌注释时往返逐字节一致）。

代码落点：`core/mcp/entry.rs:12-15`、`core/mcp/block.rs:340-372`、`core/plugin/dump.rs:202-274`。

### D5/D6 精确契约（状态与来源）

- `derive_state(row)`：无行 → `missing`；`disabled == Bool(true)` → `disabled`；
  `Bool(false)` / `Expression(_)` / 无该字段 → `enabled`。
- `effective_disabled(row)`：`Bool(v)` → `Some(v)`；表达式/无字段 → `None`（前端只读）。
- `derive_origin(block, row_id)`：`block.declare_by_row_id(row_id).is_some()` → `managed`，否则 `external`。
- 行级**只读标记**（不进状态机）：`expression`（拒绝覆盖）、`conflict`（`serverName` 重复）、
  `dangerous`（`failOnStartupError` 为**字面量** `true`）。

> **D5 的恒假守卫说明（2026-09-12 复核）**：`core/mcp/mod.rs:661-667` 存在一条
> `block.declare_by_row_id(&row_id).is_none() && row.row_id != row_id` 的守卫，其右项是同一
> 变量的自比较、**恒为 `false`**，故该守卫恒不可达。复核确认：该守卫想表达的区底情形
> （「受管声明与合成树既有行**两者都没有**」）在该路径上**不可能发生** —— `row` 来自
> `find_in_tree`，其 `None` 情形已由 `validate_transition` 以 `NotFound`(3) 拦下。因此这是
> **死代码**而非功能缺陷；external 行写定向覆盖本就是 D6/D7 的设计意图。处置见 ADR-0009 D4。

代码落点：`core/mcp/state.rs:100-149,173-219`、`core/plugin/dump.rs:13-44,163-170`。

### D7 精确契约（remove 二分）

| `origin` | 动作 | 结果消息要求 |
| --- | --- | --- |
| `managed` | 删除声明段条目 **+** 定向段条目 | 明确「声明与定向覆盖均已移除」 |
| `external` | **仅**删除定向段条目 | 明确「声明仍由 `<来源层>` 提供，服务器恢复默认启用」 |

- `managed` 真删除后该行不再存在 → **无期望态可校验**，仅校验块外字节（`expected = None`）。
- `external` 撤销覆盖后**不能**断言 dump 中 `disabled == false`：官方语义是「无定向覆盖即默认
  启用」，而 `--dump-config` 对未写 `disabled` 的行**不打印**该字段。故正确断言是
  `Expected::NoDirective(row_id)` —— 受管区块内不再有指向该行的定向条目。

代码落点：`core/mcp/mod.rs:688-754`（`Expected` 枚举定义于 `:291-301`）。

### D8 精确契约（通用 marker 家族层）

- `BlockFamily { begin_prefix, end_prefix, version, description }` 描述一个家族；
  `read_body` / `apply_family` 按家族读写区块体。
- 三家族 marker **互不包含**，故同一文件可安全共存（各自视为对方的「块外」）。
- `[]` 占位符处理：patch 文件必须是**单个**顶层 YAML 数组，模板里的 `[]` 在它后面追加
  `- id:` 会变成两个 YAML 文档（dsh 解析报错）→ 写区块时必须**替换**该行；
  **删除**最后一个受管条目后若文件只剩注释，必须**补回 `[]`**。
- 非 YAML 数组的顶层（如 `root: {}`）→ **拒绝写入**（fail loud，零写入）。
- **同文件写入互斥**：`file_write_lock` 按**规范化路径**区分，`Mutex<HashSet<PathBuf>> + Condvar`
  实现每路径互斥，**线程局部**记录可重入嵌套深度（放全局会让另一线程误判「自己已持锁」并使互斥失效）。
- marker 不成对/重复 → 报错且**零写入**。

代码落点：`core/plugin/managed.rs:38-52,148-244,480-650`。

### D9 精确契约（只实现官方两条校验）

- `validate_server_name`：长度 `1..=32` 且字符集 ∈ `[A-Za-z0-9_-]`（手写等价实现，不引 regex）。
- `validate_unique_server_name`：在**合成树全量**范围内判唯一（可排除自身行 id），
  冲突时报 `IllegalTransition`(2)，**错误信息必须含冲突行的来源层**（取自 dump 段标签）。
- `validate_transport_required`：`stdio` 必填 `command`；`streamable-http` 必填 `url`。
- **被否**：`stdio.command` 可解析性探测（官方无此校验，结果随 PATH 漂移）。

代码落点：`core/mcp/validate.rs:1-14,20-116`；测试 `core/mcp/validate.rs:123-253`。

### D10 精确契约（危险字段）

- `fail_on_startup_error_is_literal_true()`：仅当 `config_scalar(..., "failOnStartupError")`
  **恰为 `"true"`** 时为真 → 打 `dangerous` 标记。
- `failOnStartupError: !!js ...` 形态**无法在 dump 层判真** → **不得**打危险标记。
- 结构化通道（`McpAddSpec`）**不含**该字段；仅 `--raw-config` 原样透传。
- `disabled: true` 是该行唯一安全的隔离手段。

代码落点：`core/plugin/dump.rs:243-249`、`core/mcp/mod.rs:93-122,475`。

### D11 精确契约（不重启）

- `OpResult.restarted` 在 MCP 路径**恒为 `false`**（`commands/mcp.rs:6-7` 的模块注释明示
  「**不获取进程 `op_lock`**：MCP 管理不涉及 bundle 成员变更」）。
- `list.reload` 由 profile 的 `patchReload` 决定：`"live"` → `Live`；否则 → `RequiresRestart`。

代码落点：`core/mcp/mod.rs:43-51,149-158`；测试 `all_operations_report_restarted_false`、
`reload_mode_requires_restart_for_startup_profile`。

### D12 精确契约（写后校验，A5）

`apply_with_verification` 的三段口径：

1. **写前备份**：文件非空时先 `backup_files` 到 `backups\mcp\<serverName>\<ts>\`。
2. **写入**：`block::apply`（经通用层 `apply_family`，内部取 `file_write_lock`）。
3. **写后校验**（任一失败即**从备份回滚**并报 `VerificationFailed`(5)）：
   - **块外指纹**：`fingerprint_outside(before) == fingerprint_outside(after)`；
     规范化只做两件事——**行尾归一**（CRLF→LF）与**去掉 `after` 的前导换行**
     （不加也不减 `before` 的尾换行，否则"追加/删除区块"本身会被误报为内容变化）。
   - **目标态**：`Expected::RowDisabled(row_id, expected)` → 重新 `--dump-config` 后该行
     `effective_disabled` 必须等于期望；`Expected::NoDirective(row_id)` → 受管区块内不得再有
     指向该行的定向条目。
   - **绝不**用 `--dump-config` 充当**运行态**生效证据（A5）：它**不启动插件**，只证明配置合成层。

代码落点：`core/mcp/mod.rs:239-405`。

### D13 精确契约（CLI）

- 子命令：`list` / `add` / `remove <serverName>` / `enable <serverName>` / `disable <serverName>`，
  均支持 `--json`。
- `add` 选项（**全部为官方字段名**）：`--server-name`、`--transport`、`--id`、`--start-disabled`、
  `--command`、`--arg`（可重复）、`--env K=V`（可重复）、`--cwd`、`--url`、
  `--header K=V`（可重复）、`--tool-call-timeout-ms`、`--reconnect-enabled`、
  `--reconnect-initial-delay-ms`、`--reconnect-max-delay-ms`、`--reconnect-max-attempts`、
  `--raw-config <file>`。
- `--raw-config` 与结构化字段**互斥**（同时给出即 `IllegalTransition`）。
- `--arg` / `--env` / `--header` 的**值**允许以 `-` 开头（MCP 子进程自身选项常以 `-` 起始）。
- **零新增退出码**：复用 0 成功/空操作、2 非法转换、3 未找到、4 忙、5 校验失败、
  6 能力缺失、7 受管区块冲突、8 dsh 未安装。

代码落点：`cli.rs:1-7,353-630`（`ArgCursor` / `parse_pair` / `parse_bool` / `parse_u64` /
`build_add_spec`）；退出码表 `core/plugin/state.rs:88-102`。

### §API 2 精确契约（CLI / IPC 的对外形状）

CLI `--json` 与 Tauri IPC 使用**同一形状**（同一批 serde 结构，`rename_all = "camelCase"`）：

| 结构 | 字段 | 说明 |
| --- | --- | --- |
| `McpListResult` | `profile` · `reload` · `prereq` · `servers[]` | `list` 的结果；`reload` 为 `live` / `requires-restart` |
| `McpServerView` | `serverName` · `rowId` · `transport` · `state` · `origin` · `layer` · `summary` · `disabled` · `marks[]` | 单条 server 视图；`disabled: null` 表示 `!!js` 表达式（只读） |
| `PrereqView` | `installed` · `package` | 前置视图（`prereq` 字段） |
| `OpResult` | `status` · `restarted` · `message` · `serverName` | 写操作结果；MCP 路径 `restarted` **恒 `false`** |

**原始通道语义**（`--raw-config`，与结构化字段互斥）：真实 `transport` 与必填字段一律以
**原始 YAML 片段**为准（`--transport` 在该通道下只作无约束提示）；`config` 中的 `serverName`
**必须**与目标 `serverName` 一致，否则 `IllegalTransition`。

代码落点：`core/mcp/mod.rs:53-122`（结果与视图形状）、`core/mcp/prereq.rs:74-89`（`PrereqView`）、
`core/mcp/mod.rs:493-583`（互斥校验与 `serverName` 一致性校验）。

### §State Machine 2 精确契约（五个动作的转换）

| 动作 | 前置状态 | 结果 | 非法时 |
| --- | --- | --- | --- |
| `list` | 任意 | 只读，覆盖合成树**全量** mcp 行 | — |
| `add` | `missing` | → `enabled`（或 `disabled`，当 `--start-disabled`） | 已存在 / 不一致 → `IllegalTransition`(2)；前置缺失 → `CapabilityMissing`(6) |
| `enable` | `disabled` / `enabled` | 写/改定向行 → `enabled`（同态 → `unchanged`） | 无对象 → `NotFound`(3)；表达式行 → `IllegalTransition`(2) |
| `disable` | `enabled` / `disabled` | 写/改定向行 → `disabled`（同态 → `unchanged`） | 同上 |
| `remove` | `managed` | 删声明 + 定向 | 无对象 → `NotFound`(3) |
| `remove` | `external` | **仅删定向**（声明仍在） | 同上 |

不变量：

- **#1** 区块只由两段构成（一个 `- insert:` 声明段 + 零到多个定向条目）。
- **#2** 定向条目必须能指向某条声明（**同层或更早层**）——受管声明与合成树既有行都算。
- **#3** `enable`/`disable` 只触碰定向段，`config` 字节不变。
- **#4** 渲染顺序确定（声明段按 `(serverName, rowId)`、定向段按 `rowId` 字典序）。

代码落点：`core/mcp/mod.rs:13-21,484-768`。

### §State Machine 3 精确契约（前置与 NotFound 的边界）

- **前置缺失只影响 `add`**：`list` 仍返回完整列表（`prereq.installed = false` + 横幅），
  `add` 被拒绝且退出码 **6**（`CapabilityMissing`），UI 引导至插件页安装。
- **目标不存在 → `NotFound`(3)**，**不是**幂等空操作（与 `uninstall` 的幂等语义刻意不同）。
- **目标状态与期望一致 → `unchanged`**（幂等：零落盘、零事件），由服务层在
  `same_directive` 判定处返回，**不**算非法转换。

代码落点：`core/mcp/mod.rs:197-218,513-522,625-647`、`core/mcp/state.rs:173-219`。

### D14 精确契约（入口）

底部按钮组顺序固定 `MCP | 插件 | 技能 | 设置`，MCP 最左。

代码落点：`src/components/AppShell.tsx:47`。

### D15 精确契约（备份与状态文件）

- 备份目录：`%LOCALAPPDATA%\dsh-launcher\backups\mcp\<serverName>\<ts>\`。
- **未新增任何持久化状态文件**：MCP 期望态由受管区块表达、现状由合成树表达，**磁盘即事实源**。

代码落点：`core/mcp/mod.rs:228-237,770-778`。

### D16 精确契约（前置只影响 add）

- `probe(profile)` 判定 `@deepseek-ai/dsh-mcp-client` 的**可解析性**：解析链
  ① `$DSH_HOME/profiles/node_modules/<pkg>`（dsh 安装写入的符号链接/目录）
  ② `$DSH_HOME/profiles/<profile>/node_modules/<pkg>`；
  必须 `is_dir()` **且**能读到 `package.json`（**悬空链接不算可解析**）。
- `list` 恒可用（前置缺失时仍返回完整列表 + 横幅）；`add` 以 `CapabilityMissing`(6) 拒绝。
- UI 引导至插件页安装，**MCP 页不重复建设装卸入口**。

代码落点：`core/mcp/prereq.rs:1-89`、`core/mcp/mod.rs:197-218,513-522`。

### D17 精确契约（真源改锚官方 agentsHome 根）

- `agentsHome` 默认 `$DSH_AGENTS_HOME ?? ~/.agents`。
- 技能真源 = `<agentsHome>/skills`（官方 `user-agents` 根，**rank 500 原生覆盖 → 不需要任何链接**）。
- 指令真源 = `<agentsHome>/AGENTS.md`；`<dshHome>/AGENTS.md` **保留一条**指向它的链接。
- `<dshHome>/CONTEXT.md` **不建链**（dsh 不读该文件）。
- 资源状态新增 `native`（该资源不需要链接、真源已由官方扫描根覆盖）；
  判定链改为「**链接 → 真实文件 → 原生根**」——第 1 步必须在第 3 步之前，否则视图侧遗留的
  **断链**会被误判成 `native`，既掩盖 `~/.dsh` 的断链、又让清理动作失去判定依据。
- 新增 `dsh-launcher skill repair-links`：一次性、**可幂等重跑** —— 修复指令链接、清理
  **启动器自己创建**的断链、保留一切真实文件/目录与用户自建链接（**零删除用户内容**）。

代码落点：`core/dshhome.rs:11-19,89-133`、`core/skill/sharing.rs:11-14,184-273,565-646`。

### D18 精确契约（白名单唯一例外）

- `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` / `cordis.patch.yml` 的写入
  **收敛为唯一一处** `rollback_after_failed_official_op()`，且**仅**在官方通道执行**失败**时触发。
- 该函数结束前**必跟**一次官方通道 `dsh plugin … install` 收敛，使 `node_modules` 与还原后的
  manifest 重新对齐。
- **除该函数外**，`src-tauri/src/**` 中不存在任何写这三个 profile 文件的调用点 ——
  该不变量由静态断言守护。

代码落点：`core/plugin/mod.rs:575-604`；静态断言 `src-tauri/tests/part_b_compliance_test.rs:347-432`；
词表 `CONTEXT.md:43`。

### P6 精确契约（备份子路径隔离）

- `backup_files(files: &[(绝对路径, 备份子路径)], dest)`：**子路径参与落点**，每个目标各自独立。
- `restore_files(backup_dir, targets)`：按**备份子路径**匹配，**绝不按文件名**匹配。
- 修复前按 `file_name()` 落盘 → 同目录三个同名不同义文件互相覆盖。

代码落点：`core/profile.rs:235-290`；回归测试 `core/profile.rs:350-384`。

### R13 精确契约（官方扫描根常量集中）

官方扫描根常量**集中在 `core/dshhome.rs` 一处**，每处标注官方出处（源文件与行号），
便于随上游版本核对：

| 资源 | 路径 | 官方 rank |
| --- | --- | --- |
| 技能（官方根） | `<agentsHome>/skills` | 500 |
| 技能（dsh home 根，带 `skipSystem`） | `<dshHome>/skills` | 400 |
| 用户全局指令 | `<dshHome>/AGENTS.md` | — |
| 共享 agent 根 | `<agentsHome>`（默认 `~/.agents`） | — |

代码落点：`core/dshhome.rs:1-40,99-133`；静态断言 `src-tauri/tests/part_b_compliance_test.rs:82-159`。

---

## Architecture

```
core/mcp/
├── mod.rs      —— 服务门面（list / add / set_state / remove）+ 写后校验与回滚
├── entry.rs    —— 条目模型（McpDeclare / McpDirective / McpBlock / McpTransport）
├── block.rs    —— 受管 MCP 区块的解析与渲染（两段式语法 + config_raw 缩进归一）
├── state.rs    —— 三态 / origin / 行级标记 / 非法转换校验（纯函数，无 IO）
├── validate.rs —— 官方两条校验（serverName 形态 + 唯一性）+ transport 必填
└── prereq.rs   —— 前置可解析性探测（文件系统，不经 dsh）

commands/mcp.rs —— Tauri IPC 薄壳（spawn_blocking + emit mcp://changed）
cli.rs          —— mcp list|add|remove|enable|disable [--json]
src/lib/tauri.ts + src/components/McpPanel.tsx —— 前端封装与面板
```

### 复用的既有基础设施

| 能力 | 复用点 |
| --- | --- |
| 受管区块通用层（marker/占位符/块外保留/幂等/同文件锁） | `core/plugin/managed.rs` |
| 合成树读取（`dsh --profile web --dump-config`） | `core/profile.rs::dump_config` |
| 超时执行 | `core/command.rs::run_with_timeout` |
| 备份/还原 | `core/profile.rs::{backup_files,restore_files}` |
| 事件广播 | `core/events.rs::emit_mcp_changed` |
| 错误分级与退出码 | `core/plugin/state.rs`（**零新增数值**） |
| dsh 入口解析（含自家 shim 损坏短路） | `core/profile.rs::resolve_entry` + `core/github.rs::probe_dsh_command` |

---

## Consequences

### 正面

1. MCP server 获得与插件管理同构的**独立生命周期管理**（`serverName` 粒度），且**写操作不重启 dsh**。
2. `config` 不透明保真使 `!!js` 表达式、内嵌注释、未建模字段**零损失**，不因管理操作而改写用户配置。
3. Part B 修复了两处**真实失效**：技能共享真源（三条链接全断链 → 修复后无断链、技能计数 93）
   与备份落点碰撞（回滚写错文件）。
4. **零新增退出码、零新增持久化状态文件、零新增依赖**，白名单唯一例外可静态审计。

### 负面 / 代价

1. `serverName` 唯一性只能保证**启动器可见的 root 作用域**；其它作用域的重复启动器不可见。
2. 运行态是否真正生效**无官方 CLI 可查** → 只能断言配置层 + 人工在 dsh GUI 确认（D12/§Testing 3.3 的
   既定边界，不是实现缺陷）。
3. 机器级定向覆盖对**全层**有效，若用户手写层也声明了同 `id`，语义叠加需用户理解。
4. `<dshHome>/CONTEXT.md` 不建链是**行为变更**：该文件此前可能被误认为生效。

---

## Testing（锚点：§Testing 3.2 / 3.3 / 5）

### §Testing 3.2 —— 离线端到端（`src-tauri/tests/mcp_pipeline_test.rs`，16 个用例）

隔离方式：`DSH_HOME` / `DSH_AGENTS_HOME` / `LOCALAPPDATA` 指向临时目录；PATH 前置一个
**假 `dsh.cmd`**（PowerShell 实现，只做 patch 合成：解析 `- insert:` 声明并叠加行级 `disabled`
定向覆盖，打印与官方 `renderConfigDump` 同形的 dump，含 `# == <绝对路径>` 段标签）。
环境变量是进程级共享状态，故所有用例共用一把互斥锁**串行执行**。

覆盖的六类场景：

| # | 场景 | 用例 |
| --- | --- | --- |
| 1 | `list` 报告外部行（含来源层与标记） | `list_reports_external_user_rows_with_layer_and_marks`、`server_name_conflict_is_flagged_in_list`、`expression_disabled_row_is_readonly_and_dangerous_flag_is_reported` |
| 2 | `add` 写声明+定向且**不改动块外字节** | `add_writes_declaration_and_directive_without_touching_outside_bytes`、`add_start_disabled_declares_disabled`、`raw_config_channel_passes_official_fields_verbatim_including_js` |
| 3 | 幂等（重复 disable 不落盘、`config` 字节不变） | `repeated_disable_is_unchanged_and_config_bytes_untouched` |
| 4 | `remove` 二分语义 | `remove_managed_deletes_declaration_and_directive`、`remove_external_only_drops_directive_and_message_says_declaration_remains` |
| 5 | 并发串行化 + marker 破裂零写入 + 退出码 + 前置缺失 | `concurrent_add_and_disable_are_serialized_without_interleaving`、`broken_marker_is_conflict_with_zero_write`、`invalid_inputs_return_documented_exit_codes_with_zero_write`、`add_rejected_with_capability_missing_when_prereq_absent` |
| 6 | 备份先于每次变更 + 恒不重启 + reload 模式 | `backup_is_written_before_each_change`、`all_operations_report_restarted_false`、`reload_mode_requires_restart_for_startup_profile` |

### §Testing 3.3 —— 可观测性边界（不可脚本化的部分）

官方**不存在**列 MCP 状态或列工具的 CLI；`--dump-config` **不启动插件**。故：

- **脚本化门禁**（`scripts/e2e-adr0006.ps1`，真实 dsh + 隔离 `DSH_HOME`）：重新 dump 后目标行的
  `disabled` 与期望一致、块外字节不变、dsh stderr 的 logger 行在有界窗口内。
- **人工确认**（**不进门禁**）：dsh 会话中的工具列表是否真的出现 `mcp__<serverName>__<tool>`。
  **不为此复刻官方引擎或伪造证据**（ADR-0007 沿用同一处理方式）。

### §Testing 5 —— Part B 合规回归（`src-tauri/tests/part_b_compliance_test.rs`，5 张表）

| # | 表 | 断言要点 | 代码位置 |
| --- | --- | --- | --- |
| 1 | 锚点合规 | 锚点常量 ∈ 官方根集合；代码中不再出现 `<agentsHome>/agent` 形式的真源锚点 | `:82-159` |
| 2 | 链接修复与断链可检出 | 建链后 `readlink` + 目标存在性全通过；**人为造断链时 `skill status` 必须报 `broken`**（回归 A1 的检测能力） | `:160-265` |
| 3 | 冲突不删 | 目标位置为真实目录且有内容 → 判 `conflict` 且**零删除** | `:266-346` |
| 4 | 白名单例外可审计 | 静态断言写 `package.json`/`pnpm-lock.yaml`/`pnpm-workspace.yaml` 的调用点**仅**出现在失败回滚路径，且其后必跟官方通道 `dsh plugin … install` | `:347-432` |
| 5 | 无自造机制 | 无新增退出码数值；无新增 `%APPDATA%` 状态文件；无新增 registry/账本/加载器 | `:433-504` |
| 6 | 迁移动作（G3 清单的可重跑实现） | `repair-links` 可幂等重跑、零删除用户内容 | `:505-700` |

### 测试规模（0.7.0 发布时）

单元测试 139（+47）、集成测试 16（`mcp_pipeline_test`）、Part B 合规回归 15
（`part_b_compliance_test`）；ADR-0005 既有用例**零改动**全绿。端到端脚本
`scripts/e2e-adr0006.ps1` **61 项断言全通过**。

---

## 施工任务台账（锚点：T1–T8）

0.7.0 把本 ADR 的实施拆为编号任务；下表登记每项的落点，供后续维护者按编号追溯。
`T1`（通用层）、`T6`（端到端验收）、`T8`（布局期望值）是**被代码/脚本直接引用**的三项，故必须在此闭合。

| 任务 | 内容 | 落点 |
| --- | --- | --- |
| **T1** | 通用 marker-家族层 + **同文件写锁**（managed / shared / mcp 三家族共用） | `core/plugin/managed.rs:148-244,480-650`；测试 `core/plugin/managed.rs:871-1032` |
| T2 | 受管 MCP 区块两段式语法（解析 + 渲染 + `config_raw` 缩进归一） | `core/mcp/block.rs` |
| T3 | 条目模型与写入安全校验（行 id 字符集 + 非空 `config`） | `core/mcp/entry.rs` |
| T4 | 状态机与官方两条校验（纯函数） | `core/mcp/state.rs`、`core/mcp/validate.rs` |
| T5 | 前置可解析性探测（文件系统，不经 dsh） | `core/mcp/prereq.rs` |
| **T6** | **端到端验收脚本**（真实 dsh + 隔离 `DSH_HOME`，61 项断言） | `scripts/e2e-adr0006.ps1:1,8` |
| T7 | CLI 与 IPC 薄壳（`--json`、分级退出码、`mcp://changed` 事件） | `cli.rs:353-630`、`commands/mcp.rs` |
| **T8** | 布局期望值标注（把面板常量/键名对齐写进脚本头注释） | `scripts/verify-layout.py:1-4` |

> §Testing 3.3 第 1–6 项 = 脚本化门禁；第 7 项（dsh 会话工具列表）为人工确认，**不进门禁**
> （`scripts/e2e-adr0006.ps1:8`）。

---

## References

- `CHANGELOG.md:238-311`（0.7.0 施工记录，本文重建的第一事实源）
- `CONTEXT.md:43,68-80`（MCP 词条组、白名单唯一例外、可观测性边界）
- `docs/adr/0005-plugin-and-skill-management.md`（被 D17/D18 修订与取代之处）
- `docs/adr/0007-skill-management.md:121,194`（沿用 D12 的运行态确认纪律；退役共享前端入口）
- `docs/adr/0009-audit-remediation-plan.md`（本文重建的立项依据：P2-22 / D15；D4 恒假守卫处置）
- 源码：`src-tauri/src/core/mcp/*`、`src-tauri/src/core/plugin/{managed,dump,mod,state}.rs`、
  `src-tauri/src/core/profile.rs`、`src-tauri/src/core/dshhome.rs`、
  `src-tauri/src/core/skill/sharing.rs`、`src-tauri/src/commands/mcp.rs`、`src-tauri/src/cli.rs`
- 测试：`src-tauri/tests/mcp_pipeline_test.rs`、`src-tauri/tests/part_b_compliance_test.rs`
- 脚本：`scripts/e2e-adr0006.ps1`
