// MCP server 管理面板（ADR-0006 Part A）：
// - 列出**合成树全量** MCP server（含用户手写 / bundle / profile patch 的声明）；
// - 启停写 `$DSH_HOME/cordis.patch.yml` 受管 MCP 区块的**定向段**（只碰数行，
//   `config` 大块字节可证不被触碰），由 dsh live 就地热重载 → **不重启 dsh**；
// - 删除语义二分：`managed` 真删除（声明 + 定向），`external` 仅撤销定向覆盖
//   （**声明仍在**，服务器恢复默认启用）——文案必须区分，否则用户会误判；
// - `failOnStartupError: true` 会让整个 harness 启动中止 → 打「危险」徽章；
// - 目标行 `disabled` 为 `!!js` 表达式 → 只读（启动器拒绝覆盖）。
//
// 结构照 PluginsPanel 同构（refresh / run(key, action) / busy / toast / 事件订阅）；
// remove 复用 ToolchainPanel 的现有确认弹窗模式。**不抽共享 hook、不回改既有面板**（D14）。
import { useCallback, useEffect, useRef, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  listenMcpChanged,
  mcpAdd,
  mcpList,
  mcpRemove,
  mcpSetState,
  type McpListResult,
  type McpMark,
  type McpServerView,
  type McpTransport,
} from "@/lib/tauri";
import { toast } from "sonner";

/** 状态徽章配色 */
function stateVariant(state: McpServerView["state"]) {
  switch (state) {
    case "enabled":
      return "default" as const;
    case "disabled":
      return "secondary" as const;
    default:
      return "destructive" as const;
  }
}

function stateLabel(state: McpServerView["state"]) {
  switch (state) {
    case "enabled":
      return "已启用";
    case "disabled":
      return "已禁用";
    default:
      return "缺失";
  }
}

function originLabel(origin: McpServerView["origin"]) {
  return origin === "managed" ? "受管声明" : "外部声明";
}

function markLabel(mark: McpMark) {
  switch (mark) {
    case "expression":
      return "表达式";
    case "conflict":
      return "冲突";
    case "dangerous":
      return "危险";
  }
}

function markVariant(mark: McpMark) {
  switch (mark) {
    case "dangerous":
      return "destructive" as const;
    case "conflict":
      return "destructive" as const;
    default:
      return "outline" as const;
  }
}

const MARK_TITLE: Record<McpMark, string> = {
  expression:
    "disabled 由 !!js 表达式控制：启动器拒绝覆盖，启停已禁用（只读）",
  conflict:
    "serverName 与合成树中其它行重复：官方在加载期判后加载的实例失败，请先处理重复声明",
  dangerous:
    "failOnStartupError: true：初始连接失败会让整个 harness 启动中止；如需隔离请改为禁用",
};

