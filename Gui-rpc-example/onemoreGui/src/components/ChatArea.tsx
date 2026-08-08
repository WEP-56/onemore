import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "@/app/store";
import type { AssistantBlock, TranscriptItem } from "@/rpc/protocol";
import type { LiveStream } from "@/app/types";
import { formatTokens, phaseLabel } from "@/app/util";
import {
  Send,
  Square,
  ChevronDown,
  ChevronRight,
  Wrench,
  CheckCircle2,
  XCircle,
  Loader2,
  FolderOpen,
} from "lucide-react";
import { cn } from "@/lib/utils";

const RUNNING_PHASES = ["running", "retrying", "compacting", "waiting_approval"];

export default function ChatArea() {
  const conn = useStore((s) => s.conn);
  if (conn === "disconnected") return <WelcomeScreen />;
  return (
    <main className="flex min-w-0 flex-1 flex-col" style={{ background: "var(--surface-messages)" }}>
      <ChatHeader />
      <Transcript />
      <Composer />
    </main>
  );
}

function WelcomeScreen() {
  const workspaces = useStore((s) => s.workspaces);
  const connect = useStore((s) => s.connect);
  const loadSessions = useStore((s) => s.loadSessions);
  const lastError = useStore((s) => s.lastError);

  const handleConnect = async (path: string) => {
    await connect(path);
    await loadSessions();
  };

  return (
    <main
      className="flex min-w-0 flex-1 flex-col items-center justify-center"
      style={{ background: "var(--surface-messages)" }}
    >
      <div className="flex max-w-md flex-col items-center gap-3 px-10 py-8">
        <span
          className="inline-block h-7 w-7 rounded-full"
          style={{ background: "var(--status-success)", boxShadow: "0 0 20px var(--status-success)" }}
        />
        <h1 className="m-0 text-3xl font-semibold tracking-tight">OnemoreGui</h1>
        <p className="mb-4 text-sm text-[var(--text-faint)]">可靠的 Coding Agent — 选择工作区开始对话</p>

        {lastError && (
          <div
            className="flex flex-col gap-1 rounded-md px-3 py-2.5 text-[13px]"
            style={{
              border: "1px solid var(--status-error)",
              background: "rgba(255,110,110,0.08)",
              color: "var(--status-error)",
            }}
          >
            <span className="mono">{lastError.code}</span>
            <span>{lastError.message}</span>
          </div>
        )}

        <div className="flex w-full flex-col gap-2">
          {workspaces.length === 0 ? (
            <div
              className="border border-dashed px-5 py-5 text-center text-[13px] text-[var(--text-faint)]"
              style={{ borderColor: "var(--border-strong)", borderRadius: "var(--radius)" }}
            >
              还没有工作区。在左栏点击 + 添加一个项目目录。
            </div>
          ) : (
            workspaces.map((w) => (
              <button
                key={w.path}
                type="button"
                className="flex items-center gap-3 rounded-lg px-4 py-3 text-left transition-colors"
                style={{
                  border: "1px solid var(--border-subtle)",
                  background: "var(--surface-card)",
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.borderColor = "var(--status-success)";
                  e.currentTarget.style.background = "var(--surface-hover)";
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.borderColor = "var(--border-subtle)";
                  e.currentTarget.style.background = "var(--surface-card)";
                }}
                onClick={() => void handleConnect(w.path)}
              >
                <FolderOpen size={18} style={{ color: "var(--status-success)" }} />
                <div className="flex min-w-0 flex-col gap-0.5">
                  <span className="text-sm font-semibold">{w.label}</span>
                  <span className="mono truncate text-[11px] text-[var(--text-faint)]">{w.path}</span>
                </div>
              </button>
            ))
          )}
        </div>
      </div>
    </main>
  );
}

