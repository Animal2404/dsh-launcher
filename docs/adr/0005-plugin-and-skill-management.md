# ADR-0005 — 插件与技能管理：profile bundle 生命周期 + 共享技能资源

- **状态**：Accepted（2026-09-10 实施完成：`core/dshhome.rs`、`core/profile.rs`、`core/plugin/*`、`core/skill.rs`、`cli.rs`、`commands/plugin.rs`、`commands/skill.rs`、`PluginsPanel`、`SkillsPanel`、底部三按钮入口）
- **日期**：2026-09-10
- **范围**：`dsh-launcher` 新增插件管理、技能资源统一、底部入口按钮；**不修改 `deepseek-harness` 任何代码**。
- **事实依据**：`deepseek-harness` 检出 `C:\Users\Administrator\AppData\Local\dsh-launcher\github-dsh\deepseek-harness`（下称 `$DSH_SRC`）；`dsh-launcher` 检出 `Y:\dsh-launcher`。本 ADR 的每条机制结论都标注了源文件。

---

## Context

### 1. dsh 侧现有机制（架构依据，均已读源码验证）

**启动链**：`$DSH_SRC/apps/cli/src/bin.ts` 只解析三种模式 —— `profile`（`--profile <name>`）、`plugin`（`dsh plugin`）、`dump-config`（`--dump-config` / `--dump-default-config`）。`web` 是 `--profile web` 的硬编码别名（`apps/cli/src/args.ts:156`）。启动器当前 spawn 的正是 `... bin.ts web --port <p> --no-open`（`src-tauri/src/core/process.rs:972-1004`，GitHub 通道）或 `dsh web --port <p> --no-open`（npm 通道，`process.rs:206-207`）。

**profile**：一个目录 `$DSH_HOME/profiles/<name>`，内含
- `package.json`：profile manifest，`dependencies` 为 pnpm 管理的树外插件，`dsh.profile.bundles` 为**有序 bundle 层列表**，`dsh.profile.patchReload` 为 `live|startup`；
- `cordis.patch.yml`：**用户自己的 patch 层**，在全部 bundle 层之后应用；
- `cordis.yml`：空根（每次加载都被重写为空数组，仅作为 Loader 的 include 锚点）。

定义见 `$DSH_SRC/packages/boot/app-boot/src/profile.ts:79-107,170-190`；`resolveProfileDir` 拒绝含路径分隔符的 profile 名（`profile.ts:100-107`）。已装载 profile 的结构为 `Profile{name,dir,layers[],patchPath,patches,patchReload}`（`profile.ts:78-92`）。

**bundle 与层序**：bundle 是声明了 `"dsh": {"bundle": {"patch": "./cordis.patch.yml"}}` 的 npm 包（`apps/cli/src/plugin.ts:36-45`）。有效配置由 patch 层按序叠加在**空根**上：

1. `dsh.profile.bundles` 顺序的每个 bundle 层；
2. profile 自己的 `cordis.patch.yml`；
3. `$DSH_HOME/cordis.patch.yml`（**机器级**，优先级高于 profile 层）；
4. 每个 `--patch <file>` 覆盖层（argv 顺序）。

来源：`$DSH_SRC/apps/cli/src/profile-boot.ts:137-144,256-261` 与 `apps/cli/reference/README.md:9`。

**patch 语义**：`PatchOptions` 支持 `id / insert / name / config / group / disabled / inject / intercept / isolate`（`$DSH_SRC/vendor/include/src/index.ts:144-156`）。`applyEntryPatches` 的关键语义（同文件 `:58-128`）：
- 按 `id` 定位行，**整体替换该行的 `config`**（不做深合并）；
- `disabled?: boolean | null` 是可覆盖字段，因此 `- id: x` + `disabled: true/false` 可**逐行启停**；
- `insert` 插入的新行会被立即建索引，同一层后面的 patch 可以继续定位它；
- 目标行不存在时**只 warn 并跳过**，不失败（`README.md:42` 明确"一份共享 overlay 不必匹配每棵树"）。

**启停 vs 装卸的关键边界**（`$DSH_SRC/apps/cli/reference/README.md:58`，原文）：
> "The successful pnpm operation changes the Profile manifest and Bundle list on disk; a running Profile keeps the Bundle set from its current start. **Restart that Profile after adding, removing, or updating a Bundle.** This startup boundary applies to Bundle membership, while **ordinary edits to the Profile or home `cordis.patch.yml` take effect through hot reload.**"

即：**bundle 成员变更 = 需要重启**；**patch 层编辑 = 热重载**。热重载只监听两个用户层文件（profile 与 home 的 `cordis.patch.yml`），`--patch` 覆盖层是启动时一次性解析的（`profile-boot.ts:283-313` 只 `watchUserPatches` 这两个文件；`composeLive` 复用启动时解析的 overlays）。`web` 模板 `patchReload: 'live'`（`profile.ts:115-118`）。

**插件安装/卸载**：`dsh plugin --profile <name> <args...>` 是**唯一的官方依赖变更通道**（`apps/cli/src/plugin.ts:120-163`）：
1. profile 不存在则用模板初始化（`initProfile`）；
2. 把 `<args...>` 原样转发给 profile 目录下的 `pnpm`（`add`/`remove`/`update`/`why` 全部可用）；
3. 成功后按**已安装状态**对账 `dsh.profile.bundles`：能解析到且声明 `dsh.bundle` 的依赖加入层列表；被移除或不再声明 bundle 的依赖退出层列表；模板内置 bundle 不是依赖，永不动（`plugin.ts:59-91`）。

注意两个实现细节：
- 相对路径 spec 会以**调用者 cwd** 锚定（`plugin.ts:104-112`），启动器若用 cwd=安装目录调用，必须自己把本地路径转绝对路径；
- 对账依据是"已安装状态"而非依赖 diff，所以 `update` 能激活一个在新版本里才获得 `dsh.bundle` 声明的包。

**运行时行发现**：`dsh --profile <name> --dump-config` **不启动任何插件**，只做 patch 组合并打印，每个来源段前有 `# == <bundle>`（或 `# == <bundle>, patched by <overlay>`）注释，行体为 YAML（`apps/cli/src/dump-config.ts:30-52`、`app-boot/src/index.ts:408-502`）。本机实测输出（`--profile web`，退出码 0）：

```
# == @deepseek-ai/dsh-base
- id: timer
  name: '@deepseek-ai/cordis-plugin-timer'
- id: hmr
  name: '@deepseek-ai/cordis-plugin-hmr'
  disabled: true
  config:
    root:
      - .
...
# == dsh-find-plugin
- id: find-dsh-plugin
  name: dsh-find-plugin
# == @michengai/dsh-archive-manager
- id: workspace-archive-manager
  name: '@michengai/dsh-archive-manager/workspace'
- id: session-projection-cache-archive-manager
  name: '@michengai/dsh-archive-manager/projcache'
  config: {...}
- id: ui-workspace-archive-manager
  name: '@michengai/dsh-archive-manager'
```

实测确认：第三方 bundle 的段标签**就是 `dsh.profile.bundles` 里的包名**；一个 bundle 可贡献 1..N 个带稳定 `id` 的行；存在 `disabled: !!js process.platform === 'win32'` 这类**表达式控制的 disabled**。