export default function McpPanel() {
  const [data, setData] = useState<McpListResult | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [pendingRemove, setPendingRemove] = useState<McpServerView | null>(null);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  // 新增表单（结构化官方字段 + 原始 YAML 通道）
  const [form, setForm] = useState({
    serverName: "",
    transport: "stdio" as McpTransport,
    command: "",
    args: "",
    cwd: "",
    url: "",
    headerRows: "",
    startDisabled: false,
    rawConfig: "",
    useRaw: false,
  });
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const list = await mcpList();
      if (mounted.current) {
        setData(list);
        setLoadError(null);
      }
    } catch (e) {
      // dump 解析失败 / marker 冲突等 → 降级为只读（不返回部分结果）
      if (mounted.current) setLoadError(String(e));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    refresh();
    let unlisten: (() => void) | undefined;
    listenMcpChanged(() => {
      refresh();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      mounted.current = false;
      unlisten?.();
    };
  }, [refresh]);

  /** 统一封装：占用 busy 标记 → 执行 → 刷新 → 提示 */
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

  async function toggle(item: McpServerView, next: boolean) {
    await run(`toggle:${item.serverName}`, async () => {
      const result = await mcpSetState(item.serverName, next);
      return result.status === "unchanged" ? "" : result.message;
    });
  }

  async function confirmRemove() {
    const item = pendingRemove;
    if (!item) return;
    setPendingRemove(null);
    await run(`remove:${item.serverName}`, async () => {
      const result = await mcpRemove(item.serverName);
      return result.status === "unchanged" ? "" : result.message;
    });
  }

  async function add() {
    const serverName = form.serverName.trim();
    if (!serverName) {
      toast.info("请填写 serverName（面向模型的命名空间）");
      return;
    }
    const raw = form.rawConfig.trim();
    if (form.useRaw && !raw) {
      toast.info("原始 YAML 通道需要填入官方 config 体片段");
      return;
    }
    await run("add", async () => {
      const result = await mcpAdd({
        serverName,
        transport: form.transport,
        startDisabled: form.startDisabled,
        command: form.useRaw ? null : form.command.trim() || null,
        args: form.useRaw
          ? []
          : form.args
              .split(/\s+/)
              .map((item) => item.trim())
              .filter(Boolean),
        cwd: form.useRaw ? null : form.cwd.trim() || null,
        url: form.useRaw ? null : form.url.trim() || null,
        headers: form.useRaw ? [] : parsePairs(form.headerRows),
        rawConfig: form.useRaw ? raw : null,
      });
      setForm((prev) => ({
        ...prev,
        serverName: "",
        command: "",
        args: "",
        cwd: "",
        url: "",
        headerRows: "",
        rawConfig: "",
      }));
      return result.message;
    });
  }

  const degraded = loadError !== null;
  const servers = data?.servers ?? [];
  const prereqMissing = data ? !data.prereq.installed : false;
  const needsRestart = data?.reload === "requires-restart";
  const writable = !degraded && !prereqMissing;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-2">
        <div className="text-xs text-muted-foreground">
          profile <span className="font-medium">{data?.profile ?? "web"}</span> ·{" "}
          {servers.length} 个 MCP server · 变更
          {needsRestart ? "需重启 dsh 生效" : "就地热重载（不重启 dsh）"}
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={busy !== null}
          onClick={() => refresh()}
        >
          刷新
        </Button>
      </div>

      {degraded && (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 p-2 text-xs text-destructive">
          读取合成树失败：{loadError}（已降级为只读，全部写操作已禁用）
        </div>
      )}

      {prereqMissing && (
        <div className="rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-xs">
          前置包 <span className="font-medium">{data?.prereq.package}</span>{" "}
          不可解析：新增会被拒绝。请到<strong>插件</strong>页安装该官方插件后重试。
        </div>
      )}

      {needsRestart && (
        <div className="rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-xs">
          当前 profile 的 <code>patchReload</code> 不是 <code>live</code>
          ，改动需重启 dsh 才会生效。
        </div>
      )}

      {/* 新增 */}
      <div className="space-y-2">
        <Label className="text-xs">新增 MCP server（官方字段）</Label>
        <div className="flex gap-1.5">
          <Input
            value={form.serverName}
            onChange={(e) => setForm({ ...form, serverName: e.target.value })}
            placeholder="serverName（^[A-Za-z0-9_-]{1,32}$）"
            className="text-xs"
          />
          <select
            className="w-36 shrink-0 rounded-md border bg-background px-1.5 text-xs"
            value={form.transport}
            onChange={(e) =>
              setForm({ ...form, transport: e.target.value as McpTransport })
            }
          >
            <option value="stdio">stdio</option>
            <option value="streamable-http">streamable-http</option>
          </select>
          <label className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
            <input
              type="checkbox"
              checked={form.useRaw}
              onChange={(e) => setForm({ ...form, useRaw: e.target.checked })}
            />
            原始 YAML
          </label>
          <label className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
            <input
              type="checkbox"
              checked={form.startDisabled}
              onChange={(e) =>
                setForm({ ...form, startDisabled: e.target.checked })
              }
            />
            声明为禁用
          </label>
        </div>

        {form.useRaw ? (
          <textarea
            value={form.rawConfig}
            onChange={(e) => setForm({ ...form, rawConfig: e.target.value })}
            placeholder={
              "官方 config 体原始片段（保 !!js / 注释 / 未建模字段）\nserverName: my-srv\ntransport: stdio\ncommand: my-mcp"
            }
            className="h-28 w-full rounded-md border bg-background p-2 font-mono text-[11px]"
          />
        ) : form.transport === "stdio" ? (
          <div className="flex gap-1.5">
            <Input
              value={form.command}
              onChange={(e) => setForm({ ...form, command: e.target.value })}
              placeholder="command（必填）"
              className="text-xs"
            />
            <Input
              value={form.args}
              onChange={(e) => setForm({ ...form, args: e.target.value })}
              placeholder="args（空格分隔）"
              className="text-xs"
            />
            <Input
              value={form.cwd}
              onChange={(e) => setForm({ ...form, cwd: e.target.value })}
              placeholder="cwd（可选）"
              className="text-xs"
            />
          </div>
        ) : (
          <div className="flex gap-1.5">
            <Input
              value={form.url}
              onChange={(e) => setForm({ ...form, url: e.target.value })}
              placeholder="url（必填）"
              className="text-xs"
            />
            <Input
              value={form.headerRows}
              onChange={(e) => setForm({ ...form, headerRows: e.target.value })}
              placeholder="headers（每行 K=V）"
              className="text-xs"
            />
          </div>
        )}

        <div className="flex items-center justify-between gap-2">
          <p className="text-[11px] text-muted-foreground">
            含 <code>!!js</code> 表达式或未知字段时请用原始 YAML 通道；
            <code>failOnStartupError</code> 属危险字段，结构化通道不暴露（仅原始通道可表达）。
          </p>
          <Button
            variant="secondary"
            size="sm"
            disabled={!writable || busy !== null}
            onClick={add}
          >
            添加
          </Button>
        </div>
      </div>

      <Separator />

      {/* 列表 */}
      <div className="space-y-2">
        {servers.length === 0 && (
          <p className="text-xs text-muted-foreground">
            合成树中尚无 MCP server 行。
          </p>
        )}
        {servers.map((item) => {
          const canToggle =
            writable &&
            (item.state === "enabled" || item.state === "disabled") &&
            !item.marks.includes("expression");
          const isOpen = expanded[item.serverName] === true;
          return (
            <div
              key={item.serverName}
              className="rounded-md border border-border/60 p-2 space-y-1.5"
            >
              <div className="flex items-center justify-between gap-2">
                <div className="min-w-0">
                  <button
                    type="button"
                    className="truncate text-left text-xs font-medium hover:underline"
                    onClick={() =>
                      setExpanded((prev) => ({
                        ...prev,
                        [item.serverName]: !isOpen,
                      }))
                    }
                    title="展开详情"
                  >
                    {item.serverName}
                  </button>
                  <div className="mt-1 flex flex-wrap items-center gap-1">
                    <Badge variant={stateVariant(item.state)}>
                      {stateLabel(item.state)}
                    </Badge>
                    {item.transport && (
                      <Badge variant="outline">{item.transport}</Badge>
                    )}
                    <Badge variant="outline">{originLabel(item.origin)}</Badge>
                    {item.marks.map((mark) => (
                      <Badge
                        key={mark}
                        variant={markVariant(mark)}
                        title={MARK_TITLE[mark]}
                      >
                        {markLabel(mark)}
                      </Badge>
                    ))}
                  </div>
                </div>
                <div className="flex shrink-0 items-center gap-2">
                  <Switch
                    size="sm"
                    checked={item.state === "enabled"}
                    disabled={!canToggle || busy !== null}
                    onCheckedChange={(value) => toggle(item, value === true)}
                    title={
                      canToggle
                        ? "启用/禁用（只改定向行；dsh 热重载，无需重启）"
                        : item.marks.includes("expression")
                          ? "disabled 由 !!js 表达式控制，启动器拒绝覆盖"
                          : "该状态不支持启停"
                    }
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={busy !== null || item.state === "missing"}
                    onClick={() => setPendingRemove(item)}
                  >
                    删除
                  </Button>
                </div>
              </div>

              {item.marks.includes("dangerous") && (
                <div className="text-[11px] text-destructive">
                  failOnStartupError: true —— 初始连接失败会让整个 harness
                  启动中止；如需隔离请用启用/禁用（官方唯一安全手段）。
                </div>
              )}

              {isOpen && (
                <div className="space-y-1 border-t border-border/50 pt-1.5 text-[11px] text-muted-foreground">
                  <div>
                    摘要: <span className="text-foreground">{item.summary}</span>
                  </div>
                  <div>
                    行 id: <span className="text-foreground">{item.rowId}</span>
                  </div>
                  <div className="break-all">
                    来源层: <span className="text-foreground">{item.layer}</span>
                  </div>
                  <div>
                    有效 disabled:{" "}
                    <span className="text-foreground">
                      {item.disabled === null
                        ? "由 !!js 表达式控制（只读）"
                        : String(item.disabled)}
                    </span>
                  </div>
                  <div>
                    {item.origin === "managed"
                      ? "受管声明：删除会同时移除声明与定向覆盖（服务器从合成树消失）"
                      : "外部声明：删除只撤销启动器的定向覆盖，声明仍由上方来源层提供"}
                  </div>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {/* 删除确认对话框（复用 ToolchainPanel 的现有模式；文案区分 managed/external） */}
      <Dialog
        open={pendingRemove !== null}
        onOpenChange={(open) => {
          if (!open) setPendingRemove(null);
        }}
      >
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>
              {pendingRemove?.origin === "managed"
                ? `删除受管 MCP server ${pendingRemove?.serverName}？`
                : `撤销 ${pendingRemove?.serverName} 的定向覆盖？`}
            </DialogTitle>
            <DialogDescription>
              {pendingRemove?.origin === "managed" ? (
                <>
                  将从 <code>$DSH_HOME/cordis.patch.yml</code>{" "}
                  的受管 MCP 区块中删除该行的**声明与定向覆盖**，服务器随即从合成树消失。
                  dsh 会就地热重载，无需重启。
                </>
              ) : (
                <>
                  该行**不是启动器声明的**（来源层：{" "}
                  <span className="break-all">{pendingRemove?.layer}</span>）。
                  删除只会撤销启动器的定向覆盖，**声明仍在**，服务器恢复默认启用状态；
                  启动器不会改动用户手写内容。
                </>
              )}
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              variant="outline"
              size="sm"
              onClick={() => setPendingRemove(null)}
            >
              取消
            </Button>
            <Button
              variant="destructive"
              size="sm"
              disabled={busy !== null}
              onClick={confirmRemove}
            >
              {pendingRemove?.origin === "managed" ? "确认删除" : "确认撤销覆盖"}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

/** 解析「每行 K=V」的键值输入 */
function parsePairs(text: string): [string, string][] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const index = line.indexOf("=");
      if (index < 0) return [line, ""] as [string, string];
      return [line.slice(0, index).trim(), line.slice(index + 1).trim()] as [
        string,
        string,
      ];
    });
}
