// 自绘窗口控制按钮（无边框窗口的主标题栏右侧）：最小化 / 最大化(还原) / 关闭
// 权限：capabilities/default.json 已补 allow-minimize / toggle-maximize / close / is-maximized
import { useEffect, useRef, useState } from "react";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { getCurrentWindow, type Window } from "@tauri-apps/api/window";
import { Button } from "@/components/ui/button";
import { Minus, Square, X } from "lucide-react";

export default function WindowControls() {
  const [maximized, setMaximized] = useState(false);
  // 惰性获取窗口句柄：非 Tauri 环境（浏览器预览）getCurrentWindow() 会抛错，此时按钮不可用但不崩溃
  const winRef = useRef<Window | null>(null);

  // 挂载后获取窗口句柄 + 初始最大化状态
  useEffect(() => {
    try {
      const win = getCurrentWindow();
      winRef.current = win;
      win.isMaximized().then(setMaximized).catch(() => {});
    } catch {
      // 非 Tauri 环境（浏览器预览）忽略：按钮保持禁用态不可用
    }
  }, []);

  // 尺寸变化 → 刷新最大化状态（最大化/还原切换都会触发）。
  // 统一走 useTauriEvent（ADR-0009 D7）：此前 unlisten 在 async IIFE 内赋值，
  // cleanup 可能先于赋值执行 → dev 双挂载/早卸载时 onResized 监听泄漏。
  // 非 Tauri 环境下 getCurrentWindow() 同步抛错，由封装捕获并记录（不崩溃、不泄漏）。
  useTauriEvent<unknown>(
    (onPayload) =>
      getCurrentWindow().onResized((event) => {
        onPayload(event.payload);
      }),
    () => {
      const win = winRef.current;
      if (!win) return;
      win.isMaximized().then(setMaximized).catch(() => {});
    },
    [],
  );

  // 统一窗口操作：句柄未就绪时静默返回
  const act = (fn: (w: Window) => Promise<void>) => () => {
    const w = winRef.current;
    if (!w) return;
    fn(w).catch(() => {});
  };

  return (
    <div className="flex items-center">
      <Button
        variant="ghost"
        size="icon-sm"
        title="最小化"
        aria-label="最小化窗口"
        onClick={act((w) => w.minimize())}
      >
        <Minus className="size-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        title={maximized ? "还原" : "最大化"}
        aria-label={maximized ? "还原窗口" : "最大化窗口"}
        onClick={act((w) => w.toggleMaximize())}
      >
        {/* 还原语义：用 Minus+Square 组合示意「双窗叠加」的 Windows 还原语义，
            此前借用 lucide `Copy`（复制）图标，语义不符（ADR-0009 D19） */}
        {maximized ? (
          <span className="relative inline-block size-3">
            <Square className="absolute right-0 bottom-0 size-2.5" />
            <Minus className="absolute top-0 left-0 size-2.5" />
          </span>
        ) : (
          <Square className="size-3" />
        )}
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        title="关闭"
        aria-label="关闭窗口"
        onClick={act((w) => w.close())}
      >
        <X className="size-3.5" />
      </Button>
    </div>
  );
}