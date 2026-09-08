// dsh-launcher 主界面：dark 主题
// 布局：三栏（左侧栏 状态+工具链 / 中央 版本管理 / 右侧 日志），骨架见 AppShell
// 管理入口：侧栏底部按钮组（插件 | 技能 | 设置）→ 对应 Dialog（状态在本组件管理）
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import AppShell, { type ShellPanel } from "@/components/AppShell";
import SettingsPanel from "@/components/SettingsPanel";
import PluginsPanel from "@/components/PluginsPanel";
import SkillsPanel from "@/components/SkillsPanel";
import { Toaster } from "@/components/ui/sonner";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** 各面板的标题与最大宽度（插件/技能内容更宽） */
const PANEL_META: Record<ShellPanel, { title: string; className: string }> = {
  plugins: {
    title: "插件管理",
    className: "max-h-[85vh] w-full max-w-2xl overflow-y-auto",
  },
  skills: {
    title: "技能共享",
    className: "max-h-[85vh] w-full max-w-xl overflow-y-auto",
  },
  settings: {
    title: "设置",
    className: "max-h-[85vh] w-full max-w-lg overflow-y-auto",
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

      {/* 管理面板弹出子窗口（Dialog）：插件 / 技能 / 设置 */}
      {(Object.keys(PANEL_META) as ShellPanel[]).map((key) => (
        <Dialog
          key={key}
          open={panel === key}
          onOpenChange={(open) => setPanel(open ? key : null)}
        >
          <DialogContent className={PANEL_META[key].className}>
            <DialogHeader>
              <DialogTitle>{PANEL_META[key].title}</DialogTitle>
            </DialogHeader>
            {key === "plugins" && <PluginsPanel />}
            {key === "skills" && <SkillsPanel />}
            {key === "settings" && <SettingsPanel />}
          </DialogContent>
        </Dialog>
      ))}

      <Toaster theme="dark" position="top-right" />
    </>
  );
}