**技能**：`ctx.skills` 是分层注册表（`$DSH_SRC/docs/subsystems/skills.md:9-15`），本地提供方 `dsh-skill-filesystem` 按 rank 扫描：

| rank | source | 根目录 |
|---|---|---|
| 100 | `project-dsh` | `<projectRoot>/.dsh/skills` |
| 200 | `project-agents` | `<projectRoot>/.agents/skills` |
| 300 | `custom` | `Config.customSkillDirs` |
| 400 | `user-dsh` | `<dshHome>/skills` |
| 500 | `user-agents` | `<agentsHome>/skills` |
| 600 | `bundled` | `Config.bundledSkillDir` |

来源 `docs/subsystems/skills.md:64-77` 与 `$DSH_SRC/packages/skill/skill-filesystem/src/index.ts:36-44,163-165,240-256`。其中
`dshHome` 默认 `$DSH_HOME` 或 `~/.dsh`；`agentsHome` 默认 `$DSH_AGENTS_HOME` 或 `~/.agents`，技能根是 **`<agentsHome>/skills`**（`index.ts:56-57,164`）。识别规则：kebab-case 目录 bundle `<name>/SKILL.md` 或扁平文件 `<name>.md`，**不支持递归 `**/SKILL.md`**（`skills.md:85`）。

**指令文件**：`dsh-agent-instructions` 的**用户全局文件固定为 `<dshHome>/AGENTS.md`**（`$DSH_SRC/packages/context/agent-instructions/src/files.ts:280-296`），`dshHome` 是行配置项（`config.ts:19-20,40`），`maxBytes` 为**必填**（`config.ts:42`），候选名为 `AGENTS.md`/`CLAUDE.md`（`config.ts:12`）。**`CONTEXT.md` 不被 dsh 任何代码读取**（全仓 `packages/**` 无引用），它是 agent/技能侧的约定资源。

**Cordis 生命周期**：`PENDING → LOADING → ACTIVE`，失败入 `FAILED`，`ACTIVE → UNLOADING → DISPOSED`；注册即 effect，卸载自动回滚（`$DSH_SRC/docs/user/develop/framework/index.md:7-24,40-63`）。启停一个插件在 dsh 侧等价于"该行的 `disabled` 翻转 → 该 Fiber 卸载/装载"，无需改代码。

### 2. dsh-launcher 侧现状

- 后端 `Rust/Tauri2`，模块划分见 `docs/DESIGN.md`；IPC 命令在 `src-tauri/src/lib.rs:273-300` 注册。
- 配置持久化 `%APPDATA%\dsh-launcher\config.json`，**临时文件 + rename 原子写**、GitHub Token 用 DPAPI 加密（`src-tauri/src/core/config.rs:115-144,161-229`）。
- 子进程统一 `CREATE_NO_WINDOW`；`.cmd` 用 `cmd /C` 包装并注入用户级 Node PATH（`core/command.rs:25-60`）；带超时执行会强杀进程树（`command.rs:94-162`）；流式执行 + 进度回调（`core/stream.rs:41,206-220`）。
- GitHub 通道已有 `git ls-remote` 解析与镜像/Token 注入（`core/github.rs:31-81,113-146`），安装目录 `%LOCALAPPDATA%\dsh-launcher\github-dsh\deepseek-harness`（`github.rs:83-97`）。
- **已有的、与插件相关的既有行为**：启动后 8s 内端口未监听且进程退出时，`fix_incompatible_plugins()` 会硬编码地 `pnpm dsh plugin --profile web uninstall dshmarket`（`core/process.rs:513-638`）。这是"启动崩溃→修插件"的雏形，但目标写死、动作不可逆（卸载而非禁用）。
- 生命周期操作有 `op_lock` 互斥（`process.rs:140-144`）。
- 前端：`App.tsx` 用受控 `Dialog` 承载 `SettingsPanel`（`src/App.tsx:30-45`）；侧栏底部目前是**单个** `Button`（`src/components/AppShell.tsx:186-198`）；IPC 全部走 `src/lib/tauri.ts` 的类型化封装。

### 3. 本机真实状态（迁移基线，实测）

- `%USERPROFILE%\.dsh\skills` 是 **Junction** → `C:\Users\Administrator\.agents\agent\skills`；
- `%USERPROFILE%\.dsh\AGENTS.md`、`CONTEXT.md` 是**文件符号链接** → `C:\Users\Administrator\.agents\agent\{AGENTS.md,CONTEXT.md}`；
- `%USERPROFILE%\.agents\agent\skills` 下有 28 个技能目录；
- `%USERPROFILE%\.dsh\profiles\web\package.json` 已装 7 个树外 bundle：`dsh-find-plugin`、`dsh-cost-meter`、`dshmarket`、`@linxin666/dsh-client-ui-{task-board,git-graph,skill-explorer}`、`@michengai/dsh-archive-manager`，`patchReload: live`；
- 同目录 `cordis.patch.yml` 仍是模板空数组 `[]`，`$DSH_HOME/cordis.patch.yml` 不存在；
- 同目录存在 `package.json.bak.20260829-035650` / `pnpm-lock.yaml.bak.20260829-035651`，说明**该生态已有"操作前备份"的约定**；
- `.dsh-market/state.json` 含 `{"disabled":[...]}` —— 社区插件自建了启停账本，属于可复用的先例，也是**重复维护风险**的来源。

### 4. 问题

1. 启动器没有任何按 ID 管理插件生命周期的能力；唯一动作是写死的 `dshmarket` 卸载。
2. 插件状态散落在 profile `package.json`、`dsh.profile.bundles`、`cordis.patch.yml`、社区插件自己的 `.dsh-market/state.json` 四处，无单一事实源、无幂等语义、无非法转换拒绝。
3. 上游插件更新（npm 版本、git commit）需要人工逐个 `dsh plugin ... add/update`，且变更后必须重启 dsh。
4. `~/.dsh` 与 `~/.agents/agent` 的技能/指令共享目前靠**手工创建的 junction/符号链接**维持，无检测、无修复、无迁移、无权限降级路径；`CONTEXT.md` 的消费方不是 dsh 而是 agent。
5. 底部只有"设置"一个入口，插件/技能没有入口。

---

## Decision

### 决策一览

