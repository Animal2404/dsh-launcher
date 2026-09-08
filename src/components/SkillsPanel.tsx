// 技能共享面板（ADR-0005）：
// - 共享真源固定为 ~/.agents/agent（skills/、AGENTS.md、CONTEXT.md）；
// - Mode L：把 ~/.dsh 下的同名资源链接到真源（零 dsh 配置）；
// - Mode C：无文件符号链接权限时，写 $DSH_HOME/cordis.patch.yml 的 shared 区块，
//   让 dsh 直接读真源（skill-filesystem.agentsHome / agent-instructions.dshHome）；
// - 冲突资源永不自动删除，只提供显式迁移（原文件改名保留）。
import { useCallback, useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import {
  listenSkillChanged,
  skillApply,
  skillMigrate,
  skillStatus,
  type MigrateReport,
  type ResourceState,
  type SkillStatus,
} from "@/lib/tauri";
import { toast } from "sonner";

function resourceVariant(state: ResourceState) {
  switch (state) {
    case "linked":
    case "config":
      return "default" as const;
    case "conflict":
      return "destructive" as const;
    case "broken":
      return "secondary" as const;
    default:
      return "outline" as const;
  }
}

function resourceLabel(state: ResourceState) {
  switch (state) {
    case "linked":
      return "已链接";
    case "config":
      return "配置共享";
    case "conflict":
      return "冲突";
    case "broken":
      return "链接异常";
    default:
      return "未建立";
  }
}

export default function SkillsPanel() {
  const [status, setStatus] = useState<SkillStatus | null>(null);
  const [mode, setMode] = useState<"auto" | "link" | "config">("auto");
  const [busy, setBusy] = useState<string | null>(null);
  const [report, setReport] = useState<MigrateReport | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await skillStatus();
      if (!mounted.current) return;
      setStatus(next);
      if (next.preferredMode === "link" || next.preferredMode === "config") {
        setMode(next.preferredMode);
      }
    } catch (e) {
      toast.error(`读取技能共享状态失败: ${e}`);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    let unlisten: (() => void) | undefined;
    listenSkillChanged(() => {
      refresh();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      mounted.current = false;
      unlisten?.();
    };
  }, [refresh]);

  async function run(key: string, action: () => Promise<string>) {
    setBusy(key);
    try {
      const message = await action();
      await refresh();
      if (message) toast.success(message);
    } catch (e) {
      toast.error(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }

  async function apply() {
    await run("apply", async () => {
      const result = await skillApply(mode);
      return result.message;
    });
  }

  async function migrate(dryRun: boolean) {
    await run(dryRun ? "migrate-dry" : "migrate", async () => {
      const result = await skillMigrate(dryRun);
      setReport(result);
      return dryRun ? "迁移预演完成" : "迁移完成";
    });
  }

  return (
    <div className="space-y-4">
      <div className="space-y-1 text-xs">
        <div className="flex items-center gap-1.5">
          <span className="text-muted-foreground">共享真源</span>
          <span className="truncate font-medium">
            {status?.canonicalRoot ?? "-"}
          </span>
        </div>
        <div className="text-muted-foreground">
          DSH_HOME {status?.dshHome ?? "-"}
        </div>
        <div className="flex flex-wrap items-center gap-1">
          <Badge variant="outline">
            链接能力 {status?.linkCapable ? "可用" : "不可用"}
          </Badge>
          <Badge variant="secondary">生效模式 {status?.activeMode ?? "-"}</Badge>
          <Badge variant="outline">技能 {status?.skillCount ?? 0} 个</Badge>
        </div>
      </div>

      <Separator />

      {/* 共享模式 */}
      <div className="space-y-2">
        <div className="text-sm font-medium">共享模式</div>
        <div className="flex items-center gap-1.5">
          <select
            className="w-40 rounded-md border bg-background px-1.5 py-1 text-xs"
            value={mode}
            onChange={(e) =>
              setMode(e.target.value as "auto" | "link" | "config")
            }
          >
            <option value="auto">自动（推荐）</option>
            <option value="link">链接（~/.dsh 指向真源）</option>
            <option value="config">配置（写 home patch）</option>
          </select>
          <Button
            variant="secondary"
            size="sm"
            disabled={busy !== null}
            onClick={apply}
          >
            应用
          </Button>
          <Button
            variant="ghost"
            size="sm"
            disabled={busy !== null}
            onClick={() => migrate(true)}
          >
            迁移预演
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy !== null}
            onClick={() => migrate(false)}
            title="把 ~/.dsh 下的真实文件/目录合并到真源后建立链接（原文件改名保留）"
          >
            执行迁移
          </Button>
        </div>
        <p className="text-[11px] text-muted-foreground">
          自动模式优先建立链接（目录用 junction 免特权，文件需开发者模式）；不可用时降级为
          配置模式，通过 $DSH_HOME/cordis.patch.yml 让 dsh 直接读取真源。
        </p>
      </div>

      <Separator />

      {/* 资源列表 */}
      <div className="space-y-2">
        {(status?.resources ?? []).map((item) => (
          <div
            key={item.resource}
            className="rounded-md border border-border/60 p-2 space-y-1"
          >
            <div className="flex items-center gap-1.5">
              <Badge variant={resourceVariant(item.state)}>
                {resourceLabel(item.state)}
              </Badge>
              <span className="text-xs font-medium">{item.resource}</span>
            </div>
            <div className="text-[11px] text-muted-foreground">
              真源 {item.canonical}
            </div>
            <div className="text-[11px] text-muted-foreground">
              视图 {item.view}
            </div>
            <div className="text-[11px] text-muted-foreground">{item.detail}</div>
          </div>
        ))}
      </div>

      {report && (
        <>
          <Separator />
          <div className="space-y-1 text-[11px]">
            <div className="text-xs font-medium">
              迁移{report.dryRun ? "预演" : "结果"}
            </div>
            {report.actions.map((item, index) => (
              <div key={`${item.resource}-${index}`} className="flex items-start gap-1.5">
                <Badge variant="outline">{item.action}</Badge>
                <span className="text-foreground">{item.resource}</span>
                <span className="text-muted-foreground">{item.detail}</span>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
