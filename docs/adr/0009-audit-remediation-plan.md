# ADR-0009 — 全链路审计发现修复计划（v0.9.0 基线）

- **状态**：Accepted（2026-09-12 Phase 0 已实施并验证；Phase 1–4 待实施）
- **日期**：2026-09-12
- **修订记录**：
  - `2026-09-12` Phase 0：对三条结论做**实测复核**，其中 **D3 的初稿判据被证伪**、**P2-11 的影响面被证伪**，均已按实测更正（见各条「实测复核」小节）。修订依据是 PowerShell 与 `cargo test` 的真实运行结果，而非静态推断。
  - `2026-09-12` **完成度核验（D1–D22 逐条）**：发现并补齐 **3 处缺口** ——
    ① D19 的 `plugin_whitelist_test.rs` 失效引用（`core/plugin/mod.rs:586`）；
    ② D20 的两处布局语义改动经**可达性穷举证伪**（各自 138,240 / 46,080 组），已收窄为"仅实施 `"use client"` 清理"并单列说明；
    ③ D12 的实施事实（**未拆分**文件）未回写，文档 5 处仍按"已拆出 `part_b_static_assert_test.rs`"表述，已统一订正。
    核验同时纠正了**核验方法自身的缺陷**：首轮 grep 因未剥离注释而把 D4 的"注释中引用旧写法"误判为未修复；后续核验统一采用「剥离注释后再匹配代码」。
- **范围**：对 `dsh-launcher` 在 `v0.9.0` 基线上执行的一次全链路只读静态审计（前端组件/状态/样式/可访问性、Rust core 与 commands、Tauri 窗口与 IPC、依赖与工程配置、测试与 CI、文档一致性）所产出的全部发现的**修复计划**。本文只定义"怎么修、按什么顺序修、如何验证"，不含任何代码改动。
- **硬边界**：**不修改 `deepseek-harness` 任何代码**；**不新增 npm 依赖**；**不新增 Rust 依赖**；修复不得放宽任何既有 capability 授权。
- **事实依据**：审计对约 90 个手写文件逐文件通读（前端 31、Rust 50+、CI 2、脚本 4、文档 12），全部结论均带 `文件:行号` 证据。本文引用的关键行号已在撰写时二次核对。

---

## Context

### 1. 审计结论摘要

| 严重度 | 数量 | 性质 |
| --- | --- | --- |
| P0 阻断（安全 / 数据 / 崩溃） | 0 | 未发现必然导致崩溃、数据损坏或越权的缺陷 |
| P1 严重（阻断发版） | 1 | 版本号五文件漂移，`bump-version.mjs` 一致性保护会直接 `exit(1)` |
| P2 一般（隐患 / 冗余 / 失效测试） | 20 | 安全口径漂移、假测试与 CI 缺口、文档脱节、前端竞态与行为缺陷 |
| P3 提示（规范 / 风格 / 脆弱实现） | 28 | 可访问性、死代码、冗余样式、陈旧注释、供应链 pin、**2 条由 P2 实测降级** |

> **实测复核说明**：上表已并入 Phase 0 的复核结果 —— 原判 P2 的两条经实测降级为 P3：**D3**（CHANGELOG 防重，原写法实测可正确命中）与 **D4**（MCP 恒假守卫，其守护的不变量由上游保证）。审计初稿在这两条上存在**未经实测的判断错误**，已如实更正而非保留原判。

安全面整体扎实：GitHub Token 走 DPAPI 加密落盘（`core/config.rs:190-212`）、`read_log` 有规范化前缀校验防穿越（`commands/logs.rs:90-96`）、受管区块 YAML 渲染强制单引号包裹并校验行 id（`core/plugin/managed.rs:130-146`）、停进程前做 `cmdline_looks_like_dsh` 身份校验防误杀（`core/process.rs:767-792`）。本次修复**不推翻**这些设计。

### 2. 三个根因（决定修复策略，而非逐条打补丁）

**根因 A — 版本号存在"第二入口"，破坏脚本的权威性。**
`bump-version.mjs` 是唯一同步点（五文件），但 `package.json` 的 `version` 可被手工修改。一旦手工改过而未跑脚本，`package-lock.json` 就停留在旧值：当前 `package-lock.json:3,9` 为 `0.7.1`，而 `package.json:4`、`Cargo.toml:3`、`tauri.conf.json:4`、`Cargo.lock:834` 均为 `0.9.0`。脚本的一致性保护（`scripts/bump-version.mjs:54`）此时从"防漂移护栏"变成"发版阻断器"。**因此修复必须同时做三件事：对齐当前漂移、堵住第二入口、把一致性变成 CI 可判定的不变量。**

**根因 B — 有两处"业务决策变更后未回收旧实现"的残留。**
- token 口径：v0.4.13 引入打码（`core/logging.rs:429` `redact_web_token`），v0.5.6 改为明文（`core/process.rs:824-826` 注释自述"产品决策"），但打码函数与陈旧注释（`core/logging.rs:449` 仍称"日志正文已对 token 打码"）未回收 → 死代码 + 注释与实现互相矛盾。
- MCP 启停守卫：`core/mcp/mod.rs:663` 的 `row.row_id != row_id` 是同一变量自比较，恒为 `false`，使该守卫**恒不可达**（**实测复核：属死代码清理，非功能缺陷**——其守护的不变量已由上游 `find_in_tree` 保证，见「D4 精确契约」）。

**根因 C — 测试与 CI 的"覆盖假象"。**
6 个测试（`channel_mutex_logic_test`、`path_inject_test` 两用例、`icon_window_test`、`concurrency_test`）只测测试文件内自行复刻或纯字符串拼接的逻辑，**不触达生产代码**；与此同时 2 个真正离线可跑的端到端测试（`plugin_pipeline_test`、`mcp_pipeline_test`）**未列入 CI**。结果是"CI 绿"与"生产受保护"之间没有因果关系。

### 3. 不修的后果（按时间线）

1. **下一次 push 到 master 即失败**：`release.yml:87` 判定 tag `v0.9.0` 已存在 → `:107` 执行 `bump-version.mjs --patch` → 因漂移 `exit(1)`，发布流水线中断（根因 A）。**已由 Phase 0 消除**。
2. **CHANGELOG 防重为健壮性改进而非缺陷**：`release.yml:125` 的 `-match "\[$newVersion\]"` 经实测**能正确命中**已存在条目（`\[`/`\]` 是转义字面括号，`.` 是单字符通配）。审计初稿"字符类永不命中"的判断**经实测证伪**，已更正（见「D3 精确契约」的实测复核）。
3. **MCP 不变量 #2 的守卫恒不可达**（**冗余，非功能缺陷**）：`core/mcp/mod.rs:663` 的 `row.row_id != row_id` 是同一变量自比较，恒 `false`。但该守卫的**另一半**条件（受管区块无该 rowId 声明）在 `external` 行场景下本就应放行——`remove` 的 `McpOrigin::External` 分支（`:725-752`）明确以"external 行可被写定向覆盖"为设计意图。因此恒假条件使这段守卫**永不执行且永不需要执行**；写入 external 行定向覆盖**不会**产生 dsh 启动告警（告警前提是"声明完全不存在"，而 external 行在合成树中确有声明）。属**死条件**，删或改均为清理性质。
4. **带 token 的访问 URL 明文落盘 + 误导性死代码**（根因 B）：`%LOCALAPPDATA%\dsh-launcher\logs\<date>.log` 与前端实时流均含完整 token（**该明文口径经裁定保留**），但同时仓库中**残留**已无调用方的 `redact_web_token()` 与单测 —— 后续维护者会误以为已打码，且注释与实现互相矛盾。**风险不在明文本身，而在"实现说一套、注释说另一套"**。

---

## Decision

