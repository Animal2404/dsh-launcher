# ADR-0007 — 技能管理：用户级技能根的启用/停用与目录治理

- **状态**：Accepted（2026-09-11 实施完成：`core/skill/{mod,frontmatter,scan,manage}.rs`、`commands/skill.rs`、`lib.rs`、`src/lib/tauri.ts`、`SkillsPanel`）
- **日期**：2026-09-11
- **范围**：`dsh-launcher` 新增技能管理（列出 / 启停 / 可恢复删除 / 定位 / 编辑器打开）；**不修改 `deepseek-harness` 任何代码**。
- **事实依据**：`deepseek-harness` 检出 `C:\Users\Administrator\AppData\Local\dsh-launcher\github-dsh\deepseek-harness`（下称 `$DSH_SRC`，版本 `0.1.5-rc.1`，tag `dsh-v0.1.5-rc.1`）。本文每条机制结论均标注源文件与行号。
- **取代**：本案推翻 **ADR-0005 D9**（当时明确拒绝在启动器内重建技能目录 UI，主张让使用者改用 dsh 官方 `skill-explorer`）。ADR-0005 的 D8「技能/指令共享」机制在本案中被**退役**。

---

## Context

### 1. 官方技能子系统的真实形状（读源码验证）

**注册表**：`ctx.skills` 是 `SkillRegistry extends Service`（`$DSH_SRC/packages/skill/skill/src/index.ts:358`，`super(ctx,'skills')` 在 `:376`）。其**全部**公开面只有五个成员：

- `registerProvider(create: (control: SkillProviderControl) => SkillProvider): () => void`（`:392`）
- `register(skill: SkillRegistration): () => void`（`:441`）
- `async list(options: SkillViewOptions = {}): Promise<SkillSummary[]>`（`:472`）
- `async snapshot(options: SkillViewOptions = {}): Promise<SkillCatalogSnapshot>`（`:483`）
- `async get(name: string, options): Promise<SkillDefinition | undefined>`（`:502`）

**没有** `invoke`、`enable`、`disable`、`unregister`、`create`、`delete`。**没有**任何技能相关的 CLI 子命令（`$DSH_SRC/apps/cli/src` 全域零 `skill` 匹配；`args.ts` 只注册 `web` 与 `plugin`）。**没有**任何技能相关的 HTTP 路由（`apps/web`、`packages/host/*` 全域零匹配；浏览器只能经 `/api` JSON-RPC 前缀桥）。**没有**任何可启停单个技能的配置键。

**唯一的技能 Remote** 是 `skills/list`（`$DSH_SRC/packages/api/session-controller/src/skill-catalog.ts:20-89`，namespace `skills` 在 `:25`）。它**只读**，返回记录**仅四个字段** —— `{name, description, whenToUse?, modelInvocable}`（`types.ts:225-234`），且被 `filter(isUserInvocable)` 过滤（`:77-84`）。它**不含路径**，因此**无法用于定位文件，也就无法驱动任何写操作**。

**结论**：官方**没有开放任何技能启停接口**。「严格依据官方」在此处的唯一可行解释是 —— 严格依据**官方定义的文件格式语义**改写 `SKILL.md`，并依赖官方热重载使其生效。这不是「调用官方接口」，而是「写入官方数据格式」。

### 2. 发现规则（一层深，刻意不递归）

`$DSH_SRC/packages/skill/skill-filesystem/README.md:36` 原文：

> A skill is either a directory bundle `<name>/SKILL.md` or a flat file `<name>.md` at the top level of a scanned root; **nested `**/SKILL.md` files are deliberately not discovered.**

同文 `:146` 再次强调：**Discovery is one level deep**。故任何「clone 一个仓库即得到技能」的设想都不成立 —— 例如 `mattpocock/skills` 的实际布局是 `skills/<分类>/<name>/SKILL.md`（**两层**），直接克隆到技能根将得到**零个**可发现技能（顶层只有 `engineering/`、`productivity/`，两者都不含 `SKILL.md`）。

### 3. 技能根与 rank（官方常量）

`$DSH_SRC/packages/skill/skill-filesystem/src/index.ts:36-41`：

```
PROJECT_DSH_RANK = 100   PROJECT_AGENTS_RANK = 200   CUSTOM_RANK = 300
USER_DSH_RANK = 400      USER_AGENTS_RANK = 500
```

`BUNDLED_SKILL_RANK = 600`（`packages/skill/skill/src/index.ts:28`）。根的组装见 `skill-filesystem/src/index.ts:241-261`：

