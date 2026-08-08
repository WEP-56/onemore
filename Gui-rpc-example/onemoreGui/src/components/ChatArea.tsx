import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useStore } from "@/app/store";
import type { AssistantBlock, TranscriptItem } from "@/rpc/protocol";
import type { LiveStream } from "@/app/types";
import {
  ChevronDown,
  ChevronRight,
  Wrench,
  CheckCircle2,
  XCircle,
  Loader2,
  FolderOpen,
  Copy,
  Check,
  ArrowDown,
  Brain,
  Info,
  AlertTriangle,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Markdown } from "@/components/Markdown";
import Composer from "@/components/Composer";

export default function ChatArea() {
  const conn = useStore((s) => s.conn);
  if (conn === "disconnected") return <WelcomeScreen />;
  return (
    <main className="flex min-w-0 flex-1 flex-col" style={{ background: "var(--surface-messages)" }}>
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

function Transcript() {
  const snapshot = useStore((s) => s.snapshot);
  const liveStreams = useStore((s) => s.liveStreams);
  const liveTools = useStore((s) => s.liveTools);
  const liveUsers = useStore((s) => s.liveUsers);
  const liveNotices = useStore((s) => s.liveNotices);
  const scrollRef = useRef<HTMLDivElement>(null);
  const shellRef = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);
  const [showScrollBtn, setShowScrollBtn] = useState(false);

  const nodes = useMemo(() => {
    const out: { key: string; node: ReactNode }[] = [];
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
    if (follow && nodes.length > 0) {
      scrollRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
    }
  }, [nodes.length, follow, nodes[nodes.length - 1]?.key]);

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const el = e.currentTarget;
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 160;
    setFollow(nearBottom);
    setShowScrollBtn(!nearBottom);
  };

  const scrollToBottom = () => {
    setFollow(true);
    scrollRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  };

  return (
    <div className="messages-shell">
      <div className="messages-scroll" ref={shellRef} onScroll={handleScroll}>
        <div className="messages-inner">
          {nodes.length === 0 && (
            <div className="m-auto max-w-md px-5 py-16 text-center text-[var(--text-faint)]">
              输入消息开始对话。Onemore 会根据工作区内容自动理解上下文。
            </div>
          )}
          {nodes.map((n) => (
            <div key={n.key}>{n.node}</div>
          ))}
          <div ref={scrollRef} />
        </div>
      </div>
      <button
        type="button"
        className={cn("scroll-bottom-btn", showScrollBtn && "is-visible")}
        title="回到底部"
        onClick={scrollToBottom}
      >
        <ArrowDown size={15} />
      </button>
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
    <div className="message user">
      <div className="message-body">
        <div className="message-bubble-user">{text}</div>
      </div>
    </div>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="message-action-button"
      title="复制"
      onClick={() => {
        void navigator.clipboard.writeText(text).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        });
      }}
    >
      {copied ? <Check size={13} className="text-[var(--status-success)]" /> : <Copy size={13} />}
    </button>
  );
}

