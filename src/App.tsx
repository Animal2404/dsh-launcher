// dsh-launcher 主界面：dark 主题
// 布局：三栏（左侧栏 状态+工具链 / 中央 版本管理 / 右侧 日志），骨架见 AppShell
// 管理入口：侧栏底部按钮组（MCP | 插件 | 技能 | 设置）→ 对应 Dialog（状态在本组件管理）
//
// v0.7.1（弹窗宽度修复 + 统一版式）：
// - 修复：v0.7.0 的四个弹窗在 ≥640px 视口下实际只有 384px —— DialogContent 基线里的
//   `sm:max-w-sm` 在 Tailwind v4 的源序中位于基础工具类之后，恒覆盖调用方的 max-w-*。
//   现在宽度由本文件的 PANEL_MAX_WIDTH（48rem = 768px，即修复前实测 384px 的两倍）声明，
//   且用 `w-[calc(100vw-2rem)]` 保证任意窗口尺寸下都不溢出。
// - 版式：DialogHeader（图标 + 标题 + 说明）常驻，内容区独立滚动（长列表不丢上下文）。
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import AppShell, { type ShellPanel } from "@/components/AppShell";
import SettingsPanel from "@/components/SettingsPanel";
import PluginsPanel from "@/components/PluginsPanel";
import McpPanel from "@/components/McpPanel";
import SkillsPanel from "@/components/SkillsPanel";
import { Toaster } from "@/components/ui/sonner";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Plug, Puzzle, Settings as SettingsIcon, Sparkles } from "lucide-react";

/**
 * 四个管理面板弹窗的宽度上限：48rem = 768px。
 * 该值 = v0.7.0 四个弹窗的**实测渲染宽度 384px 的两倍**（见 CHANGELOG v0.7.1 证据链）。
 * 配合 `w-[calc(100vw-2rem)]`：窗口够宽时取 768px，窗口变窄时自动收缩并保留 1rem 边距。
 */
const PANEL_DIALOG_WIDTH = "w-[calc(100vw-2rem)] max-w-[48rem]";

/** 各面板的标题、说明与图标（标题/图标与侧栏四入口一一对应，保持认知一致） */
const PANEL_META: Record<
  ShellPanel,
  { title: string; desc: string; Icon: typeof Puzzle }
> = {
  mcp: {
    title: "MCP server 管理",
    desc: "按 serverName 管理合成树中的 MCP server；启停由 dsh 就地热重载，无需重启。",
    Icon: Plug,
  },
  plugins: {
    title: "插件管理",
    desc: "按包名管理 profile bundle 的启停与装卸，并同步上游更新。",
    Icon: Puzzle,
  },
  skills: {
    title: "技能管理",
    desc: "管理用户级技能根下的技能，启用或停用模型调用；改动由 dsh 热重载生效。",
    Icon: Sparkles,
  },
  settings: {
    title: "设置",
    desc: "镜像源、GitHub 凭据与窗口运行行为。",
    Icon: SettingsIcon,
  },
};

export default function App() {
  // 强制黑暗主题
  useEffect(() => {
    document.documentElement.classList.add("dark");
  }, []);

  // 应用版本号（来自 tauri.conf.json，构建时注入）
  const [version, setVersion] = useState("");
  useEffect(() => {
    getVersion()
      .then((v) => setVersion(`v${v}`))
      .catch(() => setVersion(""));
  }, []);

  // 当前打开的管理面板（null = 全部关闭；同一时刻只开一个）
  const [panel, setPanel] = useState<ShellPanel | null>(null);

  return (
    <>
      {/* 三栏布局骨架（布局/拖拽/响应式/持久化全在 AppShell） */}
      <AppShell
        version={version}
        activePanel={panel}
        onOpenPanel={(next) => setPanel(next)}
      />

      {/* 管理面板弹出子窗口（Dialog）：MCP / 插件 / 技能 / 设置
          四者宽度一致（48rem，修复前 384px 的两倍），窄窗口自动收缩 */}
      {(Object.keys(PANEL_META) as ShellPanel[]).map((key) => {
        const { title, desc, Icon } = PANEL_META[key];
        return (
          <Dialog
            key={key}
            open={panel === key}
            onOpenChange={(open) => setPanel(open ? key : null)}
          >
            <DialogContent className={PANEL_DIALOG_WIDTH}>
              <DialogHeader>
                <DialogTitle className="flex items-center gap-2">
                  <Icon className="size-4 text-muted-foreground" />
                  {title}
                </DialogTitle>
                <DialogDescription className="text-xs">{desc}</DialogDescription>
              </DialogHeader>
              {/* 主体独立滚动：长列表滚动时标题栏常驻可见 */}
              <DialogBody>
                {key === "mcp" && <McpPanel />}
                {key === "plugins" && <PluginsPanel />}
                {key === "skills" && <SkillsPanel />}
                {key === "settings" && <SettingsPanel />}
              </DialogBody>
            </DialogContent>
          </Dialog>
        );
      })}

      <Toaster theme="dark" position="top-right" />
    </>
  );
}