| # | 决策 | 依据 / 被否方案 |
|---|---|---|
| **D1** | 插件 = **profile bundle**（`dsh.profile.bundles` 中的包）。启动器只管理启动器自己启动的 profile（常量 `MANAGED_PROFILE = "web"`），不改 profile 名、不改 `cordis.yml`。 | `dsh web` 的官方别名即 profile `web`；`resolveProfileDir` 拒绝非法名。 |
| **D2** | **装卸**走官方唯一通道 `dsh plugin --profile web add/remove`；**启停**不走层列表，而走 **patch 层的 `id` + `disabled` 覆盖**。 | `plugin.ts:59-91` 的按状态对账会在任何一次 `dsh plugin` 后把"仅从 bundles 摘掉"的依赖**重新加回**，故"从 bundles 移除"不是稳定的禁用手段；而 `vendor/include/src/index.ts:121-124` 的 `disabled` 覆盖是官方语义。被否：直接改 `dsh.profile.bundles`（被对账回滚）、直接改 `cordis.yml`（每次加载被重写为空）。 |
| **D3** | 启停写 **profile 自己的 `cordis.patch.yml` 中的受管区块**（marker 包裹、EOF 追加、原子写、块外字节永不改动），而不是 `--patch` 覆盖层。 | 只有 profile / home 的 `cordis.patch.yml` 会被 `watchUserPatches` 热重载（`profile-boot.ts:283-313`）；`--patch` 是启动时一次性的。profile 级而非 home 级：插件是 profile 私有的，避免其它 profile 出现 `patch: entry X not found` 噪音。 |
| **D4** | 插件状态机为 `uninstalled / plain / enabled / disabled`（`plain` = 已安装但不声明 `dsh.bundle` 的普通依赖，不可启停）。行级状态额外有 `expression`（`disabled` 由 `!!js` 决定，拒绝覆盖）。 | `plugin.ts:70-75` 明确 bundle-less 依赖只装不激活；实测 dump 存在 `disabled: !!js ...`（`bash-sandbox`）。 |
| **D5** | 幂等由**启动器侧的"目标态 == 现状则不落盘"**保证：`enable/disable/uninstall` 先计算期望状态，与磁盘比对，相同则返回 `unchanged`，不写文件、不发事件、不重启。 | 保证"重复执行无副作用"不依赖 dsh/Loader 对相同 patch 列表的二次应用行为。 |
| **D6** | upstream 插件由后台同步任务**自动跟踪并自动应用**（默认开）；自研插件（`origin = in-house`：`link:`/`file:`/相对或绝对路径/tarball）**永不被同步任务改动**。git 源一律**钉 commit sha**（`github:owner/repo#<sha>`）。 | 复用 `core/github.rs:31-81,113-146` 的 `git_command/apply_git_auth/ls-remote` 与镜像；`apps/cli/reference/README.md:173` 明确"钉 commit 防止后续 push 悄悄改变运行内容"。 |
| **D7** | bundle 成员变更后**必须重启 dsh**（stop → 变更 → start 作为一次受管事务）；patch 变更**不重启**（`live` 热重载）。 | `README.md:58` 的启动边界；`web` 模板 `patchReload: live`（`profile.ts:115-118`）。 |
| **D8** | 技能/指令共享以 `%USERPROFILE%\.agents\agent` 为**唯一真源**；两种共享模式：**Mode L**（`~/.dsh/skills` junction + `AGENTS.md`/`CONTEXT.md` 符号链接，默认）与 **Mode C**（写 home 层受管 patch：`skill-filesystem.agentsHome`、`agent-instructions.dshHome`，无链接权限时的降级）。 | Mode L 是本机既有状态、零 dsh 配置；Mode C 的字段来自 `skill-filesystem/src/index.ts:56-57,164` 与 `agent-instructions/src/config.ts:19-20,40-42`。被否：复制文件（违反"避免复制/分叉"）；递归发现 `**/SKILL.md`（dsh 不支持，`skills.md:85`）。 |
| **D9** | 技能管理**不重建技能目录 UI**。列表/调用继续由 dsh 侧（`ctx.skills`、社区 `skill-explorer`）承担；启动器只负责"共享资源的检测/建立/修复/迁移"与状态展示。 | 复用现有能力，避免与 `@linxin666/dsh-client-ui-skill-explorer` 重复维护。 |
| **D10** | 新增 **无 GUI CLI**（同一二进制 argv 分发）与 Tauri IPC 两套入口，**共用同一 Rust core**；前端只做薄渲染。 | 让端到端验收可脚本化；业务逻辑集中在 Rust，沿用本仓库"逻辑在 core、组件薄"的既有结构。 |
| **D11** | 底部单按钮改为**三按钮组**，顺序固定 `插件 | 技能 | 设置`，各自打开对应 `Dialog`；`Dialog` 状态提升到 `App.tsx`（沿用现有 `settingsOpen` 模式）。 | `src/App.tsx:30-45`、`AppShell.tsx:186-198` 的既有模式；不引入新 UI 依赖。 |
| **D12** | 既有 `fix_incompatible_plugins` 的**意图保留、机制替换**：启动崩溃 → 解析 stderr 定位失败行 → 一键/自动**禁用（写 patch）**而非卸载；`dshmarket` 硬编码路径迁移为注册表里的一条隔离记录。 | 禁用可逆、可热重载，比卸载安全；`process.rs:513-638` 的现状被本决策取代。 |

---

## Architecture

### 1. 分层与职责

```
┌──────────────────────────── dsh-launcher (Rust) ────────────────────────────┐
│ commands/plugin.rs   commands/skill.rs        ← Tauri IPC（薄）              │
│ cli.rs                                        ← 无 GUI CLI 分发（薄）        │
│ ─────────────────────────────────────────────────────────────────────────── │
│ core/plugin/        状态机 + 注册表 + 受管区块 + dump 解析 + 同步计划         │
│ core/skill.rs       共享资源模式判定 + 链接/配置落地 + 迁移                   │
│ core/dshhome.rs     DSH_HOME / profiles / agents home 解析（唯一实现）        │
│ core/profile.rs     ★ 官方 API 适配器：唯一调用 dsh/pnpm、唯一读写 profile    │
│ ─────────────────────────────────────────────────────────────────────────── │
│ core/process.rs（既有，启动/停止/状态）  core/stream.rs（既有，流式+进度）    │
│ core/command.rs（既有，超时/无窗口）     core/github.rs（既有，git/镜像/Token）│
│ core/config.rs（既有，原子写/DPAPI）     core/events.rs（既有，事件推送）      │
└───────────────────────────────────────────────────────────────────────────┘
                 │ spawn / 读写                      ▲ 读取
                 ▼                                   │
        dsh CLI（官方接口）                    $DSH_HOME / ~/.agents/agent
```

**唯一出口原则**：`core/profile.rs` 是**唯一**允许
- 执行 `dsh`/`pnpm` 的地方；
- 读写 `$DSH_HOME/profiles/<p>/package.json`、`cordis.patch.yml`、`$DSH_HOME/cordis.patch.yml` 的地方。

其余模块（状态机、同步、技能）只依赖它暴露的类型化操作，禁止自行 `Command::new("dsh")` 或直接 `fs::write` 到 `$DSH_HOME`。这样"官方 API 调用边界"是可静态审计的。

### 2. 与现有架构的关系