- 项目根 `join(projectRoot, '.dsh/skills')`（:246）、`join(projectRoot, '.agents/skills')`（:247），其中 `projectRoot` 由 `findProjectRoot(resolve(cwd))` 向上找 `.git` 得出（:244、:937-947）；
- custom 根来自配置 `customSkillDirs`（:250）；
- 用户根 `join(this.dshHome,'skills')`（:253，**带 `skipSystem: true`** → 忽略 `.system`）、`join(this.agentsHome,'skills')`（:254）；
- bundled 根 `this.bundledSkillDir`（:258），来源为 `config.bundledSkillDir ?? (includeDefaultRoots ? process.env.DSH_BUNDLED_SKILL_DIR : undefined)`（:171-173）。

`agentsHome` 解析为 `config.agentsHome ?? process.env.DSH_AGENTS_HOME ?? join(homedir(), '.agents')`（:164）；`dshHome` 同理（:163、:55）。

**两条排除本项目干预的关键事实**：

1. **项目根取决于会话工作区，启动器不可知。** `roots(options.cwd)` 的 `cwd` 来自 `agent.session.header.cwd`（`tool-skill` 的 `agent/pre-step` 快照调用，见 `$DSH_SRC/packages/skill/tool-skill/src/index.ts:222`），也就是**在 dsh Web GUI 里按会话选择的工作区**，而非 launcher 进程的 CWD。启动器当前 spawn dsh 时并不设置 CWD 为用户目录（`src-tauri/src/core/process.rs:940` 与 `core/profile.rs:111` 均设为 dsh 安装目录）。
2. **custom 根 per-preset 且不出现在合成配置中。** `skill-filesystem` 的宿主行被官方**刻意禁用**：`$DSH_SRC/packages/bundle/web-app/cordis.patch.yml:398` 注释「the base host `skill-filesystem` row is disabled here (**presets own local discovery**)」，`:402-403` 为 `- id: skill-filesystem` / `disabled: true`。真正的发现由各 preset 挂载：`presets/standard/agent.cordis.yml:84-85`、`presets/ptc/agent.cordis.yml:91-92` 无配置；**`presets/cordis/agent.cordis.yml:256-259` 带 `customSkillDirs:`**。启动器的真实 dump 中该行**无 `config:` 块**（`src-tauri/tests/fixtures-real-dump.txt:172-174`），故 `--dump-config` **无法**揭示实际技能根。

### 4. frontmatter 契约（官方解析器）

字段：必填 `name` + `description`；可选 `whenToUse`、`metadata`、`disable-model-invocation`、`user-invocable`（README `:36-38`）。

`parseInvocationPolicy`（`skill-filesystem/src/index.ts:992-1002`）：

```ts
disableModelInvocation = frontmatterBoolean(data, 'disable-model-invocation')
userInvocable          = frontmatterBoolean(data, 'user-invocable')
return { modelInvocable: disableModelInvocation !== true,
         userInvocable:   userInvocable !== false }
```

即**缺省即允许**：只有显式 `disable-model-invocation: true` 才关闭模型面；只有显式 `user-invocable: false` 才关闭用户面。

**解析器的严格性（写入器的失败模式）**：

- 名字须匹配 `isSkillName = /^[a-z0-9]+(?:-[a-z0-9]+)*$/`（`packages/skill/skill/src/index.ts:21,35-37`；调用点 `skill-filesystem/src/index.ts:816`）。仅小写字母/数字，`-` 仅作分隔，**无长度上限**，大写/下划线/点一律非法。
- 布尔接受 YAML 布尔、`1`/`0`、以及大小写不敏感的 `true`/`false`/`yes`/`no`/`on`/`off`（`:1010-1029`）。
- **legacy camelCase 键被显式拒绝**：`disableModelInvocation`、`modelInvocable`、`userInvocable` 命中即抛错（`:993-995`、`:1004-1008`）。
- **任何一处失败都会让整个技能被丢弃**，只留一条 `logger.warn`（`:803-826`）。即：写入器的一个拼写错误 = 技能凭空消失。

frontmatter 的界定方式（`:909-935`）：首行经过去掉一个尾随 `\r` 后须**恰好等于** `---`；闭合符是其后第一行同样等于 `---` 的行；两者之间交给 YAML 解析。

### 5. 热重载（无需重启）

`skill-filesystem` 用 chokidar 监视各根，`awaitWriteFinish.stabilityThreshold` 默认 200ms（`:41`、`:496-499`），事件经 `queueInvalidation`（`:579-588`）调用注册表注入的 `control.invalidate`（`:166`）→ `invalidateCache()` 递增 revision、清缓存并 emit `skills/change`（`packages/skill/skill/src/index.ts:623-627`）。

模型可见目录**每个 step 重建**（非会话启动时缓存）：`tool-skill/src/index.ts:213` 的 `ctx.on('agent/pre-step', …)` 内每步 `await ctx.skills.snapshot(...)`（`:222`），并用 digest 门比对 `[name, description]`（`:228-236`、`:328-335`）；变化时追加一条**完整替换**目录的持久消息（`:279-311`）。`disable-model-invocation` 的条目被 `.filter(isModelInvocable)` 排除（`:226`），其唯一入口是 `/name` 手势（`:177-204`）。

