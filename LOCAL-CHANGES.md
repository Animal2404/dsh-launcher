# 本 fork 相对上游的改动（务必保留）

- 上游（upstream）：https://github.com/youridol/dsh-launcher
- 本 fork：https://github.com/Animal2404/dsh-launcher

本文件列出**本仓库有意保留的本地改动**。每次同步上游后请对照本清单逐条确认仍然存在，
"本地改动自检"一节的命令会直接显示当前 fork 与上游的全部差异。

## 1. 内嵌 DSH 窗口关闭 Tauri 拖放接管（dsh 插件拖放功能依赖它）

- 文件：`src-tauri/src/commands/dsh.rs`
- 做法：创建内嵌窗口的 builder 链上追加 `.disable_drag_drop_handler()`
- 原因：Tauri v2 默认 `dragDropEnabled = true`——文件拖放在**窗口层**被 Tauri 接管，webview 收不到任何
  HTML5 `dragover`/`drop`。内嵌的 dsh Web UI 里，所有拖放能力都会静默失效：拖放类插件
  （如 `@dsh-local/path-bridge` 把拖入文件转成绝对路径）与 DSH 自身的"附件拖放"都收不到事件。
  该调用是 tauri 2.11.5 官方文档中"在 Windows 前端使用 HTML5 拖放"的前提条件。
- 自检：`git grep -n disable_drag_drop_handler -- src-tauri/src/commands/dsh.rs`

## 2. 自启动实例卡在「启动中」的状态收敛（reconcile 兜底）

- 文件：`src-tauri/src/core/process.rs`（5 秒对账循环里 `DshStatus::Starting if pid != 0 && port != 0` 分支）
- 原因：启动探活线程只等 8s，而 dsh 冷启动 + 插件加载/pnpm 同步常超此窗口；窗口内端口未开则线程退出、
  状态滞留 `Starting` 且此后无人提升 →「内嵌打开」轮询 running 超时、误报「端口未监听」（端口其实已监听）。
  兜底逻辑：托管进程存活且端口已监听 → `Starting` 转 `Running`。
- 自检：`git grep -n "端口探活对账" -- src-tauri/src/core/process.rs`

## 3. README 的构建提示

- 文件：`README.md`「开发」小节
- 内容：不用 `tauri build` 而直接 `cargo build --release` 时必须显式加 `--features tauri/custom-protocol`，
  否则按 dev 模式加载 `devUrl` 导致白屏；改 `dist/` 后需 `cargo clean -p dsh-launcher`。

## 4. 已移除：Token 统计面板（TokenTracker 集成）

- 该功能曾存在于 fork 的 v0.6.x（提交 `51568ff` 等），**已按要求移除**：删除
  `src-tauri/src/core/tokentracker.rs`、`src-tauri/src/commands/tokentracker.rs`、`src/components/TokenPanel.tsx`，
  并去掉 `commands/mod.rs`、`core/mod.rs`、`lib.rs`、`AppShell.tsx`、`src/lib/tauri.ts`、
  `src-tauri/capabilities/dsh-web-gui.json`、`README.md` 中的接线与文档。
- **同步上游时不要把它带回来**；若上游将来合入同类功能，按上游实现走。

## 同步上游的标准流程

```bash
git fetch origin --tags                    # origin = 上游 youridol/dsh-launcher
git checkout -b local/merge-<上游版本> fork/master
git merge origin/master                    # 冲突时：除本清单第 1~3 项外，一律以上游为准
npx tsc --noEmit && npm run build          # 前端类型 + 构建自检
git push fork HEAD:master                  # 推回 fork 的 master
gh workflow run Release -R Animal2404/dsh-launcher --ref master   # 触发云端构建发布
```

## 本地改动自检（一条命令）

```bash
git diff origin/master --stat
# 期望只看到这 5 个文件：
#   CHANGELOG.md                              本 fork 的版本条目（0.6.1/0.6.2…）
#   LOCAL-CHANGES.md                          本文件
#   README.md                                 构建提示
#   src-tauri/src/commands/dsh.rs             拖放接管关闭
#   src-tauri/src/core/process.rs             启动状态收敛
```