function ChatHeader() {
  const snapshot = useStore((s) => s.snapshot);
  const server = useStore((s) => s.server);
  const phase = snapshot?.phase ?? "idle";
  const usage = snapshot?.usage;
  const model = snapshot?.model;

  return (
    <header
      className="flex h-10 shrink-0 items-center gap-2.5 px-4 text-[13px]"
      style={{ background: "var(--surface-topbar)", borderBottom: "1px solid var(--border-subtle)" }}
    >
      {model && (
        <span className="max-w-[200px] truncate text-[var(--text-muted)]" title={model.label}>
          {model.label}
        </span>
      )}
      <span
        className="inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs"
        style={{
          border: "1px solid var(--border-strong)",
          background: "var(--surface-messages)",
          color: "var(--text-muted)",
        }}
      >
        <span
          className={cn("h-1.5 w-1.5 rounded-full", RUNNING_PHASES.includes(phase) && "animate-[dotPulse_1.2s_ease-in-out_infinite]")}
          style={{
            background: phase === "waiting_approval" ? "var(--status-warning)" : RUNNING_PHASES.includes(phase) ? "var(--status-success)" : "var(--text-faint)",
          }}
        />
        {phaseLabel(phase)}
      </span>
      {usage && (
        <span className="mono text-xs text-[var(--text-faint)]">
          in {formatTokens(usage.input_tokens)} · out {formatTokens(usage.output_tokens)}
        </span>
      )}
      <div className="flex-1" />
      {server && <span className="mono text-xs text-[var(--text-faint)]">rev {snapshot?.revision ?? 0}</span>}
    </header>
  );
}

function Transcript() {
  const snapshot = useStore((s) => s.snapshot);
  const liveStreams = useStore((s) => s.liveStreams);
  const liveTools = useStore((s) => s.liveTools);
  const liveUsers = useStore((s) => s.liveUsers);
  const liveNotices = useStore((s) => s.liveNotices);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);

  const nodes = useMemo(() => {
    const out: { key: string; node: React.ReactNode }[] = [];
    for (const item of snapshot?.transcript ?? [])
      out.push({ key: `t:${tk(item)}`, node: <ItemView item={item} /> });
    for (const u of Object.values(liveUsers))
      out.push({ key: `u:${u.key}`, node: <UserView text={u.text} /> });
    for (const n of liveNotices)
      out.push({ key: `n:${n.key}`, node: <NoticeView level={n.level} text={n.text} /> });
    const byMsg = new Map<string, LiveStream[]>();
    for (const s of Object.values(liveStreams)) {
      const arr = byMsg.get(s.messageId) ?? [];
      arr.push(s);
      byMsg.set(s.messageId, arr);
    }
    for (const [id, arr] of byMsg) {
      arr.sort((a, b) => a.contentIndex - b.contentIndex);
      const blocks: AssistantBlock[] = arr.map((s) =>
        s.kind === "thinking" ? { type: "thinking", text: s.text } : { type: "text", text: s.text },
      );
      out.push({ key: `s:${id}`, node: <AssistantView blocks={blocks} streaming={!arr.every((s) => s.sealed)} /> });
    }
    for (const t of Object.values(liveTools))
      out.push({ key: `lt:${t.toolCallId}`, node: <ToolLive tool={t} /> });
    return out;
  }, [snapshot, liveStreams, liveTools, liveUsers, liveNotices]);

  useEffect(() => {
    if (follow) scrollRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [nodes.length, follow]);

  return (
    <div
      className="flex flex-1 flex-col gap-1 overflow-y-auto py-4"
      onScroll={(e) => {
        const el = e.currentTarget;
        setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 160);
      }}
    >
      {nodes.length === 0 && (
        <div className="m-auto max-w-md px-5 py-5 text-center text-[var(--text-faint)]">
          输入消息开始对话。Onemore 会根据工作区内容自动理解上下文。
        </div>
      )}
      {nodes.map((n) => (
        <div key={n.key} className="px-6">
          {n.node}
        </div>
      ))}
      <div ref={scrollRef} />
    </div>
  );
}

function tk(item: TranscriptItem): string {
  switch (item.type) {
    case "user_message": return `user:${item.id}`;
    case "assistant_message": return `assistant:${item.id}`;
    case "tool": return `tool:${item.tool_call_id}`;
    case "notice": return `notice:${item.id}`;
  }
}

function ItemView({ item }: { item: TranscriptItem }) {
  switch (item.type) {
    case "user_message": return <UserView text={item.text} />;
    case "assistant_message": return <AssistantView blocks={item.blocks} streaming={false} />;
    case "tool": return <ToolView item={item} />;
    case "notice": return <NoticeView level={item.level} text={item.text} />;
  }
}