**因此：改写 frontmatter 后，dsh 无需重启即生效；且该写入与 dsh 进程是否运行完全无关** —— 文件先落盘，watcher 在 dsh 运行中发现变更，或 dsh 下次启动时自然读取。

### 6. 本机真实状态（实测，写入器的设计基线）

`C:\Users\Administrator\.agents\skills`（官方 `user-agents`，rank 500）：

| 项 | 实测 |
| --- | --- |
| 技能数 | **93**（全为目录，顶层 `*.md` 平铺文件 **0**） |
| 行尾 | **93/93 全 CRLF**；LF-only 0、孤立 CR 0、混合 0 |
| frontmatter | **93/93** 首行 `---` 且有闭合 `---`；未闭合 **0** |
| BOM | **0/93** |
| 重复键 / 行内注释 / 空行 / Tab / 行尾空格 | 全部 **0** |
| 已带 `disable-model-invocation` | **14**（值一律裸 `true`） |
| 已带 `user-invocable` | **1**（`shadcn`，裸 `false`） |
| 布尔拼写 | 仅裸 `true`×14、裸 `false`×1；无 `yes`/`on`/`1`/引号形式 |
| 复杂结构 | **块标量 3**（`gpui-bench` `>-`、`gpui-test` `>-`、`rust-skills` `>`）、**纯多行标量 1**（`composition-patterns`）、**嵌套 `metadata:` 4** |
| 目录名 ≠ `name` | **2**（`composition-patterns`→`vercel-composition-patterns`、`react-best-practices`→`vercel-react-best-practices`） |
| 无末尾换行 | **1**（`web-artifacts-builder`） |
| 兄弟资源文件 | **70** 个技能含 `SKILL.md` 之外的兄弟文件；**30** 个含 `agents/` 子目录（如 `domain-modeling/agents/openai.yaml`）—— 均为**本地产物，上游没有** |
| 符号链接 | **0** |

`C:\Users\Administrator\.dsh\skills`（官方 `user-dsh`，rank 400）：**不存在**。

**复杂结构直接改写插入点**：`description:` 之后**绝不能**作为新键插入点 —— 若该技能用块标量（`>-`/`>`）、纯多行标量或嵌套映射，键会被插进标量/映射体内部，产出的 YAML 语义与预期完全不同。唯一稳健的插入点是**闭合 `---` 之前、第 0 列**：列 0 的行天然终结一切块标量与嵌套映射。

### 7. 问题

1. 官方**无**任何技能启停接口；但「停用技能」是真实且高频的需求（本机已有 14 个技能靠手工改 frontmatter 达成此目的，可见需求已被用户自行满足，只是手段原始）。
2. ADR-0005 D9 当初把这件事推给 dsh 官方 `skill-explorer` —— 但官方 `ui-skill`（`packages/client/ui-skill`）只提供 `/` 建议菜单与工具调用卡片，其数据源 `skills/list` **只读**，**不能启停**。第三方 `dsh-skill-explorer` 正是为补此空白而生，证明官方面确实缺失该能力。
3. 本启动器**完全没有**技能内容感知能力：全仓零 `frontmatter`、零 `disable-model-invocation` 匹配；`count_skills`（`core/skill.rs:377`）只判断路径形状，从不打开文件。
4. 既有的「技能共享」（link/config 两模式 + 迁移）在 ADR-0006 D17 把真源重锚到 `agentsHome` 后已**名存实亡** —— 技能走 rank 500 原生覆盖，**根本不需要链接**。该 UI 占据着「技能」这一词汇位，却与技能本身无关。

---

## Decision

### 决策一览