| 现有抽象 | 本 ADR 的复用方式 | 不允许做的事 |
|---|---|---|
| `ProcessManager::start/stop/restart` + `op_lock`（`process.rs`） | 装卸/sync 前 `stop()`、成功后 `start()`；新增 `plugin_op_lock` 与 `op_lock` 同序获取避免死锁 | 不自行 spawn dsh；不绕过状态机改状态 |
| `core::stream::run_streamed/run_cmd_script` | 所有 pnpm 操作流式输出到日志面板 + 进度事件 | 不用阻塞 `.output()` 做长任务 |
| `core::command::run_with_timeout` | `--dump-config`、`git ls-remote`、`npm view` 全部带超时 | 不加无超时网络调用 |
| `core::github::{git_command, apply_git_auth}` + `github_mirror`/`github_token` | upstream git 源解析与同步 | 不新增 git 调用路径 |
| `AppConfig::save`（临时文件 + rename） | 受管区块与注册表的原子写 | 不直接 `fs::write` 覆盖 |
| `core::events::{progress, emit}` | 复用 `install://progress`，新增 `plugin://changed`、`skill://changed` | 不新造事件总线 |
| `lib.rs` `invoke_handler` | 追加 `commands::plugin::*`、`commands::skill::*` | 不改既有命令签名 |
| `src/lib/tauri.ts` | 追加类型化封装 | 组件内直接 `invoke` |
| `fix_incompatible_plugins`（`process.rs:580-638`） | 被 `plugin::repair` 取代（D12） | 保留写死的 `dshmarket` 卸载路径 |
| `keepDshHomeOnUninstall`（`config.rs:31`） | 卸载 dsh 时注册表与备份目录同规则处理 | 不在未确认时删用户数据 |

### 3. 进程/并发模型

- 所有 IPC 命令 `async` + `spawn_blocking`（沿用 `commands/version.rs` 的既有约定）。
- 新增两把锁：`plugin_op_lock`（装卸/sync/repair 全局互斥）与每插件的 `plugin:<pkg>` 轻量锁（同包操作串行）。
- 加锁顺序固定：`ProcessManager.op_lock` → `plugin_op_lock` → 包锁；禁止反向获取。
- 同步任务在后台线程运行，通过事件推进度；UI 关闭不影响任务完成。
- 目录/文件变更一律"先备份、后原子写"；备份落在 `%LOCALAPPDATA%\dsh-launcher\backups\plugins\<pkg>\<ts>\`。

---

## State Machine

### 1. 插件（bundle）状态

| 状态 | 判定（全部基于磁盘事实） |
|---|---|
| `uninstalled` | profile `package.json.dependencies` 中无该包 |
| `plain` | 依赖存在，但包 manifest 无 `dsh.bundle.patch`（`plugin.ts:36-45`） |
| `enabled` | 依赖存在 + 包在 `dsh.profile.bundles` 中 + 至少一个贡献行的有效 `disabled != true` |
| `disabled` | 依赖存在 + 包在 `dsh.profile.bundles` 中 + **全部**贡献行有效 `disabled == true` |

> 包在依赖里但不在 `bundles` 里不是稳定状态：下一次任意 `dsh plugin` 都会被对账加回（`plugin.ts:59-91`）。启动器把它归一为 `enabled/disabled` 的派生视图，并标记 `needsReconcile = true`。

**行级状态**：`enabled` / `disabled` / `expression`（`disabled` 值为 `!!js ...`，如 `bash-sandbox`，启动器拒绝覆盖）/ `absent`（dump 中不存在，通常是包已卸载）。

### 2. 转换表

| 动作 | 前置状态 | 后置状态 | 副作用 | 幂等语义 |
|---|---|---|---|---|
| `install(spec)` | `uninstalled` | `enabled`/`disabled`/`plain`（由包自身声明决定） | `dsh plugin add` + 重启（若 dsh 在跑） | 已安装且 spec 一致 → `unchanged`，不执行 pnpm |
| `enable(pkg)` | `enabled` / `disabled` | `enabled` | 受管区块写 `disabled: false`（每行） | 期望块 == 现状块 → `unchanged`，不落盘、不重启 |
| `disable(pkg)` | `enabled` / `disabled` | `disabled` | 受管区块写 `disabled: true`（每行） | 同上 |
| `uninstall(pkg)` | `enabled` / `disabled` / `plain` | `uninstalled` | `dsh plugin remove` + 清理该包受管条目 + 重启（若在跑） | 已是 `uninstalled` → `unchanged`（成功空操作） |
| `repair(pkg)` | 任意（含 `uninstalled`） | 收敛到注册表期望态 | 按期望态执行上述动作 | 收敛即空操作 |
| `quarantine(pkg)` | `enabled` | `disabled` | 写 `disabled: true` | 同 `disable` |
| `sync()` | — | 各 upstream 包收敛到目标 commit/version | stop → 逐包 apply → start | 无待同步项 → `unchanged` |

### 3. 非法转换（显式拒绝，返回 `IllegalTransition`）

- `enable` / `disable` 作用于 `uninstalled`（**不是**幂等空操作：语义上无对象）。
- `enable` / `disable` 作用于 `plain`（包不贡献任何行，无可启停对象）。
- `enable` / `disable` 的目标行全部为 `expression`（拒绝把 `!!js` 表达式替换为字面量）。
- `uninstall` 作用于受保护包：`@deepseek-ai/dsh-base`、`@deepseek-ai/dsh-web-app`（profile 模板层，`profile.ts:110-131`）。
- 任意动作作用于正在同步/安装的同一包 → `Busy`。
- 任意动作在 dsh 未安装（`probe_dsh_command() == None` 且无安装目录）时 → `DshNotInstalled`。
- 任意动作在 profile 的 `cordis.patch.yml` 结构非法（非顶层 YAML 数组 / marker 不配对）时 → `ManagedBlockConflict`，**不写任何字节**。

### 4. 状态机不变量

1. 受管区块**只**包含 `- id: <row>\n  disabled: <bool>` 形式的条目，绝不含 `config`/`insert`/`name`。
2. 受管区块之外的内容逐字节不变（含注释、空行、CRLF/LF 差异）。
3. 一个包的全部受管条目按 `(packageName, rowId)` 字典序排列，保证同一期望态产生同一字节序列（幂等的可判定基础）。
4. `uninstall` 成功后不得残留该包的受管条目（否则 dsh 每次启动都 warn `patch: entry X not found`）。
5. 注册表是**缓存**，磁盘是事实源：每次 `list` 都从磁盘重算状态，注册表只补 `origin/source/managedRows` 等不可从磁盘推断的元数据。

### 5. 受管区块格式（精确契约）

```yaml
# Your patch layer for this dsh profile, applied after every bundle layer:
# ...（用户原有内容，逐字节保留）...
[]

# >>> dsh-launcher managed v1 — 由启动器维护，请勿手工编辑 >>>
- id: cost-meter            # plugin: dsh-cost-meter
  disabled: false
- id: dsh-market            # plugin: dshmarket
  disabled: true
# <<< dsh-launcher managed v1 <<<
```

- 起始行固定 `# >>> dsh-launcher managed v1 ` + `—...`；结束行固定 `# <<< dsh-launcher managed v1 <<<`；版本号参与解析，未来升级走迁移。
- 不存在区块 → 追加到文件末尾（保留原结尾换行风格）。
- 区块存在 → 仅替换两行 marker 之间的行。
- marker 重复或缺失一半 → `ManagedBlockConflict`，拒绝写入。
- 所有写入的标量做 YAML 安全引用（行 id 必须先通过 `^[A-Za-z0-9._@/-]+$` 校验）。

---

## Data Flow

### 1. 发现（只读，可随时调用）

```
profile package.json ──┐
                       ├─► profile.rs::load_profile()  ──► 依赖集合 + bundles 顺序 + patchReload
dsh --dump-config ─────┘                                   │
   （line-based 解析：# == 段标签 → 行 id/name/disabled）    ▼
plugins.json（缓存）◄──── plugin::reconcile_view() ────► 状态机派生（enabled/disabled/plain/expression）
```