function AssistantView({ blocks, streaming }: { blocks: AssistantBlock[]; streaming: boolean }) {
  const plainText = useMemo(
    () =>
      blocks
        .filter((b) => b.type === "text")
        .map((b) => (b.type === "text" ? b.text : ""))
        .join("\n"),
    [blocks],
  );

  return (
    <div className="message assistant">
      <div className="message-assistant">
        <div className="message-avatar assistant">
          <span className="inline-block h-2.5 w-2.5 rounded-full" style={{ background: "var(--status-success)" }} />
        </div>
        <div className="message-assistant-content">
          {blocks.length === 0 && streaming && (
            <span className="inline-block h-4 w-0.5 animate-[caretPulse_1s_ease-in-out_infinite]" style={{ background: "var(--status-success)" }} />
          )}
          {blocks.map((b, i) => {
            if (b.type === "thinking") return <ThinkingBlock key={i} text={b.text} live={streaming && i === blocks.length - 1} />;
            if (b.type === "tool_call") {
              return (
                <div key={i} className="tool-card">
                  <div className="tool-card-header" style={{ cursor: "default" }}>
                    <span className="tool-card-status" style={{ color: "var(--status-success)" }}>
                      <Wrench size={13} />
                    </span>
                    <span className="tool-card-name">{b.name}</span>
                    <span className="tool-card-summary">{b.summary}</span>
                  </div>
                </div>
              );
            }
            return (
              <div key={i}>
                <Markdown value={b.text} />
                {streaming && i === blocks.length - 1 && (
                  <span className="ml-0.5 inline-block h-4 w-0.5 animate-[caretPulse_1s_ease-in-out_infinite] align-[-2px]" style={{ background: "var(--status-success)" }} />
                )}
              </div>
            );
          })}
          {plainText && (
            <div className="message-action-bar">
              <CopyButton text={plainText} />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function ThinkingBlock({ text, live }: { text: string; live: boolean }) {
  const [open, setOpen] = useState(false);
  const trimmed = text.trim();
  if (!trimmed) return null;
  const isLive = live && !open;
  return (
    <div className={cn("thinking-block", open && "is-expanded", live && "is-live")}>
      <button
        type="button"
        className="thinking-header"
        onClick={() => setOpen(!open)}
      >
        <Brain size={14} className="shrink-0" style={{ color: "var(--text-subtle)" }} />
        <span className="thinking-title">
          {isLive ? "思考中…" : open ? "收起思考" : `思考 ${trimmed.length} 字`}
        </span>
        {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
      </button>
      {open && (
        <div className="thinking-content">
          <div className="thinking-content-inner">
            <Markdown value={trimmed} />
          </div>
        </div>
      )}
    </div>
  );
}

function ToolView({ item }: { item: Extract<TranscriptItem, { type: "tool" }> }) {
  const [open, setOpen] = useState(false);
  const ok = item.status === "succeeded";
  return (
    <div className={cn("tool-card", !ok && "is-failed")}>
      <div className="tool-card-header" onClick={() => item.output && setOpen(!open)}>
        <span className="tool-card-status" style={{ color: ok ? "var(--status-success)" : "var(--status-error)" }}>
          {ok ? <CheckCircle2 size={14} /> : <XCircle size={14} />}
        </span>
        <span className="tool-card-name">{item.name}</span>
        {item.summary && <span className="tool-card-summary">{item.summary}</span>}
        {item.output && (
          <ChevronDown size={13} className={cn("tool-card-chevron", open && "is-open")} />
        )}
      </div>
      {open && item.output && (
        <div className="tool-card-output">
          <pre>{item.output}</pre>
        </div>
      )}
    </div>
  );
}

function ToolLive({ tool }: { tool: { toolCallId: string; name: string; summary: string; output: string; status: string; error: string | null } }) {
  const [open, setOpen] = useState(false);
  const done = tool.status === "finished";
  const failed = done && Boolean(tool.error);
  return (
    <div className={cn("tool-card", failed && "is-failed")}>
      <div className="tool-card-header" onClick={() => tool.output && setOpen(!open)}>
        <span className="tool-card-status" style={{ color: failed ? "var(--status-error)" : "var(--status-success)" }}>
          {failed ? <XCircle size={14} /> : done ? <CheckCircle2 size={14} /> : <Loader2 size={14} className="spin" />}
        </span>
        <span className="tool-card-name">{tool.name}</span>
        {tool.summary && <span className="tool-card-summary">{tool.summary}</span>}
        {tool.output && (
          <ChevronDown size={13} className={cn("tool-card-chevron", open && "is-open")} />
        )}
      </div>
      {open && tool.output && (
        <div className="tool-card-output">
          <pre>{tool.output}</pre>
        </div>
      )}
      {tool.error && <div className="tool-card-error">{tool.error}</div>}
    </div>
  );
}

function NoticeView({ level, text }: { level: string; text: string }) {
  return (
    <div className={cn("notice-row", level)}>
      {level === "error" ? (
        <AlertTriangle size={13} className="mt-0.5 shrink-0" />
      ) : level === "warning" ? (
        <AlertTriangle size={13} className="mt-0.5 shrink-0" />
      ) : (
        <Info size={13} className="mt-0.5 shrink-0" />
      )}
      <span>{text}</span>
    </div>
  );
}