| # | 决策 | 关键理由 |
| --- | --- | --- |
| **D1** | **推翻 ADR-0005 D9**：在启动器内建设技能管理 UI | 官方 `skills/list` 只读、`ui-skill` 不能启停；需求真实存在（本机 14 个技能已手工达成） |
| **D2** | **首次引入「改写用户 markdown」的写操作类别** | 官方唯一可达路径；限定为 frontmatter 单键外科手术，配备份/复验/回滚 |
| **D3** | 单一开关 = `disable-model-invocation` | 用户确认；「停用」= 关闭模型面，**不等于**不可用 |
| **D4** | **不提供** `user-invocable` 开关 | 契约最小化；已存在者仅作只读徽章展示，绝不改写用户既有键 |
| **D5** | 只管理 rank 400 + 500 两个用户级根 | 项目根取决于会话工作区（不可知）；custom 根 per-preset 且不在合成配置中（不可见） |
| **D6** | 数据来源 = **文件系统扫描**（权威）；官方 Remote 仅 v0.9.0 可选佐证 | `skills/list` 无路径 → 结构上无法定位；且需 token 换 Cookie 的额外链路 |
| **D7** | 落盘模型：**关闭写 `<key>: true`；打开删键** | 「缺省即允许」的规范式写入 → 未动过的技能字节零变化 |
| **D8** | **逐行外科手术**，不引入 YAML 依赖 | 保留 CRLF/键序/注释/块标量；本仓 Rust 侧本就无 YAML crate |
| **D9** | 复杂结构**一律拒绝报错**，零规范化 | 真实数据 93/93 干净 → 严格拒绝在本机永不触发，却能在异常文件上避免静默损坏 |
| **D10** | 身份键 = **绝对路径**；写前重扫校验 | 目录名可与 `name` 不符（本机 2 例）、同名可跨根 → 以 name 为键必然歧义 |
| **D11** | 删除 = 移入 `<root>/.trash/`；**链接技能硬拒绝** | 可恢复；移动链接会把链接目标移走 |
| **D12** | **退役** ADR-0005 的技能 link/config 共享 UI 与前端调用 | 技能走 rank 500 原生覆盖，本不需要链接 |
| **D13** | 声明**可观测性边界** | 官方无脚本化手段列举「生效中的技能」；脚本证据止于「复读目标键」 |

### D7 精确契约（落盘模型）

| 目标状态 | 写入动作 | 结果字节 |
| --- | --- | --- |
| 启用（模型可调用） | **删除** `disable-model-invocation` 行（若存在） | 键不存在时 → **完全不写**（`Unchanged`） |
| 停用 | 写入/替换为 `disable-model-invocation: true` | 已为 `true` → **完全不写**（`Unchanged`） |

因而每个技能有三个可枚举状态：`unset`（无键，官方默认允许）、`enabled`（显式 `false`）、`disabled`（显式非 `false`），外加 `conflict`（重复键或非规范布尔 → 只读，拒绝任何操作）。

### D8/D9 精确契约（外科手术边界）

对**给定文本**执行 `set_disable_model_invocation(text, desired)`：

**拒绝并返回具名错误（绝不猜测、绝不部分写入）**，当且仅当：

1. 首行去尾随 `\r` 后**不**等于 `---`（无 frontmatter）；
2. 找不到闭合 `---` 行（未闭合）；
3. `disable-model-invocation` 键在同一 frontmatter 内**出现多于一次**（歧义）；
4. 该键的值**不是裸 `true`/`false`**（例如 `yes`、`"true"`、`1`、空值、块标量）。

**成功路径**：

- 键**不存在**且 `desired == enabled` → 返回 `Unchanged`，**调用方不写盘**；
- 键**不存在**且 `desired == disabled` → 在**闭合 `---` 行之前**插入 `disable-model-invocation: true` + 原始行尾；
- 键**存在** → **原位替换该行**（保留其行序、缩进前导、前后所有字节），行尾沿用该行原有风格；
- 其余字节（含 CRLF、BOM 缺失状态、无末尾换行、块标量、嵌套映射、非 ASCII）**逐字节不变**。

### D10 精确契约（陈旧路径防御）

前端把**绝对路径**作为身份声明回传（该值仅作身份声明，不作授权）。后端在**每次写操作前**重新扫描目标根，要求「路径存在 ∧ 该路径下的 frontmatter `name` 与声明一致 ∧ 该路径仍归属某个受管根」三者同时成立；任一不成立即返回 `400` 并提示「技能已变化，请刷新」。**不允许**任何路径回退、同名兜底或模糊匹配 —— 这防的是「面板打开后技能被重命名/移动，用户点下的开关误伤另一个同名技能」。

### D5 边界与「生效」标注

同名技能按 rank **数值大者生效**。启动器**不自创优先级**，只按官方常量 100/200/300/400/500/600 排序（虽然实际只会遇到 400/500）。被覆盖项**仍列出但默认折叠**，并提示「停用它对模型无影响」。绝不隐藏任何条目。

### D11 精确契约（删除）

- 目标移入 `<root>/.trash/<name>-<unix_ts>/`（整个技能目录递归移动，保留兄弟资源文件）；
- `.trash` 目录本身**不会**被官方发现为技能（既非顶层 `*.md`，也不含 `SKILL.md`），故不污染目录；
- 若技能目录是**符号链接**（本机实测 0 例，但契约必须成立）→ **硬拒绝**，理由：移动链接会把链接**目标**移出原位，逃出当前技能根；
- 前端对链接技能**隐藏删除按钮**，后端**独立拒绝**（不信任前端）。

### D13 可观测性边界（诚实声明）