| # | 决策 | 关键理由 |
| --- | --- | --- |
| **D1** | 对齐 `package-lock.json` 至 `0.9.0`，并把"五处版本一致"升级为 CI 可判定的不变量 | 根因 A：护栏失效源于无 CI 判定 |
| **D2** | `bump-version.mjs` 确立为**唯一**版本变更入口；`package.json` 的手工版本修改在评审中被拒绝 | 消除第二入口 |
| **D3** | `release.yml:125` 的 CHANGELOG 防重改为**字面量子串匹配**（`Contains`）——**定性为健壮性改进，非缺陷修复**（原写法经实测可命中） | 消除正则元字符歧义：版本号的 `.` 在正则中退化为通配符 |
| **D4** | 清理 `core/mcp/mod.rs:663` 的**恒假死条件**（`row.row_id != row_id`），并补纯函数单测锁定语义——**定性为死代码清理，非缺陷修复** | 恒假条件恒不可达；external 行写定向覆盖本就是设计意图 |
| **D5** | token 口径**定为「不打码」（产品所有者 2026-09-12 裁定）**：日志与前端日志流保留**完整** dsh web 访问地址；删除已成死代码的 `redact_web_token()` 及其单测，并把口径集中记录于 `core/logging.rs`，消除"实现明文 / 注释称已打码"的矛盾 | 明文换取可用性：用户需从日志复制完整地址在外部浏览器打开，裸 URL 会被 dsh 以 401 拒绝；安全权衡由产品所有者明确接受（日志位于本机用户私有目录） |
| **D6** | 主窗口与内嵌 Web GUI 窗口设置**最小 CSP**，替换 `tauri.conf.json:25` 的 `"csp": null` | 纵深防御；内嵌窗口渲染远程页面 |
| **D7** | 前端 Tauri 事件订阅统一收敛到**单一封装**，禁止各处自行 `unlisten?.()` | 根因 C 同类的 7 处竞态是同一模式复制 |
| **D8** | 移动端初始状态同时关闭侧栏**与右栏** | 窄窗口全屏遮罩遮挡主内容 |
| **D9** | `ToolchainPanel` 的 UAC 异步标记按**操作类型**区分，仅 `install git` 置 `uacPending` | 代码注释自认 `uninstall_git` 为 `-Wait` 同步，前端却按异步提示 |
| **D10** | `LogPanel` 文件切换加**单调序号守卫**，丢弃过期 `readLog` 结果 | 消除错文件内容显示 |
| **D11** | 6 个假测试**删除或改为直调生产函数**；`plugin_pipeline_test`、`mcp_pipeline_test` **纳入 CI** | 根因 C：让 CI 结论可信 |
| **D12** | `part_b_compliance_test` 中**不依赖符号链接**的静态断言拆分为独立离线测试并纳入 CI；依赖符号链接的用例改 `#[ignore]` + 运行时跳过（**不得 panic**） | 该文件当前在 CI 缺席且符号链接失败即 panic |
| **D13** | `#[ignore]` 的真实网络测试（`github_tag`、`npm_versions`、`direct_node_capture`）改由**夜间/手动 workflow** 以 `--ignored` 运行 | 三条真实链路当前零自动化验证 |
| **D14** | 重写 `docs/DESIGN.md` 模块划分对齐实际结构；修正 `ADR-0004:11` 与 `DESIGN.md:84` 的"三处同步"为**五处** | 文档与实现脱节 |
| **D15** | 从代码注释与被引用锚点**重建 `docs/adr/0006-mcp-server-management.md`** | 该文件缺失却被 6 处以上按 §/D 编号引用 |
| **D16** | `e2e-adr0006.ps1` 路径**参数化**；强杀进程改为**精确 PID 树**（`taskkill /T`），移除按启动时间扫射 | 脚本不可移植 + 误杀风险 |
| **D17** | 发布工作流的 action **pin 到不可变版本**（至少消除 `@stable`、`@v0` 两个移动标签） | 供应链风险 |
| **D18** | 纯图标按钮补 `aria-label`；`dialog.tsx` 的 sr-only 文案改中文 | 可访问性与 i18n 一致性 |
| **D19** | 清理死代码与失效引用：`setWebGuiIcon`、`AppConfig.githubToken`、`OPEN_STAGES[3]`、`.cn-toast`、`.right-panel-container … pre` 重复规则、`plugin_whitelist_test.rs` 注释引用、`verify-layout.py` 陈旧注释 | 冗余与误导 |
| **D20** | 移除 `switch.tsx`/`separator.tsx`/`sonner.tsx` 的 `"use client"` 指令（✅ 已实施）。**两处布局语义改动经可达性穷举证伪，未实施**：`clampPanelWidth` 保留原语义、`getSidebarMaxWidth`/`getRightPanelMaxWidth` 保留互相依赖（见「D20 实施收窄」） | 遗留指令属实且零风险；布局改动缺乏可达缺陷支撑，属重构而非修复 |
| **D21** | 版本节奏按 ADR-0004 十进一滚动，且**以在途批次为准**（见「D21 精确契约」）：Phase 0–3 并入在途的 `0.9.0`；D5（token 口径，用户可见行为变化）单独发 `0.9.1` | 行为变更需独立回归；不得拆散在途批次 |
| **D22** | 全流程**只读审计后的首次写盘**必须逐阶段过门禁，任一阶段门禁失败即停止后续阶段 | 避免"一次改一大片"无法归因 |

---

## 精确契约

### D1 / D2 精确契约（版本一致性与唯一入口）

1. 本次修复**不修改** `bump-version.mjs` 的一致性保护语义（`new Set(allVersions).size !== 1` 仍是硬失败），只把漂移对齐。
2. 新增 `scripts/check-version-sync.mjs`：复用与 `bump-version.mjs:35-50` 相同的五处解析（`package.json`、`package-lock.json` 根包、`Cargo.toml`、`Cargo.lock` 的 `name = "dsh-launcher"` 段、`tauri.conf.json`），五处不一致时打印差异并 `exit(1)`，一致时打印解析值并 `exit(0)`。
3. `ci.yml` 在「类型检查（tsc）」步骤之前插入 `node scripts/check-version-sync.mjs`；`release.yml` 在「校验（tsc + vite build + cargo check）」步骤内首行插入同一条命令。
4. `release.yml:75-79` 的 `git reset --hard origin/master` 步骤中，检出后须重新执行一次 `check-version-sync.mjs`（防止 reset 后版本与本地构建产物不一致）。
5. 补一条**防回归单测**：`src-tauri/tests/version_sync_test.rs`，读取仓库根五处文件并断言版本一致（与脚本互为独立实现，避免"脚本本身写错导致两侧同时放过"）。

### D3 精确契约（CHANGELOG 防重）

- 将 `release.yml:125` 的 `if ($joined -match "\[$newVersion\]")` 改为字面量判定：
  `if ($joined.Contains("[$newVersion]")) { ... exit 0 }`。
- 保留 `:164-172` 的 `awk index()` 提取逻辑不动（它已是字面匹配）。
- 验收：对已含 `## [<新版本>] - <date>` 的 CHANGELOG 重复执行该步骤，输出"CHANGELOG 已含 v<新版本>，跳过"且文件字节不变。

#### D3 实测复核（2026-09-12，**初稿判据被证伪**）

审计初稿断言"`[0.9.0]` 被当作正则字符类，永不匹配字面量"。**实测证伪**：

| 实测表达式 | 结果 | 说明 |
| --- | --- | --- |
| `"## [0.9.0] - 2026-09-11" -match "\[0.9.0\]"` | **True** | 转义后的 `\[`/`\]` 是**字面方括号**，`.` 是单字符通配 → 能匹配 7 字符 `[0.9.0]` |
| `"x[0]y" -match "\[0.9.0\]"` | False | 佐证：模式并非"匹配单个字符"的字符类 |
| `"x[9]y" -match "\[0.9.0\]"` | False | 同上 |

**结论**：原写法**不是**功能缺陷，CHANGELOG **不会**被重复插入。D3 被**降级为 P3 健壮性改进**，理由改为：版本号中的 `.` 在正则里退化为通配符，理论上 `[0x9y0]` 也会命中；`Contains` 消除该歧义。此修正已同步到 `release.yml` 的代码注释与本文决策表。

### D4 精确契约（MCP 恒假死条件清理）

`core/mcp/mod.rs:661-667` 的守卫当前为：

```
if block.declare_by_row_id(&row_id).is_none() && row.row_id != row_id {
```

`row_id` 取自 `row.row_id.clone()`（`:635`），故后半段恒 `false`，整个守卫**永不执行**。

#### D4 实测复核（2026-09-12，**影响面被证伪**）

审计初稿断言"不变量 #2 从未生效 → 对 external 行写定向覆盖会触发 dsh 启动告警"。该影响**经复核证伪**：

1. 守卫的两个条件在 `set_state` 路径上**不可能同时为真**。`row` 来自 `find_in_tree(&tree, server_name)`（`:631`），`validate_transition` 在其为 `None` 时已返回 `NotFound`（`core/mcp/state.rs:184-189`）——因此能走到 `:634` 的 `row` 必然**存在于合成树**，其 `row_id` 必然指向一条真实声明。
2. 因此"受管声明与合成树既有行**两者都没有**"这一区底情形在该路径上**不可达**；不存在"应该拒绝却放行"的场景。
3. external 行可写定向覆盖本就是**设计意图**（`remove` 的 `McpOrigin::External` 分支，`:725-752`，明确"仅撤销定向覆盖，声明仍在"）。

**结论**：D4 从"P2 逻辑缺陷"**降级为 P3 死代码清理**。处置方式随之调整为**删除该死条件**（而非"修正判据"），并在删除处留注释说明"目标行存在于合成树由 `find_in_tree` + `validate_transition` 保证"，同时补一条回归单测锁定 external 行写定向覆盖 + `NotFound` 区底两条语义。

### D5 精确契约（token 口径 = **不打码**）

1. **保留明文**：`core/process.rs` 的 `process_dsh_output_line` 继续以原文调用
   `logger.log(LogSource::Dsh, level, line)`，日志落盘与前端 `log://line` 流均含**完整**
   带 token 的访问地址。
2. **删除死代码**：移除 v0.4.13 引入、v0.5.6 后已无调用方的 `redact_web_token()` 及其单测
   （`core/logging.rs`），避免"存在打码函数"误导后续维护者以为已打码。
3. **口径集中记录**：在 `core/logging.rs` 增设「dsh web token 的日志口径」说明节，
   记录决策依据（明文换取可用性）、历史沿革（v0.4.13 打码 → v0.5.6 明文 → 本 ADR 定案）
   与安全权衡（日志位于 `LOCALAPPDATA`，仅本机用户可读写；dsh 自身 stdout 落盘文件本就不打码）。
4. **同步订正矛盾注释**：`core/logging.rs` 中"日志文件本身不涉密（token 已打码）"等
   与实现矛盾的表述一并更正。