function UserView({ text }: { text: string }) {
  return (
    <div className="flex gap-3 py-2">
      <div
        className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full text-xs font-semibold"
        style={{ background: "var(--surface-card)", border: "1px solid var(--border-strong)", color: "var(--status-success)" }}
      >
        你
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5 pt-1">
        <div className="whitespace-pre-wrap break-words text-[var(--text-muted)]" style={{ lineHeight: 1.55 }}>
          {text}
        </div>
      </div>
    </div>
  );
}

function AssistantView({ blocks, streaming }: { blocks: AssistantBlock[]; streaming: boolean }) {
  return (
    <div className="flex gap-3 py-2">
      <div
        className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full"
        style={{ background: "rgba(120,235,190,0.12)", border: "1px solid var(--status-success)" }}
      >
        <span className="inline-block h-2.5 w-2.5 rounded-full" style={{ background: "var(--status-success)" }} />
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-1.5 pt-1">
        {blocks.length === 0 && streaming && <span className="inline-block h-4 w-0.5 animate-[caretPulse_1s_ease-in-out_infinite]" style={{ background: "var(--status-success)" }} />}
        {blocks.map((b, i) =>
          b.type === "text" ? (
            <div key={i} className="whitespace-pre-wrap break-words" style={{ lineHeight: 1.55 }}>
              {b.text}
              {streaming && i === blocks.length - 1 && (
                <span className="ml-0.5 inline-block h-4 w-0.5 animate-[caretPulse_1s_ease-in-out_infinite] align-[-2px]" style={{ background: "var(--status-success)" }} />
              )}
            </div>
          ) : b.type === "thinking" ? (
            <Thinking key={i} text={b.text} />
          ) : (
            <div key={i} className="inline-flex items-center gap-1.5 self-start rounded-full px-2.5 py-0.5 text-xs text-[var(--text-muted)]" style={{ border: "1px solid var(--border-strong)" }}>
              <Wrench size={12} />
              <span className="mono">{b.name}</span>
              <span className="text-[var(--text-faint)]">{b.summary}</span>
            </div>
          ),
        )}
      </div>
    </div>
  );
}

function Thinking({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  if (!text.trim()) return null;
  return (
    <div className="pl-2.5" style={{ borderLeft: "2px solid var(--border-strong)" }}>
      <button type="button" className="flex items-center gap-1 py-0.5 text-xs text-[var(--text-faint)] hover:text-[var(--text-muted)]" onClick={() => setOpen(!open)}>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
        思考 {text.length} 字
      </button>
      {open && <div className="mt-1 whitespace-pre-wrap text-[13px] text-[var(--text-muted)]">{text}</div>}
    </div>
  );
}

function ToolView({ item }: { item: Extract<TranscriptItem, { type: "tool" }> }) {
  const [open, setOpen] = useState(false);
  const ok = item.status === "succeeded";
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1.5 text-[13px]" style={{ border: "1px solid var(--border-subtle)", background: "var(--surface-card)" }}>
      <span style={{ color: ok ? "var(--status-success)" : "var(--status-error)" }}>
        {ok ? <CheckCircle2 size={14} /> : <XCircle size={14} />}
      </span>
      <span className="mono font-semibold">{item.name}</span>
      <span className="flex-1 truncate text-[var(--text-muted)]">{item.summary}</span>
      {item.output && (
        <button type="button" className="text-xs transition-colors hover:text-[var(--text-strong)]" style={{ color: "var(--status-success)" }} onClick={() => setOpen(!open)}>
          {open ? "收起" : "展开"}
        </button>
      )}
      {open && item.output && (
        <pre className="mono mt-1 w-full overflow-auto whitespace-pre-wrap break-words rounded p-2 text-xs" style={{ background: "var(--surface-messages)", border: "1px solid var(--border-subtle)", maxHeight: 240 }}>
          {item.output}
        </pre>
      )}
    </div>
  );
}