官方**不存在**任何脚本化手段可列举「当前生效的技能」：无技能 CLI；`--dump-config` 只列插件行、不枚举技能且不暴露技能根；`skills/list` 需要 live `sessionId` 且仅返回 4 字段。因此本功能的**脚本化证据**只有两条：

1. 重新读取目标 `SKILL.md`，确认目标键的取值符合预期；
2. 官方 watcher 的存在使缓存失效**必然发生**（源码级证据）。

**技能是否真的从模型可见目录中消失，由人在 dsh GUI 中确认** —— 沿用 ADR-0006 D12 对 MCP 的同一处理方式，不为此去复刻官方引擎或伪造证据。

---

## Architecture

### 1. 分层与职责

```
core/skill/
├── mod.rs          —— 模块出口与共享类型（SkillEntry / SkillRoot / SkillState …）
├── frontmatter.rs  —— 纯函数逐行外科手术（零 IO、零依赖、全可单测）
├── scan.rs         —— 受管根枚举 + frontmatter 只读解析 + rank 生效标注
└── manage.rs       —— 写操作编排：身份校验 → 备份 → 写入 → 复验 → 回滚
```

`core/skill.rs`（原 1079 行共享模块）按 D12 **退役**；其仍被 `cli.rs` 使用的部分（`status`/`apply`/`migrate`/`repair_links`）在 v0.8.0 **保留为 CLI 后端**，前端调用点移除。

**职责边界**：`frontmatter.rs` 是唯一理解 YAML frontmatter 文本形状的模块，且是**纯函数**（输入 `&str`，输出 `Result<String, WriteError>`）—— 因此 D8/D9 的全部边界用例都可用单元测试穷举，无需文件系统。`manage.rs` 是唯一执行写入的模块，且必须经 `plugin::managed::file_write_lock` 取锁（与 `cordis.patch.yml` 的写入共用同一把按路径重入锁，避免同进程内自相干扰）。

### 2. 复用既有基础设施（不重造）

| 需求 | 复用 |
| --- | --- |
| 路径解析 | `core/dshhome.rs`：`agents_skills_dir()`（rank 500）、`dsh_home_skills_dir()`（rank 400）—— 已存在且与官方解析逐字一致（读 `DSH_HOME`/`DSH_AGENTS_HOME` 环境变量） |
| 原子写 | `core/plugin/managed.rs::write_atomic`（`create_dir_all` + tmp + rename）。**注意**：其临时名硬编码为 `.yml.tmp`（`managed.rs:425`），对 `SKILL.md` 会产生 `SKILL.yml.tmp` —— 功能可用但语义错位，故本案在 `manage.rs` 内实现带正确后缀的原子写，**不改动** `managed.rs`（避免波及插件/MCP 两条既有链路） |
| 文件写锁 | `core/plugin/managed.rs::file_write_lock`（按规范化路径、线程内可重入） |
| 读取 | `core/plugin/managed.rs::read_file`（`Ok(None)` 表示不存在） |
| 事件 | `core/events.rs::emit_skill_changed`（`skill://changed`）—— 沿用，前端据此刷新 |
| 原子写参照 | `core/plugin/registry.rs`（临时文件 + 删旧 + rename）与其「缺文件/损坏 → 返回默认」的容错姿态 |

### 3. 进程与并发模型

- 所有命令经 `tauri::async_runtime::spawn_blocking` 卸载（沿用既有 39 个命令的一致模式）。
- 每个写操作在**单次锁持有期内**完成「读取 → 变换 → 备份 → 写入 → 复验」，杜绝读改写竞态。
- 写操作**不依赖** dsh 进程状态：不 spawn dsh、不探测端口、不等待就绪。
- 扫描是多目录 `read_dir`，为 O(技能数) 次小 IO；93 个技能的实测规模下无需并发化，保持单线程顺序以简化错误归因。

---

## State Machine

### 1. 技能启用状态

| 状态 | 判定 | 可否操作 |
| --- | --- | --- |
| `unset` | frontmatter 无 `disable-model-invocation` 键 | 可（官方默认 = 模型可调用） |
| `enabled` | 键存在且可解析为 `false` | 可 |
| `disabled` | 键存在且可解析为 `true` | 可 |
| `conflict` | 键出现多于一次，或值非裸布尔 | **拒绝**（只读展示；返回具名原因） |
| `unreadable` | 无 frontmatter / 未闭合 / 读失败 / `name` 或 `description` 缺失 | **拒绝**（只读展示） |

### 2. 转换表（`set_enabled(path, desired)`）