5. **能力不退化**：`commands/dsh.rs` 的 `get_web_url` 与 `core/logging.rs` 的
   `save_latest_web_url` / `extract_latest_web_url` 行为**不变**（继续提供完整 URL）。
6. **文档一致性**：`CHANGELOG.md:541` 曾声称"dsh web token 不再明文进日志/日志流（日志打码…）"，
   与实现矛盾，须更正为明文口径 —— 否则历史条目会持续误导。
7. 验证：全仓无 `redact_web_token` 调用与单测；`cargo check --all-targets` 零警告；
   `cargo test --lib` 全绿（打码单测移除后数量相应减少，不得残留失败用例）；
   文档中不再存在"已打码"的错误声明。

### D6 精确契约（CSP）

- 目标：`tauri.conf.json:24-26` 的 `"csp": null` 替换为最小策略。
- 主窗口（本地打包产物，`frontendDist: ../dist`）：
  `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ipc: http://ipc.localhost; font-src 'self'`
- 内嵌 Web GUI 窗口（`WebviewUrl::External`，见 `commands/dsh.rs:174-182`）加载远程 dsh 页面，**不**套用主窗口 CSP；其约束由 Tauri 的 window 级 CSP 或保持宽松并**限定 `remote.urls`**（`capabilities/dsh-web-gui.json:6-13` 已限定回环）实现。
- 验收：主窗口功能无回归（日志流、进度事件、窗口控制、对话框）；`dev` 与 `build` 两种模式均验证。

### D7 精确契约（前端事件订阅统一）

- 新增 `src/hooks/useTauriEvent.ts`，导出两个原语：
  - `useTauriEvent<T>(listenFn, handler, deps)`：内部持有 `disposed` 标志，`listen()` resolve 时若已 `disposed` 立即调用返回的 `unlisten`，否则保存；`cleanup` 先置 `disposed = true` 再 `unlisten?.()`；`listen` 的 rejection 走 `console.error` 并**不吞掉**（不产生未处理拒绝）。
  - `useRefreshOnEvent(listenFn, refresh)`：上述原语的薄封装。
- 迁移全部 7 处调用点：`StatusCard.tsx:129-146`、`VersionPanel.tsx:127-141`、`ToolchainPanel.tsx:81-95,98-116`、`McpPanel.tsx:139-143`、`PluginsPanel.tsx:89-93`、`SkillsPanel.tsx:139-143`、`WindowControls.tsx:14-32`。
- `LogPanel.tsx:82-108` 已有的正确写法一并迁移到同一封装（保持全仓单一实现）。
- 验收：`tsc --noEmit` 通过；React StrictMode dev 下反复挂载/卸载组件，控制台无重复事件回调、无未处理 Promise 拒绝。

### D8 / D9 / D10 精确契约（前端行为修正）

- **D8**：`AppShell.tsx` 移动端 effect（`:159-161`）在 `isMobile` 为真时同时 `setSidebarOpen(false)` 与 `setRightOpen(false)`；初次挂载与 `isMobile` 变化均生效。
- **D9**：`ToolchainPanel.tsx` 的 `handleSyncDone(name, msg)` 增加 `action: "install" | "uninstall"` 参数；仅 `action === "install" && name === "git"` 时 `setUacPending(true)`；对应调用点 `:165-172`（install）与 `:166-177`（uninstall）分别传参。
- **D10**：`LogPanel.tsx` 引入 `useRef<number>` 序号，`selectLog` 与 `refreshFiles` 均采用"自增 → await → 仅当序号仍为最新才 setState"。

### D11 / D12 / D13 精确契约（测试体系）

**D11 逐条处置**（不得只做"改断言"的敷衍修复）：

| 测试 | 当前位置 | 处置 |
| --- | --- | --- |
| `channel_mutex_logic_test.rs:7-38` | 只测测试内 `cleanup_dir` | 删除；其意图（清理幂等）由新增的 `github_shim_cleanup_test` 直调 `core/github.rs` 的 `global_shim_path`/`installed` 判定覆盖 |
| `path_inject_test.rs:10-34` | 构造 Command 从不执行 | 改为直调 `core/pathutil.rs::inject_node_path_into`，断言 `cmd.get_envs()` 中 `PATH` 含 `node_dir` 前缀 |
| `path_inject_test.rs:37-83` | 复刻 `hidden_cmd` 注入 | 改为直调 `core/command.rs::hidden_cmd("npm")` 并断言其 env |
| `icon_window_test.rs:20-32` | 复刻像素转换 | 从 `commands/dsh.rs::ensure_big_hicon` 抽取 `rgba_to_bgra_and_mask(rgba) -> (Vec<u8>, Vec<u8>)` 纯函数，测试直调该函数 |
| `concurrency_test.rs:10-30` | 测 tokio 运行时 + 耗时断言 | 改为对 `ProcessManager::start/stop` 的 `op_lock` 互斥做断言（并发调用不产生双进程），移除挂钟阈值 |

**D12**：`part_b_compliance_test.rs` 的 `symlink_file`/`mklink` 失败由 `panic!` 改为**返回 `bool` 并由调用方 `return` 跳过**（打印 `[skip]` 原因）。

> **实施修正（2026-09-12）**：本节原计划"把不涉及符号链接的声明式断言拆到 `tests/part_b_static_assert_test.rs`"。
> 实施时发现：**panic 消除后，该文件全部 17 个用例在无特权环境下同样可跑**（失败路径自动跳过），
> 拆分的唯一动因（避免符号链接失败拖垮整个测试文件）已随之消失。故**未创建**
> `part_b_static_assert_test.rs`，静态断言与特权用例同处 `part_b_compliance_test.rs`。
> 代价：该文件同时承载两类关注点，若静态断言继续增长仍应拆分（见 Consequences）。

**D13**：新增 `.github/workflows/nightly.yml`，`schedule: cron` 触发，以 `cargo test -- --ignored` 运行 `github_tag_test`、`npm_versions_test`、`direct_node_capture_test`，失败不阻断 master（仅产出告警与日志工件）。

**CI 集成测试清单**（`ci.yml`）扩充为 **12 个文件**：`config_default_test`、`tray_icon_test`、`concurrency_test`、`icon_window_test`、`path_inject_test`、`github_channel_state_test`、`version_sync_test`、`part_b_compliance_test`、**`plugin_pipeline_test`**、**`mcp_pipeline_test`**、`skill_write_pipeline_test`、`skill_import_pipeline_test`。

### D14 / D15 精确契约（文档）

- **D14**：`docs/DESIGN.md:24-42` 的模块清单重写为与 `src-tauri/src/core/` 实际一致（含 `command/config/dshhome/events/github/logging/pathutil/port/process/profile/stream/text/toolchain/tray` + `mcp/`、`plugin/`、`skill/` 三子树），删除不存在的 `install.rs`/`mirror.rs`/`backup.rs` 与顶层 `logging.rs`/`events.rs` 表述；`ADR-0004:11` 与 `DESIGN.md:84` 的"三处同步"改为"五处同步（含 `package-lock.json`、`Cargo.lock`）"。
- **D15**：重建 `docs/adr/0006-mcp-server-management.md`，作为**权威文本**收录代码中已固化的决策，至少覆盖以下被引用锚点，且每个锚点须能在代码中找到对应实现：
  - `§State Machine 2`（五动作语义：`list`/`add`/`enable`/`disable`/`remove`）→ `core/mcp/mod.rs:13-21,484-768`
  - `§State Machine 3`（前置缺失只影响 `add`）→ `core/mcp/mod.rs:197-218,513-522`
  - `§API 2`（`--raw-config` 与结构化字段互斥；`OpResult` 形状）→ `core/mcp/mod.rs:93-122,493-511`
  - `D4`（`enable`/`disable` 绝不重渲染 `config`）→ `core/mcp/entry.rs:12-15`、`core/mcp/block.rs:374-427`
  - `D7`（`remove` 两种后果必须区分）→ `core/mcp/mod.rs:688-754`
  - `D9`（只实现官方两条校验）→ `core/mcp/validate.rs:1-14`
  - `D10`（`failOnStartupError` 不暴露 / 只对字面量 `true` 打危险标）→ `core/mcp/dump.rs:243-249`、`core/mcp/mod.rs:475`
  - `D12`（写后校验口径）→ `core/mcp/mod.rs:303-405`
  - `D17`（`agentsHome` 锚点）→ `core/dshhome.rs:89-105`、`core/skill/sharing.rs:11-14`
  - `D18`（回滚白名单唯一例外）→ `core/plugin/mod.rs:575-604`
  - `P6`（备份子路径隔离）→ `core/profile.rs:235-290`
  - `R13`（官方扫描根常量集中）→ `core/dshhome.rs:11-19`
  - `§Testing 3.2/3.3`（离线集成测试与可观测性边界）→ `tests/mcp_pipeline_test.rs`、`tests/part_b_compliance_test.rs`
- 同时把 `core/plugin/mod.rs:586` 对不存在的 `tests/plugin_whitelist_test.rs` 的引用改为 `tests/part_b_compliance_test.rs`（D12 的实施结论为**不拆分**，故不指向 `part_b_static_assert_test.rs`）。✅ 已实施。

### D16 精确契约（e2e 脚本）

