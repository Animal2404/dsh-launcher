# dsh-launcher

[deepseek-harness](https://github.com/deepseek-ai/deepseek-harness) 的 Windows 桌面启动器与运行环境管理器（Tauri 2 + React + TypeScript + shadcn/ui）。

> [!IMPORTANT]
> **本分支（`feat/token-tracker-integration`）新增：📊 Token 统计面板 —— 集成 [TokenTracker](https://github.com/xiufengsun/TokenTracker)，一个看板聚合 36 个 AI 编码工具的 token 用量与成本，启动器打开即自动运行。** 全部改动 10 个文件（+841/−4），详见下方 [Token 统计](#-token-统计tokentracker-集成--本分支新增)章节与[改动清单](#-改动清单)。

## 功能

- **dsh 生命周期管理**：启动 / 停止 / 重启 `dsh web`（端口可配置，默认 3080），状态实时探测
- **双通道版本管理**：npm 通道（registry 装包）与 GitHub 通道（clone + pnpm 构建），最新版本置顶，安装进度实时展示
- **工具链一键安装**：检测 Node / npm / pnpm / Git / Python，支持镜像源（npm registry / GitHub 加速 / Node 二进制）
- **Web GUI 集成**：内嵌 Tauri 窗口打开 dsh Web UI（自动携带 token 免认证），或外部浏览器 / 桌面快捷方式
- **📊 Token 统计（本分支新增，详见下文）**：36 个 AI 工具统一 token 用量看板
- **系统托盘**：打开主窗口 / 启动 / 停止 / 重启 / 退出，可配置"退出时驻留 dsh"
- **全局日志**：dsh 与启动器日志统一落盘（按天轮转 + 切割 + 保留 30 天），前端实时流式展示

---

## 📊 Token 统计（TokenTracker 集成）— **本分支新增**

> **一句话**：主界面新增「**Token 统计**」Tab——聚合 **36 个 AI 编码工具**（Claude Code、Codex、Cursor、Gemini、DeepSeek Harness、OpenCode、Grok…）的 **token 总数 / 预估费用 / 模型占比 / 每日细目 / 活动热力图**，数据全部留在本机，**启动器打开即自动运行，零配置**。

### ✨ 它能做什么

- **一个看板看所有 Agent**：本地解析各工具日志统一聚合——不只 DeepSeek Harness，Claude Code / Codex / Cursor / Gemini 等全部覆盖
- **启动即用**：启动器启动 2.5 秒后自动拉起看板；若已有实例在跑则**直接收养**，秒级恢复、绝不重复起进程
- **两种打开方式**：主界面 iframe 内嵌看板，或工具条一键弹**独立窗口**
- **手动控制保留**：工具条随时 启动 / 停止 / 刷新 / 独立窗口，状态徽标 + 端口实时可见
- **未安装也有兜底**：检测不到 `tokentracker-cli` 时提供一键安装按钮（npm 全局包，Node ≥ 20）

### 🔑 谷歌登录（云同步 / 排行榜）— **请务必用「独立窗口」**

- Google 登录页**禁止被 iframe 嵌入**（X-Frame-Options 安全策略），面板内点谷歌登录无法弹出登录页——这是 Google 的限制，不是 bug
- 正确姿势：点工具条「**独立窗口**」→ 在窗口里完成谷歌登录 → 登录态同源自动共享回面板 iframe
- TokenTracker 云端 OAuth 回调白名单**仅含默认端口 7680**——本集成的收养机制保证看板固定跑在 7680（被外部 tracker 实例占用时直接收养），登录回调不会再报 `not in the allowed redirect URLs`

### 🧩 技术实现（为什么这样集成）

- **零 vendor、零编译**：不搬运 TokenTracker 源码，托管官方 npm 包 `tokentracker-cli`（看板已预构建），本仓库只加约 840 行胶水代码
- **复用启动器既有基建**：子进程生命周期管理 / 端口探活 / 内嵌窗口（带高清图标）/ Node 工具链 PATH 注入——与托管 dsh 同一套模式
- **端口策略**：优先 **7680**（OAuth 白名单端口）；被 tracker 实例占用 → **收养**；被其他程序占用 → 递增退让（此时谷歌登录回调不可用，日志会有警告）
- **自动启动**：启动器 setup 后 2.5 秒后台执行，失败仅落日志静默降级（未安装 / 端口全占时面板内手动按钮兜底）

### 📁 改动清单（本分支 10 文件 +841/−4）

| 类型 | 文件 | 内容 |
|---|---|---|
| **新增** | `src-tauri/src/core/tokentracker.rs` | 进程管理器：CLI 检测 / 一键安装 / serve 生命周期 / 优雅停止 / **7680 收养** / 自动启动 |
| **新增** | `src-tauri/src/commands/tokentracker.rs` | 7 个 IPC 命令（状态/端口/检测/安装/启动/停止/独立窗口） |
| **新增** | `src/components/TokenPanel.tsx` | Token 统计面板（紧凑工具条 + iframe 看板） |
| **修改** | `src-tauri/src/lib.rs` | AppState + 命令注册 + 启动 2.5s 自动拉起/收养 |
| **修改** | `src/components/AppShell.tsx` | 主区「版本管理 / Token 统计」Tab——双面板常驻挂载（修复切 Tab 重新加载） |
| **修改** | `src/lib/tauri.ts` | 前端类型化 IPC 封装 |
| **修改** | `src-tauri/capabilities/dsh-web-gui.json` | 放行 `tokentracker-dash-*` 看板窗口 |
| **修改** | `core/mod.rs` / `commands/mod.rs` / `README.md` | 模块注册 / 文档 |

---

## 安装

从 [Releases](https://github.com/youridol/dsh-launcher/releases) 下载最新 `dsh-launcher_<version>_x64-setup.exe` 安装包（Windows x64，NSIS 简体中文安装器，免管理员）。

## 开发

```bash
npm install          # 安装依赖（注意本机 .npmrc 若含 omit=dev 需 --include=dev）
npm run tauri dev    # 开发模式（前端 + Rust 热重载）
npm run build        # 前端构建（tsc + vite build）
npm run tauri build  # 打包安装包（NSIS）
```

> ⚠️ 不用 `tauri build` 直接 `cargo build --release` 时，必须显式加
> `--features tauri/custom-protocol`，否则 Tauri 按 dev 模式加载 `devUrl`
> （localhost:1420）导致白屏/连接拒绝；且修改 `dist/` 后如未生效，先
> `cargo clean -p dsh-launcher`（tauri-build 不追踪 dist 文件变化）。

Rust 后端入口在 `src-tauri/src/`（`core/` 核心逻辑 + `commands/` Tauri IPC），前端在 `src/`。

## 持续集成与发布

仓库内置 GitHub Actions 自动流水线：

- **CI**（`.github/workflows/ci.yml`）：每次 push / PR 自动执行 tsc 类型检查、前端构建、cargo check、单元与离线集成测试
- **Release**（`.github/workflows/release.yml`）：每次 push 到 master 自动迭代 PATCH 版本、更新 CHANGELOG、构建 NSIS 安装包并发布 GitHub Release

版本迭代由 `scripts/bump-version.mjs` 同步五文件（package.json / package-lock.json / Cargo.toml / Cargo.lock / tauri.conf.json，见 ADR-0004），幂等防重复发布。

## 文档

- `CONTEXT.md`：术语表
- `docs/DESIGN.md`：设计总览
- `docs/adr/`：架构决策记录（ADR-0001~0004）

## 技术栈

Tauri 2 · Rust · React 19 · TypeScript · Vite · Tailwind CSS 4 · shadcn/ui

## 许可

[MIT](LICENSE) © 2026 dsh-launcher contributors