| 起始 | `desired = 停用` | `desired = 启用` |
| --- | --- | --- |
| `unset` | 插入键 → `disabled` | **Unchanged**（不写盘） |
| `enabled` | 替换值为 `true` → `disabled` | **Unchanged**（不写盘） |
| `disabled` | **Unchanged**（不写盘） | 删除键行 → `unset` |
| `conflict` | **拒绝** `ConflictKey` | **拒绝** `ConflictKey` |
| `unreadable` | **拒绝** `NoFrontmatter` / `Unterminated` / … | 同左 |

### 3. 不变量

1. **幂等**：对已是目标状态的技能重复操作，第二次必然 `Unchanged` 且不写盘（对齐插件的 `BlockOutcome::Unchanged` 纪律）。
2. **可逆**：`停用 → 启用` 后文件**逐字节等于**操作前（由 D8 的字节保留契约保证）。
3. **原子**：任一步失败（备份失败、写入失败、复验失败）都不得留下半写状态；复验失败必须回滚到备份。
4. **最小惊讶**：只增删/替换目标键那一行；`user-invocable`、`metadata`、`description`、块标量、缩进、行尾**永不触碰**。
5. **前置失败即不写**：身份校验（D10）不通过时，**在读取目标文件之前**就拒绝。

---

## Data Flow

### 1. 列出（只读，可随时调用，与 dsh 运行状态无关）

```
skill_list()
  ├─ 枚举受管根：dsh_home_skills_dir()(400) ─┐
  │                agents_skills_dir()(500) ─┤ 存在者才扫；rank 400 跳过 .system
  ├─ 每根 read_dir：
  │    ├─ <name>/SKILL.md  → 目录包技能
  │    ├─ <name>.md        → 平铺技能
  │    └─ 其余（含 .trash/、非 .md、无 SKILL.md 的目录）→ 忽略
  ├─ 每个候选：读取文本 → 解析 frontmatter（只读路径，宽容）
  │    ├─ 成功 → { name, description, whenToUse?, state, path, root, rank }
  │    └─ 失败 → { state: unreadable, 原因 }
  └─ 按 rank 降序 + 名称升序排列；标注同名覆盖关系
       └─ 生效者 / 被覆盖者（后者附覆盖它的技能名与 rank）
```

**只读解析与写入解析的严格度不同**：列出必须**宽容**（一个坏技能不应让整个面板报错），只标注状态；写入必须**严格**（拒绝一切歧义）。两者共用同一套 frontmatter 定位逻辑，但对外契约不同。

### 2. 启停（写）

```
skill_set_enabled(path, name, enabled)
  ├─ 1. 身份校验（D10）：重扫受管根，要求
  │      路径 ∈ 受管根 ∧ 磁盘 frontmatter name == 声明的 name
  │      └─ 不符 → 400「技能已变化，请刷新」（此前不读目标文件）
  ├─ 2. 取 file_write_lock(path)
  ├─ 3. read_file(path) → 文本
  ├─ 4. frontmatter::set_disable_model_invocation(text, !enabled)
  │      ├─ Err(WriteError) → 400 + 具名原因（不写盘）
  │      └─ Unchanged        → 返回 {changed:false}（不写盘、不发事件）
  ├─ 5. 备份：<backup_root>/skills/<name>/<ts>/SKILL.md（保留原字节）
  ├─ 6. write_atomic(path, new_text)
  ├─ 7. 复验：重新读取 → frontmatter 可解析 ∧ name/description 仍在
  │           ∧ 目标键可解析为预期值
  │      └─ 失败 → 用备份回滚 + 返回错误
  └─ 8. emit_skill_changed
```

### 3. 删除（写）

同上的身份校验与锁；额外**在步骤 1 拒绝符号链接技能**；随后 `fs::rename(<skill_dir>, <root>/.trash/<name>-<ts>)`（同根内 rename 是原子的）。不触碰技能目录内任何文件内容。

### 4. 运行时链路（端到端，热生效）

```
launcher 写 SKILL.md
   └─ dsh 运行中：chokidar(~200ms 稳定) → queueInvalidation → invalidate()
        └─ revision++ / 清收集缓存 / emit skills/change
             └─ 下一个 agent step：ctx.skills.snapshot()
                  └─ digest 变化 → 追加「完整替换」的 <available_skills> 消息
   └─ dsh 未运行：无动作；下次启动读取最新文件 → 天然生效
```

**两条路径都无需重启 dsh，也都无需 launcher 与 dsh 通信** —— 这正是 D6 选择文件扫描的额外收益，也是「无论 dsh 运行与否都能控制技能」得以成立的结构性原因。

---

## API

### 1. 官方 API 调用边界（唯一允许的 dsh 交互）

**v0.8.0：零 dsh 交互。** 不 spawn dsh、不读 `--dump-config`、不调任何 Remote。全部依据是本 ADR 第 1–6 节所引的**官方源码事实**（常量、路径、格式契约），这些事实是**稳定契约**而非运行时查询。