- `scripts/e2e-adr0006.ps1:11-13,307` 的本机绝对路径改为 `param()` 入参 + 环境变量默认值；无默认值时给出明确用法说明并 `exit 1`。
- `:239` 的"按启动时间批量强杀 node"改为：启动时记录 `$proc.Id`，结束时对该 PID 执行 `taskkill /PID <id> /T /F`；不得再按时间窗口扫射。
- 脚本头部注释标注"需显式提供 `-Launcher` / `-RealDsh` / `-DshSrc` 参数"。

### D17 / D18 / D19 / D20 精确契约（供应链、可访问性、死代码）

- **D17**：`release.yml:44,49,60,212` 与 `ci.yml:25,28,34,39` 的 action 引用改为不可变版本；`dtolnay/rust-toolchain@stable`（`release.yml:55`、`ci.yml:34`）改为具体版本 tag；`tauri-apps/tauri-action@v0`（`release.yml:212`）改为具体 release tag。
- **D18**：`WindowControls.tsx:43-66`（最小化/最大化/关闭）、`AppShell.tsx:256-282`（侧栏切换）、`VersionPanel.tsx:248-256`（日志收起）、`ToolchainPanel.tsx:226`（刷新）补 `aria-label`；`dialog.tsx:86,143` 的 `Close` 改「关闭」。
- **D19 清单**（逐项删除，不留注释残骸）：`src/lib/tauri.ts:69-71` `setWebGuiIcon` 与 `lib.rs:314` 的 `set_web_gui_icon` 命令注册一并移除；`src/lib/tauri.ts:171-172` `AppConfig.githubToken`；`StatusCard.tsx:61-67,341,371` 的 `OPEN_STAGES[3]` 与跳变（改为顺序推进或删除该阶段）；`ui/sonner.tsx:39` 的 `toast: "cn-toast"`；`index.css:293-307` 重复规则保留后者；`scripts/verify-layout.py:54,89` 陈旧注释。
- **D20**：`ui/switch.tsx:1`、`ui/separator.tsx:1`、`ui/sonner.tsx:1` 移除 `"use client"`；`src/lib/panel-layout.ts` 改为单向依赖（先定右栏宽上限、再由侧栏读取），并在 `useResizablePanel` 拖拽结束时触发一次重算。

#### D20 实施收窄（2026-09-12 复核后确认）

D20 含**两**部分，实测结论不同，必须分开记录：

| 部分 | 原判 | 实测结论 | 处置 |
| --- | --- | --- | --- |
| `clampPanelWidth` 的"上限被下限放弃" | 会在 960~1024px 溢出 | **证伪**：可达状态穷举（138,240 组）无溢出；且"上限优先"改动会把侧栏压到 120、违反自身声明的 min=200 | **回退原语义**，仅在注释中记录复核结论 |
| `getSidebarMaxWidth` / `getRightPanelMaxWidth` 互相依赖 | 拖拽后对侧不重算可能溢出 | **证伪**：可达状态穷举（46,080 组，含"单侧拖到最大且仅重算该侧"场景）**无溢出**；两侧 max 互相约束使系统天然收敛 | **未实施**（缺乏可达缺陷支撑；改动属重构，收益未证实而风险实在） |
| `"use client"` 指令清理 | 遗留指令 | 属实（Next.js 指令在 Vite 下无意义） | ✅ 已实施 |

**因此 D20 仅实施了 `"use client"` 清理部分**；两处"布局语义改动"均因**可达性证伪**而未实施 ——
这是**有意的收窄**，不是遗漏。若未来出现真实的可达溢出，应连同 AppShell 的联动 reclamp
一并重新设计（当前 AppShell 仅在 `rightOpen`/`sidebarOpen` 变化时 reclamp 对侧，
拖拽本侧不触发对侧重算，但既有设计已被证明足以收敛）。

---

## 执行阶段与门禁

| 阶段 | 内容 | 门禁（全部通过才进入下一阶段） |
| --- | --- | --- |
| **Phase 0 — 发版解阻** ✅ **已完成（2026-09-12）** | D1、D2、D3 | ✅ `node scripts/check-version-sync.mjs` 退出码 0；✅ release 决策步骤干跑通过（见「Phase 0 实施记录」） |
| **Phase 1 — 后端正确性与安全口径** ✅ **已完成（2026-09-12）** | D4、D6、D16、D17 | ✅ `cargo check --all-targets` 零警告；✅ `cargo test --lib` 与 `mcp_pipeline_test` 全绿；✅ CSP 实测 8/8；✅ action 全 pin SHA（见「Phase 1 实施记录」）。**注**：D4 已降级为死代码清理，原「三分支单测」不再适用（其两条语义已有既有断言覆盖） |
| **Phase 2 — 前端收敛** ✅ **已完成（2026-09-12）** | D7、D8、D9、D10、D18、D19、D20 | ✅ `tsc --noEmit` 与 `tsc -p tsconfig.node.json --noEmit` 均 0 错误；✅ `npm run build` 成功；✅ `verify-layout.py` **33/33**（含新增 D8 断言）；✅ CSP 8/8（未回归）；✅ D19 含 `plugin_whitelist_test.rs` 失效引用订正。**注**：D20 经可达性穷举**证伪两处布局语义改动**，仅实施 `"use client"` 清理（见「D20 实施收窄」） |
| **Phase 3 — 测试与文档** ✅ **已完成（2026-09-12）** | D11、D12、D13、D14、**D15 ✅** | ✅ CI 集成清单扩为 **12 文件 / 63 用例全绿**；✅ `part_b_compliance_test` 符号链接不可用时**跳过**（有契约测试）；✅ nightly `--ignored` 选中 5 个真实测试；✅ DESIGN.md 与 49 个 `core` 文件 **100% 对照**、ADR-0004 三处→五处（见「Phase 3 实施记录」） |
| **Phase 4 — 行为口径定案** ✅ **已完成（2026-09-12）** | D5 | ✅ 口径定为**不打码**（产品所有者裁定）；✅ `redact_web_token` 及单测已删（全仓零调用）；✅ `cargo check --all-targets` 零警告、`cargo test --lib` 193 passed；✅ CHANGELOG 矛盾声明已订正（见「Phase 4 实施记录」） |

**发布**：Phase 0–3 **并入在途的 `v0.9.0`**（机械修复与一致性）；Phase 4 单独 `v0.9.1`（用户可见行为变更，需独立评审与回归）。版本递增一律经 `bump-version.mjs`，并按 D1 的 CI 不变量校验。

### D21 精确契约（版本节奏以在途批次为准）

Phase 0 实施前的现场取证（2026-09-12）改变了本文初稿的发布号判断，此处如实记录并据此修正：

| 取证项 | 实测值 | 含义 |
| --- | --- | --- |
| `git log -1` | `02ebf6a v0.7.0` | HEAD 提交仍是 `0.7.0` |
| `git show HEAD:package.json` 的 `version` | `0.7.0` | 在途改动尚未提交 |
| `git tag -l` / `git ls-remote --tags` 最高 tag | `v0.7.0` | **`v0.9.0` 不存在** |
| 工作树 `package.json:4` | `0.9.0` | `0.9.0` 是**在途未发布版本** |
| `CHANGELOG.md:3` | `## [0.9.0] - 2026-09-11` | 该版本条目已写好，等待发布 |
| 工作树五处版本 | 仅 `package-lock.json` 为 `0.7.1`，其余四处 `0.9.0` | 漂移是"在途批次内漏跑版本同步"的产物 |

**结论**：Phase 0 的修复（对齐 `package-lock.json` 至 `0.9.0`）属**该在途批次自身的完整性修复**，必须并入 `0.9.0` 一起提交与发布。若改为新开 `0.9.1`，则 `0.9.0` 永不发版，`CHANGELOG.md:3` 的条目与 `v0.9.0` tag 双双成为孤儿，且发布工作流会因"当前版本已发布/未发布"判定逻辑（`release.yml:82-93`）产生一次多余的版本跳跃。

**因此**：Phase 0–3 全部完成后**一次性**提交并发布 `0.9.0`；D5 在其后单独发布 `0.9.1`。Phase 0 阶段**不执行** `bump-version.mjs`（版本号保持 `0.9.0`），只做漂移对齐与护栏补齐。

---

## Phase 0 实施记录（2026-09-12，已完成并验证）

### 改动清单

| # | 文件 | 改动 | 对应决策 |
| --- | --- | --- | --- |
| 1 | `package-lock.json:3,9` | `0.7.1` → `0.9.0`（**仅这 2 行**；`:2749` 的 `class-variance-authority@0.7.1` 是真实依赖版本，未触碰） | D1 |
| 2 | `scripts/check-version-sync.mjs` | **新增**：五文件六落点一致性门禁，不一致即 `exit(1)` | D1/D2 |
| 3 | `.github/workflows/ci.yml` | 在「类型检查（tsc）」前新增「版本一致性门禁」步骤 | D1 |
| 4 | `.github/workflows/release.yml` | ① 校验步骤首行接入门禁；② `git reset --hard` 后**复检**一次；③ CHANGELOG 防重改 `Contains` 并更正注释口径 | D1/D3 |
| 5 | `src-tauri/tests/version_sync_test.rs` | **新增**：与脚本**互为独立实现**的防回归单测（脚本用正则，测试用逐行/JSON 解析） | D1 |

