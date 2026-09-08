// Tauri invoke 的类型化封装
// 对应 Rust 端 commands/* 模块（见 src-tauri/src/commands/）

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** dsh 安装版本变更事件名（对应 Rust VERSION_CHANGED_EVENT） */
export const VERSION_CHANGED_EVENT = "version://changed";

/** 订阅 dsh 安装版本变更事件（卸载/安装后触发，前端刷新安装状态） */
export async function listenVersionChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(VERSION_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 工具链变更事件名（对应 Rust TOOLCHAIN_CHANGED_EVENT） */
export const TOOLCHAIN_CHANGED_EVENT = "toolchain://changed";

/** 订阅工具链变更事件（安装完成后触发，前端刷新各条目状态） */
export async function listenToolchainChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(TOOLCHAIN_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** dsh 运行状态（对应 Rust DshStatus） */
export type DshStatus = "stopped" | "starting" | "running" | "stopping" | "error";

/** 工具链项（对应 Rust ToolchainItem） */
export interface ToolchainItem {
  name: string;
  installedVersion: string | null;
  required: string;
  state: string;
}

/** 可用 dsh 版本（对应 Rust DshVersion） */
export interface DshVersion {
  version: string;
  channel: "npm" | "github";
}

/** 查询 dsh 运行状态 */
export function getDshStatus(): Promise<DshStatus> {
  return invoke("get_status");
}

/** 获取 dsh web 完整访问 URL（含 token，免认证） */
export function getWebUrl(): Promise<string> {
  return invoke("get_web_url");
}

/** 探测 dsh web 是否 HTTP 200 可服务（开窗前确认，防冷启动 404） */
export function probeWebReady(url: string): Promise<boolean> {
  return invoke("probe_web_ready", { url });
}

/** 创建内嵌 Web GUI 窗口（v0.4.15：统一走 Rust 窗口创建，消除 JS 创建路径无高清图标的问题） */
export function createWebGuiWindow(url: string): Promise<string> {
  return invoke("create_web_gui_window", { url });
}

/** 为内嵌 Web GUI 窗口设置高清任务栏图标（Rust 创建路径内部已设置；本命令为兜底重试） */
export function setWebGuiIcon(label: string): Promise<void> {
  return invoke("set_web_gui_icon", { label });
}

/** 创建桌面快捷方式（双击用默认浏览器打开 dsh web） */
export function createDesktopShortcut(): Promise<string> {
  return invoke("create_desktop_shortcut");
}

/** 启动 dsh web */
export function startDsh(): Promise<string> {
  return invoke("start_dsh");
}

/** 停止 dsh */
export function stopDsh(): Promise<string> {
  return invoke("stop_dsh");
}

/** 重启 dsh */
export function restartDsh(): Promise<string> {
  return invoke("restart_dsh");
}

/** 检测工具链 */
export function detectToolchain(): Promise<ToolchainItem[]> {
  return invoke("detect_toolchain");
}

/** 一键安装工具链 */
export function installToolchain(name: string): Promise<string> {
  return invoke("install_toolchain", { name });
}

/** 卸载工具链（node 同步删目录；git/python 启动官方卸载器 UAC） */
export function uninstallToolchain(name: string): Promise<string> {
  return invoke("uninstall_toolchain", { name });
}

/** 批量操作结果（对应 Rust BatchResult） */
export interface BatchResult {
  name: string;
  ok: boolean;
  message: string;
}

/** 批量一键安装缺失工具链（按 node→pnpm→git→python 依赖顺序） */
export function batchInstallToolchains(): Promise<BatchResult[]> {
  return invoke("batch_install_toolchains");
}

/** 一键卸载全部工具链 */
export function batchUninstallToolchains(): Promise<BatchResult[]> {
  return invoke("batch_uninstall_toolchains");
}

/** 列出某通道可用版本 */
export function listVersions(channel: string): Promise<DshVersion[]> {
  return invoke("list_versions", { channel });
}

/** 获取已安装版本 */
export function getInstalledVersion(): Promise<string | null> {
  return invoke("get_installed_version");
}

/** 安装路径信息 */
export interface InstallPaths {
  githubDir: string;
  githubInstalled: boolean;
  npmGlobalDir: string;
  npmBinDir: string;
}

/** 获取 harness 下载/安装目录 */
export function getInstallPaths(): Promise<InstallPaths> {
  return invoke("get_install_paths");
}

/** 安装指定版本 */
export function installVersion(channel: string, version: string): Promise<string> {
  return invoke("install_version", { channel, version });
}

/** 卸载 dsh */
export function uninstallDsh(): Promise<string> {
  return invoke("uninstall");
}

/** 日志文件条目 */
export interface LogFile {
  path: string;
  size: number;
  modified: number;
}

/** 应用配置（对应 Rust ConfigView） */
export interface AppConfig {
  port: number;
  npmRegistry: string;
  githubMirror: string;
  /** v0.4.13：Rust 不再回传明文 token，只回传是否已设置 */
  githubTokenSet: boolean;
  githubToken: string;
  nodeMirror: string;
  closeExits: boolean;
  minimizeToTray: boolean;
  keepDshOnExit: boolean;
  keepDshHomeOnUninstall: boolean;
  autoStartDsh: boolean;
  autoOpenBrowser: boolean;
  /** 启动后自动同步 upstream 插件（自研插件不受影响） */
  autoSyncPlugins: boolean;
}

/** 读取配置 */
export function getConfig(): Promise<AppConfig> {
  return invoke("get_config");
}

/** 保存端口 */
export function setPort(port: number): Promise<void> {
  return invoke("set_port", { port });
}

/** 保存镜像源 */
export function setMirrors(opts: {
  npmRegistry: string;
  githubMirror: string;
  nodeMirror: string;
}): Promise<void> {
  return invoke("set_mirrors", {
    npmRegistry: opts.npmRegistry,
    githubMirror: opts.githubMirror,
    nodeMirror: opts.nodeMirror,
  });
}

/** 保存 GitHub Token（防 API 限流 / git 认证增强） */
export function setGithubToken(token: string): Promise<void> {
  return invoke("set_github_token", { token });
}

/** 保存滑动开关 */
export function setSwitches(opts: {
  closeExits: boolean;
  minimizeToTray: boolean;
  keepDshOnExit: boolean;
  keepDshHomeOnUninstall: boolean;
  autoStartDsh: boolean;
  autoOpenBrowser: boolean;
  autoSyncPlugins: boolean;
}): Promise<void> {
  return invoke("set_switches", opts);
}


/** 列出日志文件 */
export function listLogs(): Promise<LogFile[]> {
  return invoke("list_logs");
}

/** 读取日志文件内容 */
export function readLog(relPath: string): Promise<string> {
  return invoke("read_log", { relPath });
}

// ==================== 插件管理（ADR-0005） ====================

/** 插件变更事件名（对应 Rust PLUGIN_CHANGED_EVENT） */
export const PLUGIN_CHANGED_EVENT = "plugin://changed";

/** 技能共享变更事件名（对应 Rust SKILL_CHANGED_EVENT） */
export const SKILL_CHANGED_EVENT = "skill://changed";

/** 订阅插件变更事件 */
export async function listenPluginChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(PLUGIN_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 订阅技能共享变更事件 */
export async function listenSkillChanged(
  onChanged: () => void,
): Promise<UnlistenFn> {
  return listen(SKILL_CHANGED_EVENT, () => {
    onChanged();
  });
}

/** 插件状态（对应 Rust PluginState） */
export type PluginState = "uninstalled" | "plain" | "enabled" | "disabled";

/** 行状态（对应 Rust RowState） */
export type RowState = "enabled" | "disabled" | "expression";

/** 来源分类（对应 Rust Origin） */
export type PluginOrigin = "upstream" | "in-house" | "unknown";

/** 依赖 spec 形态（对应 Rust SpecKind） */
export type SpecKind = "npm" | "git" | "path" | "tarball" | "unknown";

/** 插件来源描述 */
export interface PluginSource {
  kind: SpecKind;
  spec: string;
  repo: string | null;
  reference: string | null;
  commit: string | null;
}

/** 一次同步的结果 */
export interface SyncRecord {
  at: string;
  from: string;
  to: string;
  result: string;
}

/** 行视图 */
export interface RowView {
  id: string;
  name: string | null;
  state: RowState;
}

/** 插件视图（对应 Rust PluginView） */
export interface PluginView {
  package: string;
  version: string | null;
  origin: PluginOrigin;
  state: PluginState;
  rows: RowView[];
  source: PluginSource | null;
  lastSync: SyncRecord | null;
  protected: boolean;
  needsReconcile: boolean;
  desired: string | null;
  lastError: string | null;
  installedSpec: string | null;
}

/** 插件列表结果 */
export interface PluginList {
  profile: string;
  plugins: PluginView[];
  degradedReason: string | null;
}

/** 操作结果 */
export interface OpResult {
  status: "changed" | "unchanged";
  restarted: boolean;
  message: string;
  package: string | null;
}

/** 单个插件的同步结果 */
export interface SyncItemResult {
  package: string;
  origin: PluginOrigin;
  from: string | null;
  to: string | null;
  result: string;
  message: string;
}

/** 同步报告 */
export interface SyncReport {
  applied: boolean;
  restarted: boolean;
  items: SyncItemResult[];
}

/** 列出插件 */
export function pluginList(): Promise<PluginList> {
  return invoke("plugin_list");
}

/** 安装插件（spec 可为 npm 包名/git 地址/本地路径） */
export function pluginInstall(
  spec: string,
  origin?: PluginOrigin,
): Promise<OpResult> {
  return invoke("plugin_install", { spec, origin: origin ?? null });
}

/** 启用/禁用插件 */
export function pluginSetState(
  pkg: string,
  enabled: boolean,
): Promise<OpResult> {
  return invoke("plugin_set_state", { package: pkg, enabled });
}

/** 卸载插件 */
export function pluginUninstall(pkg: string): Promise<OpResult> {
  return invoke("plugin_uninstall", { package: pkg });
}

/** 同步 upstream 插件（apply=false 只检查） */
export function pluginSync(apply: boolean, pkg?: string): Promise<SyncReport> {
  return invoke("plugin_sync", { apply, package: pkg ?? null });
}

/** 收敛：重新对账 bundles 并重放期望态 */
export function pluginRepair(pkg?: string): Promise<OpResult> {
  return invoke("plugin_repair", { package: pkg ?? null });
}

// ==================== 技能共享（ADR-0005） ====================

/** 资源状态（对应 Rust ResourceState） */
export type ResourceState =
  | "missing"
  | "linked"
  | "config"
  | "conflict"
  | "broken";

/** 单个共享资源状态 */
export interface ResourceStatus {
  resource: string;
  canonical: string;
  view: string;
  state: ResourceState;
  detail: string;
}

/** 技能共享总状态 */
export interface SkillStatus {
  canonicalRoot: string;
  dshHome: string;
  agentsHome: string;
  linkCapable: boolean;
  activeMode: string;
  preferredMode: string;
  resources: ResourceStatus[];
  skillCount: number;
}

/** 应用结果 */
export interface SkillApplyReport {
  mode: string;
  changed: boolean;
  message: string;
  resources: ResourceStatus[];
}

/** 迁移动作 */
export interface MigrateAction {
  resource: string;
  action: string;
  detail: string;
}

/** 迁移报告 */
export interface MigrateReport {
  dryRun: boolean;
  actions: MigrateAction[];
}

/** 读取技能共享状态 */
export function skillStatus(): Promise<SkillStatus> {
  return invoke("skill_status");
}

/** 应用共享模式（auto/link/config） */
export function skillApply(
  mode: "auto" | "link" | "config",
  resource?: string,
): Promise<SkillApplyReport> {
  return invoke("skill_apply", { mode, resource: resource ?? null });
}

/** 迁移冲突资源（dryRun 只报告） */
export function skillMigrate(dryRun: boolean): Promise<MigrateReport> {
  return invoke("skill_migrate", { dryRun });
}