**v0.9.0（可选佐证，见后续 ADR）**：`GET /?token=…` 换 Cookie → `POST /api/session/list`（wire 参数名为 `_request`）→ `POST /api/skills/list`（wire 参数名为 `request`）。仅在 dsh running 时尝试，失败**绝不影响**开关本身。

### 2. Rust 公共面（`core/skill/`）

```rust
// frontmatter.rs —— 纯函数，零 IO
pub enum WriteError { NoFrontmatter, Unterminated, DuplicateKey, NonPlainBoolean, MissingName }
pub enum Toggle { Unchanged, Rewritten(String) }
pub fn disable_state(text: &str) -> DisableState;              // Unset/Enabled/Disabled/Conflict/Unreadable
pub fn set_disable_model_invocation(text: &str, disable: bool) -> Result<Toggle, WriteError>;

// scan.rs
pub struct SkillEntry { path, name, description, when_to_use, state, root, rank, overridden_by }
pub fn list() -> Result<Vec<SkillEntry>, SkillError>;

// manage.rs
pub struct ToggleReport { changed: bool, message: String }
pub struct DeleteReport { trashed_to: String, message: String }
pub fn set_enabled(declared_path: &Path, declared_name: &str, enabled: bool, logger) -> Result<ToggleReport, SkillError>;
pub fn delete(declared_path: &Path, declared_name: &str, logger) -> Result<DeleteReport, SkillError>;
```

### 3. Tauri IPC（`lib.rs` 追加）

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `skill_list` | — | `Vec<SkillEntry>`（camelCase） |
| `skill_set_enabled` | `path: String, name: String, enabled: bool` | `ToggleReport` |
| `skill_delete` | `path: String, name: String` | `DeleteReport` |

三个命令均 `spawn_blocking`；两个写命令成功后 `emit_skill_changed`。

**退役**：`skill_status` / `skill_apply` / `skill_migrate` 三个 IPC 命令与其前端封装移除（Rust 后端 `core/skill.rs` 与 CLI 分支按 D12 保留）。

### 4. CLI（不变）

`dsh-launcher-cli skill status|apply|migrate|repair-links` 保留（共享机制的应急出口）。v0.9.0 追加 `skill list|enable|disable`。

### 5. UI

`SkillsPanel` 重写为**单一管理视图**（不再分 Tab）：

```
技能管理
  受管根  <DSH_HOME>/skills（400，存在/不存在）
          <agentsHome>/skills（500）            共 N 个技能
  ─────────────────────────────────────────────
  [搜索框]                        [刷新] [定位全部?]
  ─────────────────────────────────────────────
  ▾ code-review            500  生效            [开关] [定位] [打开]
    Review the changes since a fixed point…
  ▾ grill-me               500  生效            [开关] [定位] [打开]
  ▸ ask-matt               500  被 grill-me 覆盖（折叠，停用它对模型无影响）
  ▸ broken-skill           500  无法解析：缺少 description（只读）
  已停用 N 个 · 冲突 N 个 · 无法解析 N 个
```

- 开关：`Switch`（shadcn 风格，复用既有 `src/components/ui/switch.tsx`）。
- 定位：`revealItemInDir`（**已被 `opener:default` 允许**，零新增权限）。
- 打开：走 Rust 命令（v0.9.0 起带可配置编辑器；v0.8.0 用系统默认）。
- 删除：二次确认；链接技能不显示按钮。
- 冲突/无法解析项：徽章 + 具名原因，**开关禁用**。

---

## Directory Layout

### 1. 仓库内改动

```
src-tauri/src/core/
  skill.rs                     → 删除（内容迁往 core/skill/；CLI 用到的部分由 mod.rs 再导出）
  skill/mod.rs                 + 新增
  skill/frontmatter.rs         + 新增（纯函数 + 单测）
  skill/scan.rs                + 新增（含单测：临时目录夹具）
  skill/manage.rs              + 新增（身份校验/备份/复验/回滚 + 单测）
  dshhome.rs                   ~ 增 agents_home_context_md()
src-tauri/src/commands/skill.rs ~ 三个新命令；移除三个旧命令
src-tauri/src/lib.rs           ~ invoke_handler 增删
src-tauri/src/cli.rs           ~ 保留 run_skill（不变）
src/lib/tauri.ts               ~ 新类型 + 新封装；修 ResourceState/needsLink/agentsSkillsRoot 漂移
src/components/SkillsPanel.tsx ~ 重写为管理面板
src/components/App.tsx         ~ 面板标题/说明文案更新
docs/adr/0001..0005            + 自 git 历史取回重建
docs/adr/0007-skill-management.md  + 本文件
CONTEXT.md                     ~ 「技能管理（ADR-0007）」词汇节 + 「技能共享」标记退役
```