### 验证证据（均为真实运行结果，非静态推断）

| 验证项 | 命令 / 方法 | 结果 |
| --- | --- | --- |
| 漂移已消除 | `node scripts/check-version-sync.mjs` | 六落点全部 `0.9.0`，`exit=0` |
| 门禁**能捕获**漂移 | 注入 `tauri.conf.json=0.0.0` 后运行 | 打印"实际出现 2 个不同版本: 0.9.0, 0.0.0"，`exit=1` |
| 注入后字节级还原 | SHA256 前后比对 | `A4468F73…D4F4` 一致，`restored byte-exact = True` |
| 单测**能捕获**漂移 | 注入 `package.json=0.9.9` 后 `cargo test --test version_sync_test` | `test result: FAILED`，`exit=101` |
| 单测在对齐态通过 | 还原后重跑 | `test result: ok. 1 passed`，`exit=0` |
| 工作流 YAML 合法 | `python -c "yaml.safe_load(...)"` | 两文件均 `OK`，`jobs=['validate']` / `['release']` |
| 锁文件 diff 最小化 | `git diff --stat -- package-lock.json` | `1 file changed, 2 insertions(+), 2 deletions(-)` |

### 实施中发现的**两处审计初稿错误**（已更正，见各条「实测复核」）

1. **D3 前提证伪**：初稿称 CHANGELOG 正则"永不命中"，实测 `-match "\[0.9.0\]"` 返回 **True**。已降级为 P3 健壮性改进，并修正 `release.yml` 中我最初写入的错误注释（该注释一度复述了假结论）。
2. **D4 影响面证伪**：初稿称"不变量 #2 从未生效 → 触发 dsh 启动告警"，复核确认该守卫**恒不可达且其守护的不变量由上游 `find_in_tree` 保证**，external 行写定向覆盖本就是设计意图。已降级为 P3 死代码清理。

> **流程反思**：这两条错误同源于"采信静态推断而未运行验证"。Phase 0 的其余结论（P1-1 漂移、P2-8 移动端右栏、P2-9 git UAC 误标、P2-14~17 假测试、P2-18 CI 缺口、P2-20 脚本硬编码、P2-21 DESIGN 过期、P2-22 ADR-0006 缺失、P3-5/6/8/9 死代码）已在复核中逐条回读源码确认属实。**后续 Phase 一律先复现再修**。

---

## D15 实施记录（2026-09-12，已完成并验证）

`docs/adr/0006-mcp-server-management.md` 已**重建**（465 行）。重建以 `CHANGELOG.md:238-311` 的 0.7.0
施工记录为第一事实源，以 `CONTEXT.md:68-80` 词表与代码注释为交叉印证，**按仓库既有 ADR 体例**
（状态/日期/范围/事实依据/重建说明 → Context → Decision 表 + 精确契约 → Architecture →
Consequences → Testing → 施工任务台账 → References）撰写。

**锚点闭合验证**（脚本化，非人工目测）：

| 验证项 | 方法 | 结果 |
| --- | --- | --- |
| 全仓引用的 ADR-0006 锚点清单 | 正则扫描全部手写文件（排除 `node_modules/target/dist`） | 去重得 **19 项** |
| 逐锚点在新文档中可检索 | 对 19 项逐个 `-match` | ✅ **19/19 覆盖** |
| 文档引用的具体代码路径是否存在 | 提取反引号路径 + `Test-Path` | ✅ 全部存在（修正 3 处 `tests/` → `src-tauri/tests/` 后） |
| 关键行号抽样核对 | 抽 6 处 ADR 引用的行号回读源码 | ✅ 6/6 命中（`state.rs` 的 `NotFound` 位于 185 行，区间 `173-219` 引用无误） |

**覆盖的 19 个锚点**：`§Context 1`、`§State Machine 2`、`§State Machine 3`、`§API 2`、
`§Testing 3.2`、`§Testing 3.3`、`§Testing 5`、`D4`、`D9`、`D12`、`D14`、`D17`、`D18`、
`P6`、`R13`、`A1`、`Part A`、`T6`、`T8`。

> **首次验证即暴露缺口**：初次核验时 `§API 2`、`§State Machine 2/3`、`T6`、`T8` **5 项未被
> 正文以可检索形式覆盖**（仅在别处顺带提及）。已补写三个独立契约小节（`§API 2` 对外形状、
> `§State Machine 2` 五动作转换表 + 四条不变量、`§State Machine 3` 前置/NotFound/幂等边界）
> 与「施工任务台账（T1–T8）」，二次核验方达 19/19。**该缺口若靠人工目测必然漏过** ——
> 印证了 D15 验收标准须为脚本化断言。

---

## Phase 1 实施记录（2026-09-12，已完成并验证）

### D4 — 删除 MCP 恒假死条件

**改动**：`core/mcp/mod.rs` 删除 `if block.declare_by_row_id(&row_id).is_none() && row.row_id != row_id { ... }`
整段守卫，替换为说明性注释（明确「目标行存在于合成树由 `find_in_tree` + `validate_transition` 保证」）。
全仓 grep 确认错误文案「无法为行 … 受管区块无该声明」已无任何引用残留。

**为何不新增测试**：该守卫守护的两条语义**均已有既有断言**——
`NotFound`(3) 由 `mcp_pipeline_test.rs:634-651` 覆盖（`remove`/`enable`/`disable` 不存在的 serverName），
external 行写定向覆盖由 `:448-475` 覆盖（`remove_external_only_drops_directive_and_message_says_declaration_remains`
断言 `origin == External` 且撤销后恢复默认启用）。新增断言只会重复既有覆盖。

**验证**：`cargo test --test mcp_pipeline_test` → **16 passed / 0 failed**，`cargo check` 无新增警告。

### D6 — 最小 CSP（含 devCsp）

**改动**：`tauri.conf.json` 的 `security` 从 `"csp": null` 改为显式 `csp` + `devCsp`：

| 模式 | 策略 |
| --- | --- |
| `csp`（生产） | `default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self' ipc: http://ipc.localhost; object-src 'none'; base-uri 'self'; frame-ancestors 'none'` |
| `devCsp`（开发） | 同上，另放行 `'unsafe-inline' 'unsafe-eval'`（script）、`data:`（font）与 `http://localhost:1420` / `ws://localhost:1421`（Vite HMR） |

**每条策略的取证依据**（均来自源码/产物实测，非套用模板）：

| 决策 | 依据 |
| --- | --- |
| `connect-src ipc: http://ipc.localhost` | Tauri 官方文档示例即此写法（`tauri-utils-2.9.3/src/config.rs:2741`）；前端 47 处 IPC 全走 `invoke`，无 `fetch`/`WebSocket`/`EventSource` |
| `script-src 'self'`（生产不加 `unsafe-inline`） | 产物 `dist/index.html` 内联 `<script>` 数为 **0**；且 Tauri 在无内联脚本时**不注入** `script-src` hash（`tauri-2.11.5/src/manager/mod.rs:140` 的 `nonces.is_empty() && hashes.is_empty()` 判据） |
| **`style-src 'unsafe-inline'`（必需）** | 产物中确有**运行时注入 `<style>` 文本节点**的代码（sonner 的 `gN(l)` → `createElement("style")` + `createTextNode`）；若不给 `'unsafe-inline'` 会静默丢 toast 样式 |
| `img-src 'self' data:` | 产物 CSS 含 1 个 `url(data:image/svg+xml;...)`；`dist` 另有本地 `favicon.png`。无 `convertFileSrc` 调用，故**不需要** `asset:` 协议 |
| `font-src 'self'` | 5 个 `@font-face` 全部指向 `/assets/*.woff2`（本地），无外部字体源、无 `url(data:)` 字体 |
| 无需 `unsafe-eval` / `worker-src` / `blob:` | 产物实测 `eval(`=0、`new Function(`=0、`new Worker(`=0、`blob:`=0、`WebAssembly`=0 |
| **必须同时配 `devCsp`** | `tauri-2.11.5/src/manager/mod.rs:369-381`：dev 下取 `dev_csp`，**若为空则回退 `csp`** → 只配 `csp` 会让 `tauri dev` 因 HMR 被拦而坏 |
| 不影响内嵌远程窗口 | CSP 经 `html2::inject_csp` 注入**本地 HTML 文档**（`manager/webview.rs:485-493`，且仅在 `data:` 分支）；内嵌 dsh Web GUI 是 `WebviewUrl::External` 的远程页面，其约束由 `capabilities/dsh-web-gui.json` 的回环 URL 白名单负责 |

**验证（新增脚本化门禁 `scripts/verify-csp.py`，可重复执行）**：
从 `tauri.conf.json` 读取**实际配置**的 CSP → 以 HTTP 头下发到 `dist/` 产物 → 真实 Chromium 加载 →
监听 `securitypolicyviolation` 事件。**8/8 通过**：零 CSP 违规、页面渲染（bodyLen=47682）、
样式表挂载（61216 B）、5 个 `@font-face` 注册、woff2 实际加载、无非预期请求失败。

**反向验证**（证明该门禁有鉴别力）：把 `font-src 'self'` 篡改为 `font-src 'none'` →
**4 项 FAIL、`exit=1`**，且违规事件精确报出 `directive: font-src` 与被拦的 woff2 URL；
还原后 8/8、`exit=0`，注入前后 **SHA256 一致**（字节级还原）。

