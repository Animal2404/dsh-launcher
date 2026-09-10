# CONTEXT — dsh-launcher 词表

> 本文件是**词表（glossary）**，不是 spec，不记录实现细节。实现决策见 `docs/adr/`。

## 核心名词

- **dsh（deepseek-harness）**：DeepSeek AI 开源的 agent harness，`@deepseek-ai/dsh`。以 `dsh web` 启动 Web GUI，默认 `http://127.0.0.1:3080`。
- **dsh-launcher（本产品）**：管理 dsh 及其工具链的 Windows 桌面启动器。本身不是 dsh 的一部分，不修改 dsh 源码。
- **运行目录（workspace root）**：启动 dsh 时所在目录，作为默认 workspace 根目录。由用户在启动器里配置，每次启动可改。
- **DSH_HOME**：dsh 自管的数据目录（`profiles/<name>`、`settings.yaml`、`.credentials.yaml` 在其中）。启动器**只读展示、不接管**。
- **profile**：dsh 的具名应用组合，如 `web`、`headless`、`sdk`、`acp`。`dsh web` 是 `--profile web` 的别名。启动器主控 `web` profile。
- **工具链（toolchain）**：dsh 运行依赖的可执行环境：Node.js、npm、pnpm、Git，以及可选检测项 Python。全部**全局安装、单版本**，无用户级目录。
- **安装通道（install channel）**：dsh 的来源。两个：**GitHub 源码通道**（v0.1.2-alpha.1 及更新版本，clone + pnpm 构建）与 **npm 通道**（registry 现有版本，当前最新 0.1.1-rc.2）。全局同时只有一个激活版本，切换 = 先卸载再装（ADR-0003）。
- **版本目录（version dir）**：全局安装的 dsh 版本目录。npm 通道与 GitHub 通道产物不同，各自独立；全局仅激活一个。

## 状态

- **dsh 运行状态**：`running` | `stopped` 等五态（见 DshStatus）。判定 = 本启动器托管进程事件 + 收养实例的**进程身份匹配**（端口监听者命令行需形如 dsh，防误杀无关进程）+ 每 5 秒后端兜底对账（v0.4.13）。
- **工具链状态**：`present`（版本满足）| `missing`（不存在）| `mismatch`（存在但版本不符）。可检测、可一键安装/升级、可配镜像源。
- **工具链安装模式**：Node = 官方 zip 解压到**用户级目录** `%LOCALAPPDATA%\dsh-launcher\toolchain\node` 并写入用户 PATH（免 UAC）；Git/Python = 官方安装器（提权，可能触发 UAC）。npm/pnpm 随 Node zip 自带于同一目录。

## 运行目录

- **workspace root**：启动 dsh 时的 CWD，由 dsh 官方行为决定（默认该目录）。启动器**不配置、不干预、不校验**。

## 插件（ADR-0005）

- **受管 profile**：启动器唯一管理的 profile，固定为 `web`（`dsh web` 的官方别名目标）。
- **插件（profile bundle）**：`dsh.profile.bundles` 里的一层，由 `dsh plugin` 官方通道装卸。
- **插件状态**：`uninstalled`（依赖不存在）| `plain`（普通依赖，不声明 `dsh.bundle`，不可启停）| `enabled` | `disabled`。
- **启停**：写 profile 的 `cordis.patch.yml` 受管区块（`- id:` + `disabled:`），由 dsh `live` 热重载，**不需要重启**。
- **装卸**：走 `dsh plugin --profile web add/remove`，属于 bundle 成员变更，**必须重启 dsh**（启动器自动完成）。
- **受管区块（managed block）**：启动器在 `cordis.patch.yml` 中拥有的一段（marker 包裹），块外内容逐字节保留。
- **upstream 插件 / 自研插件**：`upstream` 参与自动同步（npm 版本或 git commit）；`in-house`（本地路径/link/tarball）**永不被同步任务改动**。

## 技能共享（ADR-0005）

- **共享真源（canonical store）**：**官方 `agentsHome` 根**（默认 `~/.agents`）——技能 = `~/.agents/skills`（官方 `skill-filesystem` 的 `user-agents` 根，rank 500）；指令 = `~/.agents/AGENTS.md`；词表 = `~/.agents/CONTEXT.md`（dsh 不读）。**不再是 `~/.agents/agent`**（ADR-0006 D17 修订）。
- **共享模式**：`link`（`<dshHome>/AGENTS.md` 链接到真源；技能**无需链接**，由官方 rank 500 原生覆盖）| `config`（写 `$DSH_HOME/cordis.patch.yml` 的 shared 区块，让 dsh 直接读真源）。
- **资源状态**：`missing` | `linked` | `config` | `conflict`（真实文件/目录，绝不自动删除）| `broken`（链接指向别处或**断裂**）| `native`（该资源**不需要**链接，真源已由官方扫描根原生覆盖）。
- **需要链接判定（`needs_link`）**：由官方事实决定 —— `agents-md` **需要**（官方固定读 `<dshHome>/AGENTS.md`）；`skills` 与 `context-md` **不需要**（技能走官方 rank 500 根；dsh 不读 `CONTEXT.md`）。判定链顺序为「链接 → 真实文件 → 原生根」，因此视图侧遗留的**断链**不会被误判成 `native`。
- **链接修复动作（`skill repair-links`）**：一次性、**可幂等重跑**的迁移动作 —— 修复需要链接的资源；清理**启动器自己创建的断链**（目标落在 `agentsHome` 下的）；保留一切含用户内容的真实文件/目录，以及指向 `agentsHome` **之外**的用户自建链接。绝不删除用户内容。
- **白名单唯一例外（失败回滚）**：官方无"回退到任意历史 lock 状态"的能力，故 `package.json` / `pnpm-lock.yaml` / `pnpm-workspace.yaml` / `cordis.patch.yml` 仅允许在**失败回滚**时由备份还原写入（`core/plugin/mod.rs::rollback_after_failed_official_op` **一处**），且其后**必跟**一次官方通道 `dsh plugin … install` 收敛（ADR-0006 D18）。