**dump 解析契约**（`core/plugin/dump.rs`）：只接受以下形态，其它列 0 起始行 → `DumpFormatChanged`（fail loud，不返回部分结果）：

| 行 | 语法 |
|---|---|
| 段头 | `^# == (?<owner>[^,]+)(?:, patched by (?<overlays>.*))?$` |
| 行首 | `^- id: (?<id>.+)$`（列 0） |
| 行属性 | `^  name: (?<name>.+)$`、`^  disabled: (?<value>.+)$`（2 空格缩进） |
| 其它 | 归入 config / 注释，忽略 |

值可能是单引号包裹（如 `'@linxin666/dsh-client-ui-task-graph'`）或 `!!js ...`；解析后去除引号，`disabled` 以 `!!js` 开头 → 行状态 `expression`。该语法用**实测 dump 快照**做 fixture 单测（含多行 bundle、被 overlay patch 的段、表达式 disabled、引号名）。

### 2. 变更（装卸 / 启停）

```
用户动作 ─► 状态机校验（非法即拒） ─► 计算期望态
   │                                    │
   │                          期望态 == 磁盘现状 ? ──是──► unchanged（结束，零副作用）
   │                                    │否
   ├─ 装卸类 ─► [stop dsh] ─► 备份 ─► dsh plugin add/remove ─► 校验 ─► 注册表 ─► [start dsh]
   └─ 启停类 ─► 备份 ─► 受管区块原子重写 ─► dsh 热重载（live）─► 校验（dump）─► 注册表
```

**校验**（每步失败即回滚）：
- 装卸后：依赖在 `package.json`；若包声明 `dsh.bundle` 则包名在 `dsh.profile.bundles`；`--dump-config` 退出码 0 且出现该 bundle 段。
- 启停后：`--dump-config` 中该包的全部行 `disabled` 与期望一致。
- 校验失败 → 从备份还原 `package.json`/`pnpm-lock.yaml`/`cordis.patch.yml` → 执行一次 `dsh plugin --profile web install` 使 `node_modules` 收敛 → 注册表标记 `lastError` → 返回 `VerificationFailed`（附 dsh 日志尾部）。

### 3. upstream 同步（D6）

```
sync()
 ├─ 读注册表：仅 origin == upstream 的包进入计划；in-house 直接跳过（断言，不静默）
 ├─ 逐包解析目标：
 │    npm 源    → npm view <pkg> version --registry <镜像>      （超时 120s）
 │    git 源    → git ls-remote <repo> refs/heads/<branch>      （超时 90s，复用 github.rs）
 │    （git 目标一律换算成 40 位 sha，spec 重写为 <repo>#<sha>）
 ├─ 计划为空 → unchanged
 ├─ 若 dsh 在跑 → stop()（只停一次）
 ├─ 逐包 apply（每包独立备份/校验/回滚；单包失败不中断批次）
 └─ 若曾运行 → start()，并对每个改动过的包做一次 dump 校验
```

- 默认策略：**自动应用**（可在设置中关闭为"仅检查/提示"）。
- 自研插件只读检测（若其路径是 git 仓库，可比对本地 HEAD 与远端，仅提示，不改动）。
- 同步结果写入注册表 `lastSync{at, from, to, result}`，并推 `plugin://changed`。

### 4. 技能/指令共享（D8）

```
检测阶段（只读）
  canonical = %USERPROFILE%\.agents\agent        （或 DSH_AGENTS_HOME 语义下的等价根）
  资源表：skills/、AGENTS.md、CONTEXT.md
  对每个资源探测 ~/.dsh 视图：
    不存在                      → missing
    是指向 canonical 的链接      → linked
    是真实文件/目录（有内容）    → conflict（绝不删除）
    是断链/指向别处              → broken
落地阶段
  Mode L（默认，需链接能力）：
    skills      → Junction（目录，无需特权）
    AGENTS.md   → 文件符号链接（需 SeCreateSymbolicLinkPrivilege / 开发者模式）
    CONTEXT.md  → 文件符号链接（同上；dsh 不读取，供 agent/技能引用）
  Mode C（文件符号链接不可用时的降级）：
    写 $DSH_HOME/cordis.patch.yml 受管区块：
      - id: skill-filesystem
        config: { agentsHome: '<canonical>' }
      - id: agent-instructions
        config: { dshHome: '<canonical>', maxBytes: 65536 }
    效果：dsh 直接从 canonical 读取 skills/ 与 AGENTS.md；CONTEXT.md 与 AGENTS.md 同目录，天然共址。
    代价：整行 config 被替换（必须复述 maxBytes）；对未挂载这两行的 profile 会打印
          “patch: entry X not found” 警告（`README.md:42`，非致命）。
```

**能力探测**：在 `%TEMP%` 建一个临时文件符号链接，成功即具备文件链接能力；失败则走 Mode C。探测结果缓存到 `skills.json`，用户可手动覆盖模式。

**冲突处理（永不删除用户内容）**：
- `~/.dsh/skills` 是真实目录且有内容 → 提供"合并到共享库"（同名不同内容保留两份并加后缀）或"保留原状、跳过共享"；两者都需用户确认。
- `~/.dsh/AGENTS.md` 是真实文件 → 同样只提示，Mode C 下会被 dsh 忽略（`files.ts:280-296` 只读 `dshHome/AGENTS.md`），迁移向导必须显式告知。

### 5. 运行时链路（端到端）

```
dsh-launcher start ─► process.rs 组装 argv（web --port --no-open）
   ─► dsh CLI 解析 --profile web（args.ts）
   ─► profile-boot.ts: prepareProfile + healProfilesModuleFallback
        ├─ 读 profiles/web/package.json → dsh.profile.bundles
        ├─ 每个 bundle 的 cordis.patch.yml
        ├─ profiles/web/cordis.patch.yml（含启动器受管区块）
        ├─ $DSH_HOME/cordis.patch.yml（Mode C 的共享配置）
        └─ --patch overlays（启动器不注入）
   ─► vendor/include applyEntryPatches：按 id 覆盖 disabled/config
   ─► Cordis Loader 挂载：disabled 行不激活；启用行进入 PENDING→ACTIVE
   ─► live 模式：watchUserPatches 监听两个用户层 → 启动器改受管区块即热重载
```

---

## API

### 1. 官方 API 调用边界（唯一允许的 dsh 交互）

| 用途 | 调用 | 约束 |
|---|---|---|
| 行发现 | `dsh --profile web --dump-config` | 只读、不启动插件、带超时；退出码非 0 → 视为 dsh 故障，不解析 |
| 依赖变更 | `dsh plugin --profile web add\|remove\|update\|install <spec...>` | 唯一写 `package.json`/`pnpm-lock.yaml`/`node_modules` 的通道；本地 spec 必须先转绝对路径（`plugin.ts:104-112`） |
| 版本/能力 | `dsh --version` | 仅在 `probe_dsh_command()` 判定可安全执行时（`github.rs:438-506`，防 pnpm 递归爆炸） |
| 启动/停止 | `ProcessManager::start/stop`（信号 + taskkill） | 沿用 ADR-0002 |
| 插件源解析 | `git ls-remote`（复用 `github.rs` 的 `git_command/apply_git_auth`）、`npm view --registry` | 带超时；Token 仅用于 git 认证 |

