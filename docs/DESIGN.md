# dsh-launcher 设计总览

> 基于三轮 grilling 的最终设计。决策依据见 `CONTEXT.md`（词表）与 `docs/adr/`（ADR-0001~0004）。

## 1. 产品定位

**dsh-launcher** 是 deepseek-harness（`dsh`）的 Windows 桌面启动器与管理器：

- **不改 dsh 源码**，只复用官方 CLI/接口（`dsh web`、SIGTERM 信号协议、npm 包、GitHub Releases）。
- **全局单版本模型**：dsh 与工具链都是全局安装、单一版本；切换版本 = 先卸载再装。
- **职责收敛**：安装/卸载/更新、工具链检测与一键补齐、生命周期控制、日志全域、Web GUI 内嵌。

## 2. 技术栈（最终裁定）

| 层 | 选型 | 职责 |
|---|---|---|
| 后端 | **Rust（Tauri 2）** | 进程生命周期、端口探测、工具链检测/安装、日志落盘、配置持久化、版本/通道管理、事件推送 |
| 前端 | **TypeScript + React + shadcn/ui（黑暗主题）** | UI 渲染、配置表单、版本列表、日志流、托盘菜单、WebView 内嵌 |
| 桌面 | **Tauri 2（WebView2）** | 窗口、托盘、系统集成；**抛弃 Electron**（Q28 裁定） |
| 分发 | **NSIS 安装包 + 便携版（zip）** | Windows 两种分发形态 |

## 3. 模块划分（Rust 后端）

```
src-tauri/src/
├── main.rs               # Tauri 入口（调用 lib.rs::run）
├── lib.rs                # 应用装配：窗口图标/托盘/单实例/内嵌 Web GUI 窗口
├── commands/             # IPC 命令（前端调用）
│   ├── mod.rs
│   ├── dsh.rs            #   dsh 生命周期：start/stop/restart/status + 内嵌窗口图标
│   ├── config.rs         #   配置读写：端口/镜像源/滑动开关/GitHub Token
│   ├── toolchain.rs      #   工具链：detect/install
│   ├── version.rs        #   版本/通道：list/install/uninstall/paths
│   └── logs.rs           #   日志：list/read（实时流经 event 推送）
├── core/
│   ├── mod.rs
│   ├── process.rs        #   spawn dsh、PID 跟踪、SIGTERM/taskkill、状态机
│   ├── port.rs           #   端口探测（启动前预检 + 5s 轮询兑底）
│   ├── command.rs        #   无窗口子进程辅助（CREATE_NO_WINDOW / cmd 包装）
│   ├── stream.rs         #   流式命令执行（逐行日志 + 进度回调）
│   ├── github.rs         #   GitHub 通道：ls-remote/list、clone、pnpm 构建、全局 shim
│   ├── toolchain.rs      #   Node 下载/解压安装（用户级）
│   ├── events.rs         #   Tauri event 推送（安装进度/版本变更/插件/技能）
│   ├── logging.rs        #   dsh stdout/stderr + 启动器日志统一落盘（按天/10MB 轮转）
│   ├── config.rs         #   启动器配置持久化（%APPDATA%\dsh-launcher\config.json）
│   ├── tray.rs           #   系统托盘 + 窗口事件（关闭/最小化行为）
│   ├── dshhome.rs        #   DSH_HOME / profiles / agents home 解析（唯一实现）
│   ├── profile.rs        #   官方 API 适配器（唯一调用 dsh/pnpm、唯一读写 profile）
│   ├── plugin/           #   插件管理（ADR-0005）：状态机/受管区块/dump 解析/注册表/同步
│   └── skill.rs          #   技能共享资源（ADR-0005）：检测/链接或配置落地/迁移
├── cli.rs                # 无 GUI CLI（plugin/skill 子命令，与 GUI 共用 core）
```

## 4. 核心流程

### 4.1 工具链检测与补齐
1. 检测清单：Node.js(22.19+/24+)、npm(随 Node)、pnpm、Git(2.26+)、Python(可选)。
2. 状态：`present` / `missing` / `mismatch`；缺失或版本不符 → UI 列出 + "一键安装"。
3. 安装走**官方通道**：Node 为用户级目录（免 UAC，写入用户 PATH）；Git/Python 用官方安装器、需提权（runas 子进程），主进程不常驻管理员。
4. 网络问题由镜像源解决（npm registry / GitHub 加速 / Node 镜像独立配置，默认官方源 + 常用镜像下拉）。

### 4.2 dsh 安装（双入口 + 全局单版本）
- **npm 通道**：`npm i -g @deepseek-ai/dsh@<version>`（registry 现有版本）。
- **GitHub 通道**：git ls-remote 列 tag → 选 v0.1.2-alpha.1 及更新的源码 tag → clone → `pnpm install` + `pnpm build`（ADR-0001/0003）。
- **全局单版本**：切换 = 先卸载再装；GitHub 安装后建全局 `dsh.cmd` shim。