function ToolLive({ tool }: { tool: { toolCallId: string; name: string; summary: string; output: string; status: string; error: string | null } }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex flex-wrap items-center gap-2 rounded-md px-2.5 py-1.5 text-[13px]" style={{ border: "1px solid var(--border-subtle)", background: "var(--surface-card)" }}>
      <span style={{ color: tool.status === "finished" ? (tool.error ? "var(--status-error)" : "var(--status-success)") : "var(--status-success)" }}>
        {tool.status === "finished" ? (tool.error ? <XCircle size={14} /> : <CheckCircle2 size={14} />) : <Loader2 size={14} className="spin" />}
      </span>
      <span className="mono font-semibold">{tool.name}</span>
      <span className="flex-1 truncate text-[var(--text-muted)]">{tool.summary}</span>
      {tool.output && (
        <button type="button" className="text-xs" style={{ color: "var(--status-success)" }} onClick={() => setOpen(!open)}>
          {open ? "收起" : "展开"}
        </button>
      )}
      {open && tool.output && (
        <pre className="mono mt-1 w-full overflow-auto whitespace-pre-wrap break-words rounded p-2 text-xs" style={{ background: "var(--surface-messages)", border: "1px solid var(--border-subtle)", maxHeight: 240 }}>
          {tool.output}
        </pre>
      )}
      {tool.error && <div className="w-full text-xs" style={{ color: "var(--status-error)" }}>{tool.error}</div>}
    </div>
  );
}

function NoticeView({ level, text }: { level: string; text: string }) {
  return (
    <div
      className="rounded px-2.5 py-1 text-[13px]"
      style={{
        color: level === "error" ? "var(--status-error)" : level === "warning" ? "var(--status-warning)" : "var(--text-muted)",
        background: level === "error" ? "rgba(255,110,110,0.08)" : level === "warning" ? "rgba(255,175,85,0.08)" : "transparent",
      }}
    >
      {text}
    </div>
  );
}

function Composer() {
  const conn = useStore((s) => s.conn);
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");
  const draft = useStore((s) => s.draft);
  const setDraft = useStore((s) => s.setDraft);
  const busy = useStore((s) => s.busy);
  const sendPrompt = useStore((s) => s.sendPrompt);
  const sendSteer = useStore((s) => s.sendSteer);
  const sendAbort = useStore((s) => s.sendAbort);

  const connected = conn === "connected";
  const running = RUNNING_PHASES.includes(phase);

  const submit = () => {
    const text = draft.trim();
    if (!text || busy || !connected) return;
    setDraft("");
    void (running ? sendSteer(text) : sendPrompt(text));
  };

  return (
    <div className="flex shrink-0 flex-col gap-1.5 px-4 pb-2 pt-2.5" style={{ background: "var(--surface-composer)", borderTop: "1px solid var(--border-subtle)" }}>
      <div
        className="flex items-end gap-2 rounded-lg px-3.5 py-2 transition-colors"
        style={{ background: "var(--surface-messages)", border: "1px solid var(--border-strong)" }}
        onFocus={(e) => (e.currentTarget.style.borderColor = "var(--status-success)")}
        onBlur={(e) => (e.currentTarget.style.borderColor = "var(--border-strong)")}
      >
        <textarea
          className="flex-1 resize-none border-none bg-transparent text-[14px] outline-none placeholder:text-[var(--text-faint)]"
          style={{ lineHeight: 1.45, minHeight: 22, maxHeight: 160 }}
          rows={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          placeholder={!connected ? "请先连接工作区" : running ? "运行中：输入消息注入 steer…" : "输入消息开始对话…"}
          spellCheck={false}
        />
        <div className="flex shrink-0 gap-1">
          {running && (
            <button
              type="button"
              className="flex h-8 w-8 items-center justify-center rounded transition-colors hover:bg-[var(--surface-hover)]"
              style={{ color: "var(--status-error)" }}
              title="中止"
              onClick={() => void sendAbort()}
            >
              <Square size={15} />
            </button>
          )}
          <button
            type="button"
            className="flex h-8 w-8 items-center justify-center rounded text-black transition-colors disabled:opacity-40"
            style={{ background: "var(--primary)" }}
            disabled={!connected || !draft.trim() || busy}
            onClick={submit}
          >
            <Send size={15} />
          </button>
        </div>
      </div>
      <div className="text-center text-[11px] text-[var(--text-faint)]">
        {running ? "运行中 — Enter 发送 steer" : "Enter 发送 · Shift+Enter 换行"}
      </div>
    </div>
  );
}