**启动器直接读写的文件（白名单）**

| 路径 | 权限 |
|---|---|
| `$DSH_HOME/profiles/web/package.json` | 只读（写只能经 `dsh plugin`） |
| `$DSH_HOME/profiles/web/cordis.patch.yml` | 仅受管区块 |
| `$DSH_HOME/cordis.patch.yml` | 仅受管区块（Mode C） |
| `$DSH_HOME/profiles/web/cordis.yml` | **禁止读写** |
| bundle 包内任何文件、`profiles/node_modules`、`profiles/*/.dsh-module-fallback` | **禁止**（dsh 自愈） |
| `%APPDATA%\dsh-launcher\{config.json,plugins.json,skills.json}`、`%LOCALAPPDATA%\dsh-launcher\backups\|logs` | 自有 |

`$DSH_HOME` 解析必须与 dsh 一致：`DSH_HOME` 非空白优先，否则 `%USERPROFILE%\.dsh`，支持 `~` 展开（对齐 `$DSH_SRC/packages/util/home-paths/src/index.ts:76-100`）。启动器不设置 `DSH_HOME` 时，子进程继承同一环境，双方一致。

### 2. CLI（同一二进制，`main.rs` 在 Tauri 初始化前分发）

```
dsh-launcher plugin list [--json]
dsh-launcher plugin install <spec> [--origin upstream|in-house]
dsh-launcher plugin enable  <package>
dsh-launcher plugin disable <package>
dsh-launcher plugin uninstall <package>
dsh-launcher plugin sync [--check] [--package <p>]
dsh-launcher plugin repair [--package <p>]
dsh-launcher skill status [--json]
dsh-launcher skill apply [--mode auto|link|config] [--resource skills|agents-md|context-md|all]
dsh-launcher skill migrate [--dry-run]
```

退出码：`0` 成功或幂等空操作；`2` 非法状态转换；`3` 未找到；`4` 忙；`5` 校验失败（已回滚）；`6` 能力缺失（如无链接权限且模式被强制为 link）；`7` 受管区块冲突；`8` dsh 未安装。`--json` 输出结构化结果，供集成测试断言。

### 3. Tauri IPC（`lib.rs` 追加）

```
plugin_list() -> PluginView[]
plugin_install(spec, origin) -> OpResult
plugin_set_state(package, desired: "enabled"|"disabled") -> OpResult
plugin_uninstall(package) -> OpResult
plugin_sync(apply: bool) -> SyncReport
plugin_repair(package?) -> OpResult
skill_status() -> SkillStatus[]
skill_apply(mode, resource) -> OpResult
skill_migrate(dryRun: bool) -> MigrateReport
```

事件：`plugin://changed`（列表失效）、`plugin://progress`（复用 `install://progress` 的 `{channel, phase, percent, message}` 结构）、`skill://changed`。

### 4. 前端（`src/lib/tauri.ts` 追加类型化封装）

```ts
export type PluginState = "uninstalled" | "plain" | "enabled" | "disabled";
export interface PluginView {
  package: string; version?: string; origin: "upstream" | "in-house" | "unknown";
  state: PluginState; rows: { id: string; name: string; state: "enabled"|"disabled"|"expression" }[];
  source?: { kind: "npm"|"git"|"path"; spec: string; repo?: string; commit?: string };
  lastSync?: { at: string; from: string; to: string; result: "ok"|"failed" };
  protected: boolean; needsReconcile: boolean;
}
export interface OpResult { status: "changed"|"unchanged"; restarted: boolean; message: string }
```

### 5. UI 入口（D11）

```
侧栏底部（AppShell.tsx，原单个 Button 位置）
┌──────────┬──────────┬──────────┐
│  插件    │  技能    │  设置    │   role="group" aria-label="管理入口"
└──────────┴──────────┴──────────┘
   ↑ PluginsPanel  ↑ SkillsPanel  ↑ SettingsPanel（既有）
```

- 三个 `Button variant="outline" size="sm"`，首/末项圆角合并；选中项 `aria-pressed="true"` + `variant="secondary"`。
- 三个 `Dialog` 状态统一在 `App.tsx` 管理（沿用 `settingsOpen` 模式），互斥打开。
- `PluginsPanel`：按状态分组（已启用/已禁用/普通依赖）、行级展开、启停开关、安装框（spec + origin 选择 + 信任确认）、同步按钮 + 待同步角标、每项显示来源与 commit/version。
- `SkillsPanel`：共享模式（auto/link/config）、三个资源的状态徽章、修复/迁移按钮、冲突说明；不重复实现技能目录浏览。

---

## Directory Layout

### 1. 仓库内新增/改动

```
src-tauri/src/
├── cli.rs                              [新增] 无 GUI CLI 分发（解析 argv → core）
├── core/
│   ├── dshhome.rs                      [新增] DSH_HOME/profiles/agents home 解析（唯一）
│   ├── profile.rs                      [新增] 官方 API 适配器（唯一 dsh/pnpm 出口）
│   ├── plugin/
│   │   ├── mod.rs                      [新增] 状态机 + 服务门面
│   │   ├── state.rs                    [新增] 状态/转换/非法转换错误类型
│   │   ├── registry.rs                 [新增] plugins.json 读写 + 磁盘对账
│   │   ├── managed.rs                  [新增] 受管区块 splice / marker / 原子写
│   │   ├── dump.rs                     [新增] --dump-config line-based 解析
│   │   ├── spec.rs                     [新增] spec 分类（upstream/in-house/unknown）+ sha 解析
│   │   └── sync.rs                     [新增] upstream 同步计划与执行
│   ├── skill.rs                        [新增] 共享资源检测/落地/迁移
│   └── mod.rs                          [改动] 注册新模块
├── commands/
│   ├── plugin.rs                       [新增] IPC
│   ├── skill.rs                        [新增] IPC
│   └── mod.rs                          [改动] 注册新模块
├── lib.rs                              [改动] invoke_handler 追加命令
└── main.rs                             [改动] 有 CLI 参数时走 cli.rs，否则进 Tauri

src/
├── App.tsx                             [改动] 三个 Dialog 状态
├── components/
│   ├── AppShell.tsx                    [改动] 底部三按钮组
│   ├── PluginsPanel.tsx                [新增]
│   ├── SkillsPanel.tsx                 [新增]
│   └── ui/button-group.tsx             [新增/可选] 组合容器（也可直接用既有 Button 拼装）
└── lib/tauri.ts                        [改动] 追加类型化封装与事件订阅
```

### 2. 运行时文件系统