### 4.3 生命周期
- **启动**：spawn `dsh web --port <p>`，CWD = dsh 官方默认（不干预 workspace root），注入全局工具链 PATH。
- **停止**：Windows `taskkill /PID /T` 模拟优雅停止 → 等 ≤1s 未退则 `/F` 强杀；随后按端口兜底清剿（清剿前校验监听进程确为 dsh，防误杀无关进程，v0.4.13）。
- **重启**：停止后用相同配置（端口 + CWD）重启；端口被占则提示手动改。
- **状态检测**：事件驱动（进程句柄退出即收状态）+ 端口探活（收养/停止前校验）+ 后端 5s 对账线程兜底（v0.4.13）。

### 4.4 日志
- 采集：dsh stdout/stderr + 启动器自身日志，统一时间戳格式。
- 落盘：`%LOCALAPPDATA%\dsh-launcher\logs\<date>.log` 按天轮转 + 单文件 10MB 切割（.log.1…），保留 30 天。
- UI：实时流（`log://line` event，最新在上）+ 文件视图/导出（导出前剔除 emoji 防乱码）。

### 4.5 窗口与托盘
- 内嵌 WebView2 渲染 `http://127.0.0.1:<port>`（含 token 免认证）+ "外部浏览器打开"兜底按钮（opener 插件）。
- 内嵌窗口任务栏图标：Rust 侧设置 SMALL + ICON_BIG（多尺寸 ICO 最大帧，防模糊）。
- 托盘菜单：打开主窗口 / 启动 / 停止 / 重启 / 退出。
- 滑动开关（7 项）：① 关闭直接退出（含 dsh）② 最小化到托盘 ③ 退出时驻留 dsh ④ 卸载保留 DSH_HOME（默认保留）⑤ 启动时自动启动 dsh ⑥ 启动时自动打开 Web GUI ⑦ 自动同步上游插件（默认开）。

### 4.6 插件与技能管理（ADR-0005）

- **入口**：侧栏底部按钮组 `插件 | 技能 | 设置`（顺序固定），各开一个 Dialog。
- **插件**：按包名（ID）独立管理 `enabled/disabled/uninstalled`。
  - 启停 = 写 profile `cordis.patch.yml` 的受管区块（`- id:` + `disabled:`）→ dsh `live` 热重载，不重启；
  - 装卸 = `dsh plugin --profile web add/remove`（官方唯一依赖通道）→ 需要重启时自动 stop→apply→start；
  - 幂等：期望态与磁盘一致时返回 `unchanged`，不落盘、不重启；
  - upstream 插件后台自动同步（npm 版本 / git commit，git 一律钉 sha）；自研插件永不被同步改动。
- **技能**：共享真源 `~/.agents/agent`；优先建立 `~/.dsh` 链接（Mode L），无文件链接权限时降级为
  `$DSH_HOME/cordis.patch.yml` 的 shared 区块（Mode C）。冲突资源只提示与迁移，绝不自动删除。
- **CLI**：`dsh-launcher plugin|skill ...` 与 GUI 共用同一 core，供脚本化验收。

## 5. 数据与配置

- 启动器配置：`%APPDATA%\dsh-launcher\config.json`（端口、镜像源、GitHub Token、滑动开关）。
- 受管插件注册表：`%APPDATA%\dsh-launcher\plugins.json`（来源分类/期望态/上次同步；磁盘为事实源）。
- 技能共享偏好：`%APPDATA%\dsh-launcher\skills.json`（共享模式偏好与最近应用时间）。
- 插件操作备份：`%LOCALAPPDATA%\dsh-launcher\backups\plugins\<pkg>\<ts>\`。
- 日志：`%LOCALAPPDATA%\dsh-launcher\logs\`（单文件 10MB 切割，30 天保留）。
- DSH_HOME：**只读展示、不接管**（实际位置以 dsh 为准），卸载时默认保留、可开关（`keepDshHomeOnUninstall`）；
  插件/技能管理只按白名单触碰 profile 的 `cordis.patch.yml` 受管区块与 `$DSH_HOME/cordis.patch.yml`。

## 6. 版本策略（ADR-0004）

- 启动器版本独立于 dsh；
- 同步处：`package.json` + `src-tauri/Cargo.toml` + `src-tauri/tauri.conf.json` + `package-lock.json` + `Cargo.lock`（bump-version.mjs 自动同步五处）。
- `CHANGELOG.md` 顶部插中文条目。

## 7. 分发

- **NSIS 安装包** + **便携版 zip**。
- DoD：全流程冒烟（检测→装工具链→装 dsh→启动→内嵌 web UI 可交互→停止→卸载）在干净 Windows 跑通；日志导出含 dsh+启动器全量；NSIS 与便携版均可安装/运行。

## 8. 开放事项（实现前确认）

- Tauri 2 的 WebviewWindow 内嵌是否满足 dsh web GUI 全部交互（如有兼容问题退化为外链浏览器）。
- `taskkill /T` 对 Node 子进程树（pnpm 构建产物运行）的优雅排空验证。
- 提权子进程（runas）在 Windows 不同版本上的 UAC 行为一致性。