### D16 — e2e 脚本参数化与精确终止

**改动**（`scripts/e2e-adr0006.ps1`）：

1. 三个本机绝对路径改为 `param()` 入参 + 环境变量默认值（`-Launcher`/`-RealDsh`/`-DshSrc`
   ↔ `DSH_E2E_LAUNCHER`/`DSH_E2E_REAL_DSH`/`DSH_E2E_DSH_SRC`），`-Launcher` 默认推导为仓库内
   `src-tauri\target\debug\dsh-launcher.exe`。
2. **每个入参均有存在性校验**：缺失/不存在即打印可操作的用法示例并 `exit 1`（不静默用错路径）。
3. 真实 home 断言不再硬编码 `C:\Users\Administrator\.dsh`，改为由 `$env:USERPROFILE` 推导；
   文件不存在时**如实标注**为「无可污染的既有文件」而非静默跳过。
4. **终止逻辑**：删除「`Get-Process node | StartTime > now-30s`」时间窗口扫射，改为对本脚本刚创建的
   PID 执行 `taskkill /PID <id> /T /F`；兜底只按 `ParentProcessId` 回溯到该 PID，绝不按时间扫射。

**验证**：PowerShell 语法解析零错误；grep 确认无硬编码本机路径、无 `StartTime -gt` 残留；
`taskkill /PID` 与 `ParentProcessId` 就位；清空环境变量干跑 → 正确输出用法提示并 **`exit=1`**。

### D17 — action pin 到不可变 SHA

**改动**：`ci.yml` 4 处、`release.yml` 5 处 `uses` 全部 pin 到 40 位 commit SHA，并保留行尾
注释标注原 tag 以便升级：

| Action | 原引用 | pin 后 SHA | 核验日期 |
| --- | --- | --- | --- |
| `actions/checkout` | `@v4` | `11d5960a326750d5838078e36cf38b85af677262` | 2026-07-16 |
| `actions/setup-node` | `@v4` | `49933ea5288caeca8642d1e84afbd3f7d6820020` | 2025-04-02 |
| `dtolnay/rust-toolchain` | `@stable`（**分支**） | `d1031067263f94b142dd6c0ce24c5eb9d02d52a0`（`master`） | 2026-09-03 |
| `Swatinem/rust-cache` | `@v2` | `6323deb102c322ba6fcbdcafc7e3dddab59af2b6` | 2026-08-06 |
| `tauri-apps/tauri-action` | `@v0` | `84b9d35b5fc46c1e45415bdb6144030364f7ebc5` | 2026-03-14 |

**SHA 双源核验**：先用 `git ls-remote --tags/--heads` 解析，再用已认证的 GitHub API
（`/repos/{owner}/{repo}/commits/{ref}`）独立复核，两法结果**完全一致**后才写入
（初次尝试未认证 API 遇 403 限流，**未凭推测填值**）。

**关键陷阱（已规避）**：`dtolnay/rust-toolchain` 的工具链由 `@rev` 推断（官方 README 明示），
pin 成 SHA 后**必须显式传 `toolchain:`**，否则无法推断要安装哪个工具链。故两处均补
`toolchain: stable`，并在注释中记录该约束。

**验证**：`yaml.safe_load` 两文件均合法；脚本统计 **9/9 处 `uses` 均为 40 位 SHA**（`exit=0`）；
grep 确认无裸移动标签（`@stable`/`@v0`/`@v2`/`@v4`）残留；`toolchain: stable` 两处均在位。

---

## 文件影响清单

**新增**
- `scripts/check-version-sync.mjs`
- `scripts/verify-csp.py`
- `.github/workflows/nightly.yml`
- `src/hooks/useTauriEvent.ts`
- `src-tauri/tests/version_sync_test.rs`
- `src-tauri/tests/github_channel_state_test.rs`（承接 `channel_mutex_logic_test` 的原意：直调生产标记判定）
- `docs/adr/0006-mcp-server-management.md`（重建）
- `.github/workflows/nightly.yml`
- `docs/adr/0006-mcp-server-management.md`（**重建已完成**，2026-09-12）
- `docs/adr/0009-audit-remediation-plan.md`（本文）

**修改（后端）**
- `src-tauri/src/core/mcp/mod.rs`（D4）
- `src-tauri/src/core/process.rs`、`src-tauri/src/core/logging.rs`（D5）
- `src-tauri/src/commands/dsh.rs`（D5、D19 移除 `set_web_gui_icon`）
- `src-tauri/src/lib.rs`（D19 移除命令注册）
- `src-tauri/tauri.conf.json`（D6；版本号随发布递增）
- `src-tauri/tests/{channel_mutex_logic_test,path_inject_test,icon_window_test,concurrency_test,part_b_compliance_test}.rs`（D11、D12）
- `src-tauri/tests/fixtures-real-dump.txt`（仅当 D15 重建需要引用行号时同步）

**修改（前端）**
- `src/hooks/useTauriEvent.ts`（新增）、`src/components/{StatusCard,VersionPanel,ToolchainPanel,McpPanel,PluginsPanel,SkillsPanel,WindowControls,LogPanel,AppShell}.tsx`
- `src/components/ui/{dialog,sonner,switch,separator}.tsx`
- `src/lib/{tauri,panel-layout}.ts`、`src/hooks/useResizablePanel.ts`、`src/index.css`

**修改（工程与文档）**
- `.github/workflows/{ci,release}.yml`
- `scripts/{bump-version.mjs（仅注释澄清）,e2e-adr0006.ps1,verify-layout.py}`
- `docs/DESIGN.md`、`docs/adr/0004-version-numbering-scheme.md`
- `package.json`、`package-lock.json`、`src-tauri/Cargo.toml`、`src-tauri/Cargo.lock`、`CHANGELOG.md`（随发布递增）

---

## 验收标准

1. **可定位**：每条修复都能追溯到本文某个 `D` 编号与一个 `文件:行号`。
2. **可复现**：每条修复的失败场景都有明确复现步骤（例：D1 复现 = 删 tag 后跑 release 决策步骤；D4 复现 = 对无声明 rowId 写定向覆盖）。
3. **可验证**：每条修复都有自动化断言（单测 / CI 步骤 / 脚本退出码），不依赖人工目测。
4. **无回归**：`cargo check --all-targets` 零警告、`cargo test` 全绿、`tsc --noEmit` 零错误、`npm run build` 成功、`verify-layout.py` 通过。
5. **不变量固化**：版本五处一致（`check-version-sync.mjs` + `version_sync_test.rs`）、
   `part_b_compliance_test.rs` 的 profile 写入白名单断言，均成为 CI 阻断项而非人工约定。
6. **边界守恒**：未修改 `deepseek-harness`；未新增依赖；未放宽 capability。

---

## Consequences

### 正面

1. 发布流水线从"下次 push 必失败"恢复为可用，且漂移被 CI 永久看住（D1–D3）。
2. 消除两处"决策变更后未回收旧实现"的残留，使注释与实现重新自洽（D4、D5）。
3. 测试从"绿但无保护"变为"断言触达生产代码"；两个真实离线链路与静态合规断言进入 CI（D11、D12）。
4. 文档恢复可追溯性：DESIGN 与实际结构一致，ADR-0006 的 12 个被引用锚点全部有权威文本（D14、D15）。
5. 前端 7 处同类竞态收敛为单一封装，后续新增面板不再复制该缺陷（D7）。

### 负面 / 代价

1. **D5 保留明文 token 是「已知并接受」的安全权衡**：日志与前端日志流中的 dsh 访问地址含完整
   token（换取"用户可从日志复制地址在外部浏览器打开"的可用性）。缓解事实：日志位于
   `%LOCALAPPDATA%`（仅本机用户可读写）、dsh 自身 stdout 落盘文件本就不打码；**不缓解**：
   同机其它进程/备份/误发的日志文件会连带泄漏 token。若未来要收紧，应改回打码并同时提供
   「复制访问地址」UI 入口（D5 的初版方案），届时需重新评估可用性影响。
2. **D6 的 CSP 有实际调试成本**：CSP 命中误配时表现为功能静默失效（如 `style-src` 挡住内联样式），需 `dev` 与 `build` 双模式验证。
3. **D11 会删除既有测试**，短期降低测试数量指标；换取的是"每个测试都触达生产代码"。
4. **D12 未拆分测试文件**（原计划拆分，实施时发现 panic 消除后拆分的动因已消失）：`part_b_compliance_test.rs` 的 17 个用例在无特权环境同样可跑（失败路径自动跳过）。代价是该文件同时承载"静态断言"与"需特权用例"两类关注点，后续若前者继续增长，仍应拆分。
5. **D17 的 pin 会带来升级摩擦**：每次升级 action 需人工改 SHA/tag，失去 `@stable` 的自动跟进。
6. 本计划**不覆盖**审计中列为"未覆盖区域"的部分（`frontmatter.rs` 逐行、Tailwind v4 编译产物、`@base-ui/react` 运行时行为），这些需在 Phase 2 的门禁中以构建与人工验证补足，而非本计划的自动断言。

---

## Phase 2 实施记录（2026-09-12，已完成并验证）

### D7 — 事件订阅统一收敛

**改动**：新增 `src/hooks/useTauriEvent.ts`，提供**两个**原语并把 9 个订阅点全部迁移：

