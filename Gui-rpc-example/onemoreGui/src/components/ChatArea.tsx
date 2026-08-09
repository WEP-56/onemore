import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useStore } from "@/app/store";
import { open } from "@tauri-apps/plugin-dialog";
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
import BrandMark from "@/components/BrandMark";

export default function ChatArea() {
  const conn = useStore((s) => s.conn);
  const snapshot = useStore((s) => s.snapshot);
  const liveStreams = useStore((s) => s.liveStreams);
  const liveTools = useStore((s) => s.liveTools);
  const liveUsers = useStore((s) => s.liveUsers);
  const liveNotices = useStore((s) => s.liveNotices);
  if (conn === "disconnected") return <WelcomeScreen />;

  const hasConversation = Boolean(
    snapshot?.transcript.length ||
      Object.keys(liveStreams).length ||
      Object.keys(liveTools).length ||
      Object.keys(liveUsers).length ||
      liveNotices.length,
  );

  if (!hasConversation) return <NewConversationScreen />;

  return (
    <main className="chat-workspace">
      <Transcript />
      <Composer variant="docked" />
    </main>
  );
}

function NewConversationScreen() {
  const workspaces = useStore((s) => s.workspaces);
  const activeWorkspace = useStore((s) => s.activeWorkspace);
  const label = workspaces.find((workspace) => workspace.path === activeWorkspace)?.label ?? "当前工作区";

  return (
    <main className="new-conversation">
      <div className="new-conversation-center">
        <div className="new-conversation-title">
          <BrandMark />
          <h1>创造任何东西</h1>
        </div>
        <Composer variant="home" />
        <div className="new-conversation-workspace">
          <FolderOpen size={13} />
          <span>{label}</span>
        </div>
      </div>
    </main>
  );
}

function WelcomeScreen() {
  const workspaces = useStore((s) => s.workspaces);
  const connect = useStore((s) => s.connect);
  const loadSessions = useStore((s) => s.loadSessions);
  const addWorkspace = useStore((s) => s.addWorkspace);
  const lastError = useStore((s) => s.lastError);

  const handleConnect = async (path: string) => {
    await connect(path);
    await loadSessions();
  };

  const handleAddWorkspace = async () => {
    const dir = await open({ directory: true, title: "选择项目目录" });
    if (typeof dir !== "string") return;
    await addWorkspace(dir);
    await handleConnect(dir);
  };

  return (
    <main className="welcome-screen">
      <div className="welcome-content">
        <BrandMark className="brand-mark--hero" />
        <h1>OneMore</h1>
        <p>选择一个项目，让 agent 开始工作。</p>

        {lastError && (
          <div className="welcome-error">
            <span className="mono">{lastError.code}</span>
            <span>{lastError.message}</span>
          </div>
        )}

        <div className="welcome-projects">
          {workspaces.length === 0 ? (
            <button type="button" className="welcome-primary-action" onClick={() => void handleAddWorkspace()}>
              <FolderOpen size={16} />
              打开项目文件夹
            </button>
          ) : (
            workspaces.slice(0, 5).map((w) => (
              <button
                key={w.path}
                type="button"
                className="welcome-project-row"
                onClick={() => void handleConnect(w.path)}
              >
                <FolderOpen size={16} />
                <div>
                  <strong>{w.label}</strong>
                  <span>{w.path}</span>
                </div>
                <ChevronRight size={14} />
              </button>
            ))
          )}
          {workspaces.length > 0 && (
            <button type="button" className="welcome-secondary-action" onClick={() => void handleAddWorkspace()}>
              <FolderOpen size={14} /> 打开其他项目
            </button>
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