```
%APPDATA%\dsh-launcher\
├── config.json                         既有（新增 autoSyncPlugins 等开关）
├── plugins.json                        新增：受管插件注册表（schemaVersion 1）
└── skills.json                         新增：共享资源模式与最近探测结果

%LOCALAPPDATA%\dsh-launcher\
├── toolchain\node\                     既有
├── github-dsh\deepseek-harness\        既有（GitHub 通道安装目录）
├── logs\                               既有
└── backups\plugins\<package>\<ts>\     新增：package.json / pnpm-lock.yaml / cordis.patch.yml 快照

$DSH_HOME\                              既有，启动器只按白名单触碰
├── cordis.patch.yml                    Mode C 受管区块（可不存在）
├── profiles\web\
│   ├── package.json                    只读（写经 dsh plugin）
│   ├── cordis.patch.yml                受管区块
│   └── cordis.yml                      禁止触碰
└── skills\                              Mode L 的 junction 视图

%USERPROFILE%\.agents\agent\            共享真源（canonical）
├── skills\<name>\SKILL.md              28 个技能（实测）
├── AGENTS.md                           用户全局指令（dsh 读取目标）
└── CONTEXT.md                          领域词表（agent/技能消费，dsh 不读）
```

### 3. `plugins.json`（schemaVersion 1）

```json
{
  "schemaVersion": 1,
  "profile": "web",
  "plugins": [
    {
      "package": "dsh-cost-meter",
      "origin": "upstream",
      "source": { "kind": "npm", "spec": "dsh-cost-meter@^1.7.13" },
      "rows": ["cost-meter"],
      "desired": "enabled",
      "installedSpec": "^1.7.13",
      "lastSync": { "at": "2026-09-10T02:11:00Z", "from": "1.7.13", "to": "1.7.14", "result": "ok" },
      "protected": false
    },
    {
      "package": "my-local-plugin",
      "origin": "in-house",
      "source": { "kind": "path", "spec": "link:D:\\work\\my-local-plugin" },
      "rows": ["my-local-row"],
      "desired": "enabled",
      "protected": false
    }
  ]
}
```

---

## Migration

### 1. 注册表引导（首次运行，只读）

1. 读 `$DSH_HOME/profiles/web/package.json` → 依赖集合 + `dsh.profile.bundles`。
2. `--dump-config` → 每包的行集合。
3. spec 分类：`link:`/`file:`/相对或绝对路径/`.tgz` → `in-house`；`git+`/`github:`/`https://...git`/registry 版本区间 → `upstream`；其余 → `unknown`（**自动同步默认关闭**，UI 要求确认）。
4. **引导阶段零写入**（不碰 profile 任何文件），只落 `plugins.json`。
5. 已知隔离记录迁移：把既有 `fix_incompatible_plugins` 的 `dshmarket` 特例写成 `{"package":"dshmarket","quarantine":true}` 的注册表条目；行为由"卸载"改为"禁用其行"（D12），用户可一键恢复。

### 2. 技能共享迁移（本机实测状态可直接采纳）

| 当前磁盘状态 | 迁移动作 |
|---|---|
| `~/.dsh/skills` 是指向 `~/.agents/agent/skills` 的 Junction | 判定为 `linked`，**不做任何改动**，仅在 `skills.json` 记录 |
| `~/.dsh/AGENTS.md`、`CONTEXT.md` 是指向 canonical 的符号链接 | 同上 |
| `~/.dsh/skills` 为真实目录且有内容 | 提示"合并到共享库"（同名冲突保留两份）或"跳过"；**绝不自动删除** |
| `~/.dsh/AGENTS.md` 为真实文件 | 提示并保留；选择 Mode C 时明确告知该文件将不再被 dsh 读取（`files.ts:280-296`） |
| canonical 目录不存在 | 由用户选择新建（`skills/`、`AGENTS.md`、`CONTEXT.md` 三个空/模板文件），或保持"未启用共享" |

### 3. 兼容性

- **dsh 版本**：`dsh plugin` 与 `--dump-config` 在 npm/GitHub 两通道均存在（`apps/cli/src/args.ts:171-181`）。Mode C 依赖的 `skill-filesystem.agentsHome` / `agent-instructions.dshHome` 若在旧版本不存在，落地后校验会发现行配置未生效 → 自动回退 Mode L 或提示升级。
- **旧版启动器**：`plugins.json`/`skills.json` 是 `%APPDATA%` 下新文件，旧版本不读不写，互不影响。
- **社区插件**：`.dsh-market/state.json` 等自建账本**不接管**；若检测到其 `disabled` 列表与受管区块冲突，UI 提示"两套启停账本冲突"，由用户选择以哪套为准（默认：受管区块优先，因为它在 dsh 的层序里真正生效）。
- **用户手写 patch**：受管区块之外的条目逐字节保留；同一行 id 同时被用户层与受管区块覆盖时，受管区块在文件末尾、层序更靠后 → 启动器生效，UI 必须显式提示该覆盖关系。
- **回滚**：删除受管区块 = 回到 bundle 默认；`keepDshHomeOnUninstall` 语义不变（`config.rs:31`）。

---

## Testing

### 1. 单元测试（`cargo test`，模块内 `#[cfg(test)]`）

| 目标 | 用例要点 |
|---|---|
| `core/plugin/state.rs` | 全部合法转换；全部非法转换返回 `IllegalTransition`；`expression` 行拒绝；受保护包拒绝 |
| 幂等 | 同一期望态二次调用返回 `unchanged`；文件 mtime 与字节均不变 |
| `core/plugin/managed.rs` | 无区块→追加；有区块→只替换块内；块外字节（含 CRLF/注释/空行）逐字节不变；marker 缺失/重复→`ManagedBlockConflict` 且零写入；字典序稳定性 |
| `core/plugin/dump.rs` | 实测快照 fixture：多行 bundle、`patched by` 段、单引号名、`!!js` disabled、列 0 未知行 → fail loud |
| `core/plugin/spec.rs` | `link:`/`file:`/相对/绝对/`.tgz` → in-house；`github:owner/repo#sha`/`git+`/版本区间 → upstream；其余 unknown |
| `core/dshhome.rs` | `DSH_HOME` 优先、空白视为未设、`~` 展开、Windows 分隔符 |
| `core/skill.rs` | missing/linked/conflict/broken 判定；Mode C 生成的 patch 内容；能力探测失败降级 |
| `commands/version.rs` 回归 | 既有卸载/保留 DSH_HOME 行为不变 |

### 2. 集成测试（`src-tauri/tests/`）

- 临时 `DSH_HOME` + 临时 profile 目录 + PATH 上的假 `dsh`/`pnpm` 脚本（记录 argv、模拟 dump 输出），覆盖：
  - `install` 成功路径 / pnpm 非零退出 → 文件还原且 node_modules 收敛；
  - `disable` → 受管区块内容正确 + dump 校验通过 + 不触发重启；
  - `enable` → 幂等空操作；
  - `uninstall` → 依赖与受管条目同时清理，无残留 `entry not found`；
  - `sync` 批次中单包失败不影响其它包；`in-house` 包 argv 中**永不出现**；
  - 并发 `enable`/`uninstall` 同包 → `Busy`，无文件交错写。
- 链接能力测试：不具备文件符号链接权限时断言自动选择 Mode C。
- CLI 集成：`dsh-launcher plugin list --json` 在假环境下输出可断言的 JSON 与退出码。

### 3. 端到端（干净 Windows，脚本化 + 人工确认）