| 原语 | 适用 | 迁移点 |
| --- | --- | --- |
| `useTauriEvent<T>(subscribe, onPayload, deps)` | 事件**带载荷** | `LogPanel`（`log://line`）、`ToolchainPanel`（`install://progress`）、`VersionPanel`（`install://progress`）、`WindowControls`（`onResized`） |
| `useRefreshOnEvent(subscribe, onEvent, deps)` | 事件**无载荷**（Rust emit `()`） | `StatusCard`、`VersionPanel`、`McpPanel`、`PluginsPanel`、`SkillsPanel`、`ToolchainPanel` 的 `*://changed` |

**为何拆两个而非一个宽松签名**：`listenXxxChanged(onChanged: () => void)` 与 `listenProgress(onProgress: (p) => void)` 的**参数类型本就不同**。首版合并成一个签名后 `tsc` 立刻报 7 处逆变错误（`Target signature provides too few arguments`），说明强行统一只能靠 `any` 退化 — 故按载荷有无拆成两个精确签名。

**竞态修法**（沿用原本唯一写对的 `LogPanel` 模式）：`disposed` 标志 + resolve 后若已卸载则立即解绑；rejection 经 `console.error` 记录（不产生未处理拒绝）；回调存 ref 以免调用方内联函数导致反复重订阅。

**验证**：`tsc --noEmit` 0 错误；grep 确认全仓**无** `let unlisten` / `unlisten?.()` / `.then((fn) =>` 残留（仅注释中提及原模式）。

### D8 — 移动端首屏右栏遮挡（**已双向验证**）

**改动**：`AppShell.tsx` 的移动端 effect 由「只收侧栏」改为「同时收侧栏与右栏」。

**缺陷确证**（`scripts/.repro-d8.py` 直读 DOM，旧行为）：以 600px **首次开页**时
`right-panel-container right-panel-open`、`x=0 w=600 h=850`、`z-index=270`、
`right-panel-overlay-backdrop.is-open = True` → **日志面板确实全屏盖住主内容**。
修复后同一测量为 `right-panel-closed`、`w=1`、遮罩关闭。

**我自己的断言曾无鉴别力（重要教训）**：首版 D8 断言在 `verify-layout.py` 里复用**同一页面**
「先 800px 再缩到 600px」测量 —— 而缩小过程会**途经 641~959px 档位的 effect**（它本就会关掉右栏），
于是旧代码下断言仍 PASS。**反向验证暴露了这一点**（回退旧行为后断言不 FAIL），
遂改为**新开一个 600px 页面**测量首屏状态，再验证：

| 验证 | 结果 |
| --- | --- |
| 旧行为（移动端只收侧栏） | **2 项 FAIL**、`exit=1`，精确定位 `class=…right-panel-open x=0 w=600 backdrop=True` |
| 修复后 | **33/33 通过**、`exit=0` |
| 注入前后 | SHA256 一致（字节级还原） |

### D9 / D10 — 行为修正（均已复现）

- **D9**：`handleSyncDone` 增加 `operation: "install" | "uninstall"` 参数，仅 **git 安装** 置 `uacPending`
  （原判据只看 `name === "git"`，使 `-Wait` 同步的 **git 卸载**完成后仍提示「请在 UAC 弹窗确认后重新检测」）。
- **D10**：`LogPanel` 新增 `readSeqRef` 单调序号守卫，`selectLog` 与 `refreshFiles` 的自动选中
  共用 `loadLogContent`，过期结果不落 state。
  **乱序复现实验**（可控延迟：先选慢的 A、再选快的 B）：旧实现最终显示 `CONTENT-OF-A.log`
  （**标题是 B、正文是 A**），加守卫后正确保留 `CONTENT-OF-B.log`。

### D18 / D19 — 可访问性、死代码、遗留

- **D18**：10 处纯图标按钮补 `aria-label`（窗口控制 3 处、侧栏/日志切换、工具链刷新、版本刷新、
  日志清屏/导出、Dialog 关闭）；`dialog.tsx` 两处英文 `Close` 改「关闭」。复查脚本确认 0 处遗漏。
- **D19**：删除死代码 —— 前端 `setWebGuiIcon` + Rust `set_web_gui_icon` 命令及其 `generate_handler!`
  注册（取证：全仓仅定义处，无调用方）；`AppConfig.githubToken`（Rust 恒回空、前端无消费）；
  `sonner.tsx` 的 `cn-toast`（`index.css` 无此定义）；`index.css` 重复的 `scroll-area pre` 规则
  （后者 0.6rem 覆盖前者 0.75rem，前者为死规则）；`verify-layout.py` 三处陈旧注释（260→340、260->320→340->400、640->560→340->280）。
  **修正一处原判**：`OPEN_STAGES` 数组本身**在用**（`length-1` 参与进度计算），并非整体死代码；
  实际缺陷是**阶段 3 从不被赋值**（`setStageIdx(2)` 直跳 `4`）。故**补上阶段推进**
  （地址获取前置 `setStageIdx(3)`、开窗前 `setStageIdx(4)`），而非删除数组。
- **图标语义**：`WindowControls` 的「还原」原借用 lucide `Copy`（复制），改为 `Minus`+`Square`
  组合示意双窗叠加，并移除不再使用的 `Copy` 导入。

### D20 — clamp 语义：经**穷举证伪后回退**（未产生行为变更）

**原判（P3-15）**：`clampPanelWidth` 的 `effectiveMax = Math.max(minWidth, maxWidth)` 在
「可用上限 < 设计下限」时会放弃上限，导致「侧栏 + 主内容最小宽 + 右栏」超视口。

**我曾据一组手挑数字「复现」溢出**（960px 视口、侧栏 480 + 右栏展开 → 溢出 220px），
并据此改成「上限优先」。**但可达性探索推翻了它**：

对**可达状态空间**穷举（视口 641–1920 × 两栏 open/closed × 初始宽度取默认/最小/最大 ×
不动点迭代 + 单次遍历两种顺序，共 **138,240** 组组合）后，**新旧语义均无溢出**。
原因是两侧 max 计算**互相约束**（右栏展开时侧栏上限仅 200，反之亦然），系统总能收敛到不溢出的解；
我手挑的「侧栏 480 且右栏展开」在 960px 下**不可达**。

**且新语义有实质副作用**：会把侧栏压到 **120**，低于其自身声明的 `SIDEBAR_MIN_WIDTH = 200`。

**结论**：`clampPanelWidth` **回退为原语义**（并在注释中记录该复核结论）；
D20 仅保留**确证可达**的部分 —— `panel-layout.ts` 注释订正与 `verify-layout.py` 陈旧注释更新。
本次**未对布局行为做未经证实的变更**。

### Phase 2 门禁结果

| 门禁 | 结果 |
| --- | --- |
| `npx tsc --noEmit` | ✅ 0 错误 |
| `npx tsc -p tsconfig.node.json --noEmit` | ✅ 0 错误 |
| `npm run build` | ✅ 成功（2088 modules，JS 449.84 kB / CSS 61.38 kB） |
| `scripts/verify-layout.py` | ✅ **33/33**（含新增 D8 两项；反向验证可 FAIL） |
| `scripts/verify-csp.py` | ✅ 8/8（Phase 1 成果未回归） |
| `scripts/check-version-sync.mjs` | ✅ 版本一致 `0.9.0` |

---

## Phase 3 实施记录（2026-09-12，已完成并验证）

### D11 — 假测试整治（逐项复现"假"的证据后重写）

| 原测试 | "假"的证据 | 处置 | 验证 |
| --- | --- | --- | --- |
| `channel_mutex_logic_test.rs` | 全文只在测试内定义 `cleanup_dir()` 并断言它自己；**零生产调用** | 删除，新增 `github_channel_state_test.rs`：直调生产 `github_installed()` / `github_clone_dir()`，验证「目录特征 ↔ 已安装」判定可逆且幂等 | 2 passed；**反向验证**：破坏生产 `package.json` 判据 → `FAILED`/101 |
| `path_inject_test.rs` | 用例 1 构造 `Command` 后**从不执行**、只断言自拼接的字符串；用例 2 手工复刻 `hidden_cmd` | 重写为直调生产 `inject_node_path_into` / `hidden_cmd` / `hidden`，断言真实写入 `Command` 的 env 与参数形态 | 5 passed；**反向验证**：破坏注入实现 → `FAILED`/101 |
| `icon_window_test.rs` | 文件内**独立复刻**像素转换，注释自认"两侧需同步" | 从 `commands/dsh.rs` 抽取生产纯函数 `rgba_to_bgra_and_mask`（`pub`），集成测试改为直调；另在 `dsh.rs` 内加 **6 个单元测试**（通道交换、mask 回绕、缓冲长度、多像素顺序、空输入） | 6 + 1 passed |
| `concurrency_test.rs` | 测 tokio `spawn_blocking` 的运行时行为；挂钟阈值 `<700ms` 在负载下 flaky；与产品逻辑无关 | 重写为直测生产 `ProcessManager`：并发 `start` 不得双进程、并发 `stop` 幂等不 panic、非法端口被拒不改变状态、`restart` 从未启动过不 panic、并发状态查询自洽 | 7 passed |

**连带清理**：`tokio` 此前**仅**被那个假测试使用（`src/` 与其余 `tests/` 零引用），重写后成为未使用依赖 → 按 D19 移除 `[dev-dependencies] tokio`。移除后 `cargo check --all-targets` 仍零警告。