### 2. 运行时文件系统

```
<DSH_HOME>/skills/                     rank 400（本机不存在则跳过）
<agentsHome>/skills/                   rank 500
<agentsHome>/skills/<name>/SKILL.md    写入目标
<agentsHome>/skills/.trash/<name>-<ts>/ 可恢复删除
%LOCALAPPDATA%\dsh-launcher\backups\skills\<name>\<ts>\SKILL.md
```

**关于 `skills.json`**：该文件名**已被占用** —— 旧共享模块用它保存 `preferred_mode` / `last_applied`（`core/skill.rs:275-308`）。v0.9.0 的技能来源注册表因此定名 **`skill-sources.json`**，避免语义撞车。旧 `skills.json` 在 D12 退役后**保留不读不写，不删除**（用户数据不主动清理）。

---

## Migration

### 1. 前端调用点退役（v0.8.0）

`src/lib/tauri.ts` 移除 `skillStatus` / `skillApply` / `skillMigrate` 与其类型（`SkillStatus` / `SkillApplyReport` / `MigrateReport` / `ResourceState` / `ResourceStatus`）；`SkillsPanel.tsx` 整体重写。**Rust 侧后端保留**，故 CLI 与应急路径不断。

### 2. 无数据迁移

本功能**不迁移任何既有数据**：不改写任何技能文件（只读扫描），不动 `skills.json`，不动 `cordis.patch.yml`。首次使用时 93 个技能的 `disable-model-invocation` 现状被如实读出并展示 —— 用户已有的手工停用（14 个）**天然被识别**，无需迁移。

### 3. 兼容性

- 不新增 Rust 依赖（无 YAML crate）；不新增 npm 依赖；不新增 Tauri capability 权限（定位按钮用已授予的 `opener:default`；打开走 Rust 命令，不受 ACL 约束）。
- 旧 `config.json` 不受影响（本版不新增 `AppConfig` 字段；可配置编辑器是 v0.9.0，届时依赖 `AppConfig` 现有的**结构体级 `#[serde(default)]`**，无需迁移代码）。
- 面板入口不变（底部「技能」按钮仍在，弹窗宽度沿用 v0.7.1 的 `PANEL_DIALOG_WIDTH`）。

---

## Consequences

### 正面

1. 补齐了官方缺失、且用户已在用手工手段满足的刚需（本机 14 例为证）。
2. **零新依赖、零新权限、零 dsh 交互** —— 风险面极小，且完全离线可用。
3. 字节级可逆 + 备份 + 复验 + 回滚，使「改写用户文件」这件事有明确的失败边界。
4. 纯函数写入器使全部边界用例可单测穷举，无需真实文件系统。
5. 明确写下可观测性边界，避免为「自证生效」而伪造证据或复刻官方引擎。

### 负面 / 代价

1. **技能内容成为本产品的写目标** —— 这是身份级扩张，必须长期承担「不损坏用户内容」的责任。缓解：拒绝歧义 + 备份 + 复验 + 回滚 + 幂等空操作。
2. 只覆盖 400/500 两个根，**项目级技能不可管理**（结构性限制，非实现偷懒）—— 需在 UI 上如实说明，否则用户会困惑「为什么看不到项目技能」。
3. 无脚本化证据证明「技能已从模型目录消失」，验收依赖人工确认。
4. `.trash` 会累积残留（无自动清理策略），需在 UI 显示占用并允许用户手动清理。
5. `core/skill.rs` 的退役留下一个「CLI 有后端、UI 无入口」的过渡期（共享机制），直到 v0.9.0 的编辑按钮落地。

---

## References

- `$DSH_SRC/packages/skill/skill/src/index.ts:21,28,35-37,358,376,392,441,472,483,502,623-627`
- `$DSH_SRC/packages/skill/skill-filesystem/src/index.ts:36-41,163-166,171-173,241-261,496-499,579-588,793-835,909-935,992-1029`
- `$DSH_SRC/packages/skill/skill-filesystem/README.md:36,38,146`
- `$DSH_SRC/packages/skill/tool-skill/src/index.ts:177-204,213-251,279-311,328-335`
- `$DSH_SRC/packages/api/session-controller/src/skill-catalog.ts:20-89`、`types.ts:220-239`
- `$DSH_SRC/packages/bundle/web-app/cordis.patch.yml:393-405`、`packages/preset/agent-presets/presets/{standard,ptc,cordis}/agent.cordis.yml`
- `$DSH_SRC/apps/cli/src/args.ts`（无 skill 子命令）
- 本项目：`src-tauri/src/core/dshhome.rs`、`core/plugin/managed.rs`、`core/events.rs`、`docs/adr/0005-plugin-and-skill-management.md`、`docs/adr/0006`（代码注释形态）
