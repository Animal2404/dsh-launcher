// Token 统计面板：托管 TokenTracker（tokentracker-cli）并内嵌其本地看板
// - 顶部：紧凑工具条（状态徽标 + 安装/启动/停止/独立窗口/刷新），无卡片外壳
// - 主区：iframe 内嵌 http://127.0.0.1:<port> 看板（36 个 AI 工具统一 token 视图）
import { useCallback, useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { toast } from "sonner";
import {
  Activity,
  ExternalLink,
  Loader2,
  Play,
  Square,
  Wrench,
  ChartPie,
} from "lucide-react";
import {
  detectTokentrackerCli,
  getTokentrackerPort,
  getTokentrackerStatus,
  installTokentrackerCli,
  openTokentrackerDashboard,
  startTokentracker,
  stopTokentracker,
  type TokentrackerStatus,
} from "@/lib/tauri";

const STATUS_META: Record<
  TokentrackerStatus,
  { label: string; variant: "default" | "secondary" | "destructive" | "outline" }
> = {
  stopped: { label: "未运行", variant: "outline" },
  starting: { label: "启动中", variant: "secondary" },
  running: { label: "运行中", variant: "default" },
  stopping: { label: "停止中", variant: "secondary" },
  error: { label: "出错", variant: "destructive" },
};

export default function TokenPanel() {
  const [status, setStatus] = useState<TokentrackerStatus>("stopped");
  const [port, setPort] = useState(0);
  const [cliDetected, setCliDetected] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);
  const [iframeKey, setIframeKey] = useState(0);
  const pollRef = useRef<number | null>(null);

  // 轮询状态 + 端口
  const refresh = useCallback(async () => {
    try {
      const [s, p] = await Promise.all([
        getTokentrackerStatus(),
        getTokentrackerPort(),
      ]);
      setStatus(s);
      setPort(p);
      if (s === "running" && cliDetected === null) {
        detectTokentrackerCli().then(setCliDetected).catch(() => {});
      }
    } catch {
      // 忽略瞬时错误
    }
  }, [cliDetected]);

  useEffect(() => {
    void refresh();
    detectTokentrackerCli()
      .then(setCliDetected)
      .catch(() => setCliDetected(false));
    pollRef.current = window.setInterval(() => void refresh(), 2000);
    return () => {
      if (pollRef.current !== null) window.clearInterval(pollRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleInstall = async () => {
    setBusy(true);
    try {
      const msg = await installTokentrackerCli();
      toast.success(msg);
      setCliDetected(await detectTokentrackerCli());
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleStart = async () => {
    setBusy(true);
    try {
      const p = await startTokentracker();
      setPort(p);
      setIframeKey((k) => k + 1);
      toast.success(`TokenTracker 看板已启动（端口 ${p}）`);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleStop = async () => {
    setBusy(true);
    try {
      const msg = await stopTokentracker();
      toast.success(msg);
      setPort(0);
      await refresh();
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleOpenWindow = async () => {
    try {
      await openTokentrackerDashboard();
    } catch (e) {
      toast.error(String(e));
    }
  };

  const dashboardUrl = port ? `http://127.0.0.1:${port}` : "";
  const running = status === "running";

  return (
    <div className="flex h-full flex-col overflow-hidden">
      {/* 紧凑工具条（无卡片外壳）：状态徽标 + 控制按钮 */}
      <div className="flex items-center gap-2 border-b px-3 py-1.5">
        <ChartPie className="size-3.5 text-primary" />
        <Badge variant={STATUS_META[status].variant}>
          {STATUS_META[status].label}
        </Badge>
        {cliDetected === false && (
          <Badge variant="destructive">tokentracker-cli 未安装</Badge>
        )}
        {port > 0 && <Badge variant="secondary">端口 {port}</Badge>}
        <div className="ml-auto flex items-center gap-1">
          {!running ? (
            <>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void handleStart()}
                disabled={busy || cliDetected === false}
              >
                {busy ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <Play className="size-3.5" />
                )}
                启动看板
              </Button>
              {cliDetected === false && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={() => void handleInstall()}
                  disabled={busy}
                >
                  <Wrench className="size-3.5" /> 安装
                </Button>
              )}
            </>
          ) : (
            <>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => void handleStop()}
                disabled={busy}
              >
                {busy ? (
                  <Loader2 className="size-3.5 animate-spin" />
                ) : (
                  <Square className="size-3.5" />
                )}
                停止
              </Button>
              <Button size="sm" variant="ghost" onClick={() => void handleOpenWindow()}>
                <ExternalLink className="size-3.5" /> 独立窗口
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => setIframeKey((k) => k + 1)}
              >
                <Activity className="size-3.5" /> 刷新
              </Button>
            </>
          )}
        </div>
      </div>

      {/* 看板区：iframe 占满剩余空间 */}
      <div className="min-h-0 flex-1 bg-background">
        {running && dashboardUrl ? (
          <iframe
            key={iframeKey}
            src={dashboardUrl}
            title="TokenTracker 看板"
            className="h-full w-full"
            onError={() => toast.error("看板加载失败", { id: "dash-err" })}
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-muted-foreground">
            <ChartPie className="size-10 opacity-40" />
            <p className="text-sm">
              {cliDetected === false
                ? "未检测到 tokentracker-cli，请先安装后启动看板"
                : "TokenTracker 未运行，点击「启动看板」查看所有 AI 工具的 token 统计"}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