1. 安装 dsh（GitHub 通道）→ `dsh-launcher plugin list` 显示 7 个既有插件，状态与 dump 一致。
2. `plugin disable dsh-cost-meter` → dsh 运行中，无需重启，日志出现该行卸载；`--dump-config` 显示 `disabled: true`。
3. 重复执行同一命令 → 输出 `unchanged`，`cordis.patch.yml` 字节不变。
4. `plugin enable` → 行恢复，`disabled: false`。
5. `plugin install github:<owner>/<repo>#<旧 sha>` → 段出现；`plugin sync --check` 报告可更新；`plugin sync` → commit 前进、dsh 自动重启、段仍在。
6. `plugin install link:<本地路径> --origin in-house` → `plugin sync` 后该包 argv 与文件均未变。
7. `plugin uninstall` → 段消失、依赖消失、无受管残留；`plugin uninstall` 再来一次 → `unchanged`。
8. 技能：`skill status` 显示 3 个资源 `linked`；人为断链后 `skill apply` 修复；关闭开发者模式后 `skill apply --mode auto` 落到 Mode C 且 dsh 仍能读到 28 个技能。
9. 启动崩溃修复：人为注入一个坏插件 → 启动失败 → 提示失败行 → 一键禁用 → 启动成功（不再卸载）。
10. 既有回归：`cargo test`、`cargo build --release`、`tsc --noEmit`、`vite build`、`scripts/verify-layout.py` 全绿；`dsh web` 启动/停止/内嵌窗口/日志/工具链/版本管理不受影响。

---

## Risks

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| R1 | 用户手工编辑 `cordis.patch.yml` 与受管区块冲突 | 启停失效或覆盖用户意图 | marker 契约 + 块外逐字节保留 + 同 id 冲突显式提示 + 备份 |
| R2 | dsh 的 dump 输出格式变化（js-yaml 版本/新增字段） | 解析失败 | 语法白名单 + 列 0 未知行 fail loud + 快照 fixture 回归；解析失败时降级为"只读列表 + 禁用启停按钮" |
| R3 | 行 `disabled` 为 `!!js` 表达式 | 覆盖会改变平台语义 | 行状态 `expression`，拒绝覆盖，UI 只读展示 |
| R4 | bundle 成员变更未重启导致"看起来没生效" | 用户困惑 | 装卸类操作内建 stop→apply→start 事务；UI 明确提示"已重启/需手动重启" |
| R5 | pnpm ≥10 拒绝 git 依赖的 `prepare` 脚本（`allowBuilds`） | 安装失败 | 不自动写 `allowBuilds`；把 pnpm 打印的键名原样呈现，要求用户显式确认后由启动器写入并复跑（`README.md:164-173`） |
| R6 | 安装第三方插件即执行其代码（供应链） | 任意代码执行 | git 源钉 sha、展示 repo/commit、默认拒绝 `http://` 与非绝对本地路径、日志脱敏、备份可回滚 |
| R7 | Windows 文件符号链接需特权 | Mode L 失败 | 能力探测 + Mode C 降级；junction（目录）无需特权 |
| R8 | Mode C 的整行 `config` 替换 | 覆盖该行其它配置 | 写入前读出 dump 中该行的现有 `config` 并复述必需字段（如 `maxBytes: 65536`）；仅写启动器明确管理的键 |
| R9 | home 层 patch 对未挂载目标行的 profile 产生警告噪音 | 日志不洁 | 受管区块写入前确认目标行存在（dump 探测）；否则不写 home 层并提示 |
| R10 | 与社区插件（`.dsh-market/state.json`）双账本 | 状态不一致 | 只读检测 + 冲突提示，不接管、不删除 |
| R11 | 同步期间网络/磁盘中断 | 半完成状态 | 每包独立备份与回滚；批次级 stop/start 保证不重启到半装状态 |
| R12 | 并发 IPC/CLI 同时操作 | 文件交错、dsh 反复重启 | 全局 + 每包锁、固定加锁顺序、`Busy` 返回 |
| R13 | DSH_HOME 与子进程不一致 | 改错目录 | 唯一解析实现（`core/dshhome.rs`）+ 启动时日志打印解析结果 |
| R14 | 受管区块被 dsh 自身的 write-back 覆盖 | 启停丢失 | 已确认 dsh 只重写 `cordis.yml`（`profile-boot.ts:119-123`），不写 `cordis.patch.yml`；仍以"写后校验 + 不一致告警"兜底 |

---

## Acceptance Criteria

| 验收项 | 验证方式 |
|---|---|
| 插件可按 ID 独立管理生命周期 | `dsh-launcher plugin list --json` 返回每个包的 `state`；`install/enable/disable/uninstall` 仅作用于目标包，其它包的 `package.json`/受管条目字节不变 |
| 非法状态转换被拒绝 | 对 `uninstalled` 执行 `enable/disable`、对 `plain` 执行启停、对受保护包执行 `uninstall`、对 `expression` 行启停 → 退出码 `2`/`6`，零文件写入（单测覆盖） |
| 重复执行状态操作无副作用 | 连续两次 `disable`：第二次返回 `unchanged`，`cordis.patch.yml` 与 `plugins.json` 的 mtime/字节不变；连续两次 `uninstall`：第二次 `unchanged` |
| upstream 插件无需人工逐个同步 | `plugin sync` 一次调用推进全部 `origin=upstream` 的待更新包（npm 版本或 git sha），自动 stop→apply→start，并逐包 dump 校验 |
| 自研插件不受 upstream 同步影响 | 假环境集成测试断言：`in-house` 包的 pnpm argv、`package.json` 依赖 spec、受管条目在 `sync` 前后完全一致 |
| `~/.dsh` 可直接复用 `~/.agents/agent` 的共享技能资源 | `skill status` 显示 skills/AGENTS.md/CONTEXT.md 为 `linked`（Mode L）或 `config`（Mode C）；dsh 会话中 `ctx.skills.list()` 能列出 canonical 目录下的全部技能；Mode C 下 `agent-instructions` 从 canonical 读取 AGENTS.md |
| 现有功能保持兼容 | `cargo test`、`cargo build --release`、`tsc --noEmit`、`vite build`、`scripts/verify-layout.py` 全绿；启动/停止/重启、内嵌 Web GUI、工具链、版本管理、日志、托盘行为回归通过 |
| 新增能力具备单元/集成/E2E | 上表三节测试全部落地并通过；CLI 集成测试可在 CI 无 GUI 环境运行 |
| 不修改 dsh 源码 | `git -C $DSH_SRC status --porcelain` 为空；启动器对 `$DSH_SRC` 只读（仅调用其 CLI） |

---

## Consequences

- **正面**：插件生命周期有单一事实源、显式状态机与幂等语义；启停无需重启（`live` 层）；上游同步自动化且自研插件隔离；技能共享从"手工链接"变成"可检测、可修复、可降级"的受管资源；插件/技能/设置三个入口一致。
- **负面/成本**：新增 `plugins.json`/`skills.json` 两份状态与一个受管区块约定，需要迁移与冲突处理；Mode C 会替换两行的 `config` 并在部分 profile 产生警告；`--dump-config` 成为关键依赖，需为其格式漂移准备降级路径。
- **被取代**：`core/process.rs::fix_incompatible_plugins` 的硬编码 `dshmarket` 卸载（改为注册表驱动的禁用/隔离）。
- **后续**：若 dsh 未来提供官方插件状态/启停命令，本 ADR 的 `core/profile.rs` 适配器层可整体替换实现，状态机与 UI 不变。