## MCP server 管理（ADR-0006）

- **MCP server 条目**：cordis 配置树中的一行，`name: '@deepseek-ai/dsh-mcp-client'`，其 `config.serverName` 是该服务器面向模型的命名空间。一个条目 = 一个 MCP server。仅桥接 Tools（resources / prompts 不支持）。
- **serverName**：条目的面向模型命名空间，工具名形如 `mcp__<serverName>__<tool>`。须匹配 `[A-Za-z0-9_-]{1,32}`，且在同一**注册作用域**内唯一。启动器以它作为管理标识键（而非行 id）。
- **行 id（rowId）**：条目的 cordis 标识；启动器声明的行固定用 `mcp-<serverName>`。
- **受管 MCP 区块（managed MCP block）**：启动器在 `$DSH_HOME/cordis.patch.yml`（机器级）中拥有的一段（marker 包裹），**两段式**——`insert:` 声明段 + `id`/`disabled:` 定向段。块外内容逐字节保留。
- **受管声明 / 外部声明**：条目的 `- insert:` 行位于受管区块内 = **受管声明**（`remove` 可真删除）；由 bundle、profile patch、home 文件用户区或 `--patch` 给出 = **外部声明**（`remove` 仅撤销定向覆盖，声明仍在）。
- **定向覆盖（id-targeted override）**：`- id: <rowId>` + `disabled:` 形式的独立 patch 条目，用于覆盖既有行的启停；可命中同层后置条目与**全部更早层**（home 层是最后持久层，故机器级定向覆盖对全层有效）。
- **注册作用域（registration scope）**：官方对 `serverName` 唯一性的强制范围；同一作用域内重复时，**后加载的实例在加载期失败，先前实例不受影响**。启动器只管理 root 作用域。
- **MCP 状态**：`missing`（合成树无该 serverName 的行）| `enabled`（有效 `disabled != true`）| `disabled`（有效 `disabled == true`）。行级另有无状态标记：`expression`（`disabled` 为 `!!js`，拒绝覆盖）| `conflict`（serverName 重复）| `dangerous`（`failOnStartupError: true`，可使 harness 启动中止）。
- **MCP 前置（prerequisite）**：`@deepseek-ai/dsh-mcp-client` 在受管 profile 的**可解析性**（非 `dependencies` 成员、非 bundle）；缺失时 MCP 页显示横幅并引导至插件页安装，**MCP 页不重复建设装卸入口**。
- **危险字段 `failOnStartupError`**：官方默认 `false`（失败仅 warn、harness 照常启动）；设为 `true` 时初始连接失败会让该 fiber FAILED，进而使**整个 harness 启动中止**。故 `disabled: true` 是该行唯一安全的隔离手段；启动器结构化通道不暴露该字段。
- **可观测性边界**：官方**不存在**列 MCP 状态或列工具的 CLI；`--dump-config` **不启动插件**、只证明**配置合成层**；`pluginInventory/list` 为 Remote-only 且不含 `serverName` 与 `mcp__*` 工具名。故脚本化证据只有「重新 dump 的目标行 `disabled`」与「dsh stderr 的 logger 行 + 有界窗口」，工具是否出现由人工确认（ADR-0006 D12 / §Testing 3.3）。

## 镜像源（镜像维度）

- **npm registry**：npm 通道装包 + pnpm 依赖安装的 registry。
- **GitHub 加速**：源码 clone（git 镜像）与 GitHub Release 下载（代理）共用。
- **Node/Git 二进制源**：工具链安装包的下载源。

## 生命周期

- **启动**：spawn `dsh web --port <p>`，注入工具链 PATH，CWD = 运行目录（dsh 官方默认）。
- **停止**：Windows 用 `taskkill /PID /T` 模拟优雅停止（1 秒等待，未退出即 `/F` 强杀），随后按端口兜底清剿（清剿前校验监听进程确为 dsh，防误杀）。**无官方 stop 命令**。
- **托盘**：关闭窗口→托盘；托盘菜单 = 打开主窗口 / 启动 / 停止 / 重启 / 退出。
- **设置开关（共 6 项）**：closeExits 关闭直接退出 / minimizeToTray 最小化到托盘 / keepDshOnExit 退出驻留 dsh / keepDshHomeOnUninstall 卸载保留 DSH_HOME（默认开）/ autoStartDsh 启动时自动启动 dsh / autoOpenBrowser 启动时自动打开 Web GUI。
- **退出**：托盘"退出"先优雅停止 dsh，再退出启动器；keepDshOnExit 开则驻留直接退出。
- **重启**：停止 → 用相同配置（端口）重新启动；端口被占则提示手动改。
- **运行状态检测**：事件驱动（Rust 持有 dsh 子进程句柄，退出即收状态）+ 端口探活（启动/停止/收养校验）+ 后端每 5 秒对账线程兜底（v0.4.13）。