**踩到并修正的一个验证陷阱（值得记录）**：用 `Copy-Item` 还原被注入破坏的源文件时，**mtime 被保留**，cargo 判定文件未变而**不重编译**，导致"还原后仍 FAILED"的假象（一度让我误判"生产函数不注入 PATH"，并去写了插桩诊断）。强制 `touch` 后即恢复正常。**教训：反向验证必须在还原后确认二进制真的重建**，否则会得出关于生产代码的错误结论。

### D12 — 符号链接失败改为跳过（不再 panic）

- `create_file_symlink` / `create_dir_link` 返回类型由 `()` 改为 **`bool`**：失败时打印 `[skip]` 原因并返回 `false`，调用方 `if !… { return; }` 跳过该用例。
- 全部 6 个调用点均已加守卫（含 `repair_links` 相关的 3 处链式创建）。
- **契约测试**：新增 2 个用例，用「link 路径先被占用」这一**确定性失败**制造错误，
  断言返回 `false` 而非 panic —— 因此该断言**不依赖本机是否有符号链接特权**，两种情况都成立。
- 验证：`part_b_compliance_test` **17 passed**（原 15 + 新 2）；`--list` 确认新用例真实存在。
- 说明：本文档原计划"拆出 `part_b_static_assert_test.rs`"。实施时发现：**panic 已消除后，该文件全部 17 个用例都能在无特权环境跑通**（失败路径自动跳过），故**无需再拆文件** —— 原计划的拆分动因（避免符号链接拖垮整文件）已随 panic 消除而消失。

### D13 — nightly 网络/环境集成

新增 `.github/workflows/nightly.yml`：每日 `03:17 UTC` + 手动触发，以 `--ignored` 依次运行
`github_tag_test`、`npm_versions_test`、`direct_node_capture_test`；`continue-on-error: true`
（外部服务可用性不应当作代码门禁）；失败时上传日志工件。

**本地验证**：`--ignored --list` 确认**确实选中 5 个用例**
（`test_github_tag_passthrough`、`test_list_releases_format`、`test_npm_install_dry`、
`test_npm_list_versions_full`、`test_direct_node_captures_token_url`）——
此前它们**从未被任何流水线执行**。

### D14 — 文档对齐

- **重写 `docs/DESIGN.md`**：§3 模块划分按实际结构重写（覆盖 `core/` 顶层 15 文件 +
  `plugin/` 7 + `mcp/` 6 + `skill/` 9 + `commands/` 8），删除不存在的
  `install.rs`/`mirror.rs`/`backup.rs` 与顶层 `logging.rs`/`events.rs`；
  新增 §3.1「分层纪律」（5 条架构约束及其落点）、§8「质量门禁」。
  **验证**：脚本比对 —— DESIGN.md 提及了**全部 49 个** `src-tauri/src/**/*.rs` 文件；
  三个"幽灵模块"仅出现在文首的**订正说明**中（有意保留，说明历史滞后）。
- **`ADR-0004:11`**："三处同步" → **五处同步**（含 `package-lock.json` 两处与 `Cargo.lock`），
  并标注该滞后由 ADR-0009 D14 订正。
- **CI 清单同步**：删除 `channel_mutex_logic_test`，补入 `github_channel_state_test`、
  `version_sync_test`、`part_b_compliance_test`、`plugin_pipeline_test`、`mcp_pipeline_test`
  → 集成清单由 8 个文件扩为 **12 个**。

### Phase 3 门禁结果

| 门禁 | 结果 |
| --- | --- |
| `cargo check --all-targets` | ✅ 零警告（移除 tokio 后仍零警告） |
| `cargo test --lib` | ✅ **193 passed**（含新增 6 个图标像素单测） |
| CI 集成清单（12 文件） | ✅ **63 用例全绿** |
| `part_b_compliance_test` | ✅ 17 passed（含 2 个符号链接失败契约用例） |
| nightly `--ignored` 选择 | ✅ 5 个真实链路的用例均可被选中 |
| DESIGN.md 对照 | ✅ 49/49 源文件全覆盖，幽灵模块已清 |

---

## Phase 4 实施记录（2026-09-12，已完成并验证）

### 决策定案：**不打码**

D5 的初版方案是"日志打码 + UI 复制入口"。**产品所有者于 2026-09-12 裁定改为「不打码」**
（延续 v0.5.6 决策）：日志与前端日志流保留完整 dsh web 访问地址，以便用户直接从日志复制
地址在外部浏览器打开（裸 URL 会被 dsh 以 401 拒绝）。本 ADR 的 D5 决策表、精确契约、
Context §3、Consequences 与 Phase 4 门禁口径**均已按该裁定同步修订**。

### 改动清单

| # | 位置 | 改动 |
| --- | --- | --- |
| 1 | `core/logging.rs` | **删除** `redact_web_token()` 及其单测（`test_redact_web_token`）—— 该函数自 v0.5.6 起因生产路径不再调用而成为死代码 |
| 2 | `core/logging.rs` | 新增「dsh web token 的日志口径」说明节：记录决策依据（明文换取可用性）、历史沿革（v0.4.13 打码 → v0.5.6 明文 → 本 ADR 定案）与安全权衡 |
| 3 | `core/logging.rs` | 订正与实现矛盾的注释（原"日志文件本身不涉密（token 已打码）"→ 改为按 LOCALAPPDATA 私有目录论证，并显式指明明文口径） |
| 4 | `core/process.rs` | 在 `process_dsh_output_line` 文档注释中固化口径：明文落盘/推送的原因、死代码已删除、安全权衡由所有者接受 |
| 5 | `CHANGELOG.md:541` | **订正历史条目的错误声明**：原文称"dsh web token 不再明文进日志/日志流（日志打码 + 独立缓存文件）"，与 v0.5.6 起的实现及本 ADR 定案直接矛盾 → 改为指向 v0.5.6 的明文口径并标注已被取代 |

**能力不退化**：`get_web_url`、`save_latest_web_url`、`extract_latest_web_url`（含从日志扫描的兜底）
行为**未改动**，继续提供完整 URL。

### 验证证据

| 验证项 | 方法 | 结果 |
| --- | --- | --- |
| 死代码彻底清除 | 逐行剥离注释后匹配 `redact` | ✅ 零代码性引用（仅 2 处注释作历史说明） |
| 打码单测已移除 | 搜索 `test_redact_web_token` | ✅ 无此用例（`cargo test --lib` 数量相应变化，无失败） |
| 明文口径保留 | 读 `process.rs:836-849` | ✅ 仍以 `logger.log(LogSource::Dsh, level, line)` 原文落盘/推送 |
| Rust 静态检查 | `cargo check --all-targets` | ✅ **零警告** |
| Rust 单元测试 | `cargo test --lib` | ✅ **198 passed / 0 failed** |
| 文档无矛盾声明 | 全 docs + CHANGELOG 搜"打码" | ✅ 仅剩"历史沿革/已取代"性质的说明，无"当前已打码"的错误陈述 |

### 附带发现并修正的**真实文档缺陷**

`CHANGELOG.md:541` 的 v0.4.13 条目长期声称"dsh web token 不再明文进日志/日志流（日志打码）"，
而这与 v0.5.6（`CHANGELOG.md:376-381` 明确记录改为明文）之后的实际行为**相反**。
该矛盾属审计未列出的新发现，已在本次一并订正 —— 否则历史条目会持续误导维护者
（正是根因 B「实现说一套、注释说另一套」的又一实例）。

---

## References

- 审计基线：`Y:\dsh-launcher` @ `v0.9.0`（`package.json:4`、`Cargo.toml:3`、`tauri.conf.json:4`、`Cargo.lock:834`）
- 版本与发布：`scripts/bump-version.mjs:35-59`、`.github/workflows/release.yml:75-125`、`.github/workflows/ci.yml:44-70`、`docs/adr/0004-version-numbering-scheme.md:9-12`
- MCP：`src-tauri/src/core/mcp/{mod,block,entry,validate,prereq}.rs`、`src-tauri/src/core/plugin/dump.rs:202-423`
- 插件与 profile：`src-tauri/src/core/plugin/{mod,managed,state,spec,sync,registry}.rs`、`src-tauri/src/core/profile.rs:235-290`
- 技能：`src-tauri/src/core/skill/{scan,manage,import,source,update,editor,sharing}.rs`
- 日志与 token：`src-tauri/src/core/logging.rs:197-238,402-467`、`src-tauri/src/core/process.rs:822-846`、`src-tauri/src/commands/dsh.rs:327-346`
- 前端：`src/hooks/useResizablePanel.ts`、`src/lib/panel-layout.ts`、`src/components/{AppShell,LogPanel,ToolchainPanel,StatusCard,VersionPanel,McpPanel,PluginsPanel,SkillsPanel,WindowControls}.tsx`、`src/index.css`
- 测试：`src-tauri/tests/`（15 个）、`scripts/e2e-adr0006.ps1`、`scripts/verify-layout.py`
- 参考 ADR：`docs/adr/0001-dual-channel-install-model.md`、`0002-process-lifecycle-and-status-detection.md`、`0003-single-global-version-model.md`、`0005-plugin-and-skill-management.md`、`0007-skill-management.md`、`0008-skill-import-update-and-open.md`
