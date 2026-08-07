import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "../app/store";
import type { AssistantBlock, TranscriptItem } from "../rpc/protocol";
import type { LiveStream } from "../app/types";

/// 主视觉区域：权威 transcript + 尚未被 snapshot 提交的流式/工具增量。
export default function Transcript() {
  const snapshot = useStore((s) => s.snapshot);
  const liveStreams = useStore((s) => s.liveStreams);
  const liveTools = useStore((s) => s.liveTools);
  const liveUsers = useStore((s) => s.liveUsers);
  const liveNotices = useStore((s) => s.liveNotices);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [follow, setFollow] = useState(true);

  const nodes = useMemo(() => {
    const out: { key: string; node: React.ReactNode }[] = [];
    for (const item of snapshot?.transcript ?? []) {
      out.push({ key: `t:${transcriptKey(item)}`, node: <TranscriptItemView item={item} /> });
    }
    for (const u of Object.values(liveUsers)) {
      out.push({ key: `u:${u.key}`, node: <UserView text={u.text} /> });
    }
    for (const n of liveNotices) {
      out.push({ key: `n:${n.key}`, node: <NoticeView level={n.level} text={n.text} /> });
    }
    // 按 message_id 归并流式 delta，形成一条正在流式的 assistant 消息
    const byMessage = new Map<string, LiveStream[]>();
    for (const s of Object.values(liveStreams)) {
      const arr = byMessage.get(s.messageId) ?? [];
      arr.push(s);
      byMessage.set(s.messageId, arr);
    }
    for (const [messageId, arr] of byMessage) {
      arr.sort((a, b) => a.contentIndex - b.contentIndex);
      const blocks: AssistantBlock[] = arr.map((s) =>
        s.kind === "thinking" ? { type: "thinking", text: s.text } : { type: "text", text: s.text },
      );
      const streaming = !arr.every((s) => s.sealed);
      out.push({ key: `s:${messageId}`, node: <AssistantView blocks={blocks} streaming={streaming} /> });
    }
    for (const t of Object.values(liveTools)) {
      out.push({ key: `lt:${t.toolCallId}`, node: <ToolViewLive tool={t} /> });
    }
    return out;
  }, [snapshot, liveStreams, liveTools, liveUsers, liveNotices]);

  useEffect(() => {
    if (follow) scrollRef.current?.scrollIntoView({ behavior: "smooth", block: "end" });
  }, [nodes.length, follow]);

  return (
    <div
      className="transcript"
      onScroll={(e) => {
        const el = e.currentTarget;
        setFollow(el.scrollHeight - el.scrollTop - el.clientHeight < 160);
      }}
    >
      {nodes.length === 0 && (
        <div className="transcript-empty">
          尚未开始对话。在下方输入 prompt，或使用左侧「快速示范」的预置 prompt 发起只读检查。
        </div>
      )}
      {nodes.map((n) => (
        <div key={n.key} className="transcript-node">
          {n.node}
        </div>
      ))}
      <div ref={scrollRef} />
    </div>
  );
}

function transcriptKey(item: TranscriptItem): string {
  switch (item.type) {
    case "user_message":
      return `user:${item.id}`;
    case "assistant_message":
      return `assistant:${item.id}`;
    case "tool":
      return `tool:${item.tool_call_id}`;
    case "notice":
      return `notice:${item.id}`;
  }
}

function TranscriptItemView({ item }: { item: TranscriptItem }) {
  switch (item.type) {
    case "user_message":
      return <UserView text={item.text} commandId={item.command_id} />;
    case "assistant_message":
      return <AssistantView blocks={item.blocks} streaming={false} />;
    case "tool":
      return <ToolView item={item} />;
    case "notice":
      return <NoticeView level={item.level} text={item.text} />;
  }
}

function UserView({ text, commandId }: { text: string; commandId?: string | null }) {
  return (
    <div className="msg msg-user">
      <div className="msg-role">你</div>
      <div className="msg-body">
        <div className="msg-text">{text}</div>
        {commandId && <div className="msg-meta mono">cmd {commandId}</div>}
      </div>
    </div>
  );
}

function AssistantView({ blocks, streaming }: { blocks: AssistantBlock[]; streaming: boolean }) {
  return (
    <div className="msg msg-assistant">
      <div className="msg-role">assistant</div>
      <div className="msg-body">
        {blocks.length === 0 && streaming && <span className="caret" aria-hidden />}
        {blocks.map((b, i) =>
          b.type === "text" ? (
            <div key={i} className="msg-text">
              {b.text}
              {streaming && <span className="caret" aria-hidden />}
            </div>
          ) : b.type === "thinking" ? (
            <ThinkingBlock key={i} text={b.text} />
          ) : (
            <div key={i} className="tool-call-chip mono">
              <span className="tool-call-icon">⚙</span>
              <span>{b.name}</span>
              <span className="muted">{b.summary}</span>
            </div>
          ),
        )}
      </div>
    </div>
  );
}

function ThinkingBlock({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="thinking">
      <button type="button" className="thinking-toggle" onClick={() => setOpen(!open)}>
        {open ? "▾" : "▸"} 思考 {text.length} 字
      </button>
      {open && <div className="thinking-text">{text}</div>}
    </div>
  );
}

function ToolView({ item }: { item: Extract<TranscriptItem, { type: "tool" }> }) {
  return (
    <div className={`tool-row tool-${item.status}`}>
      <span className="tool-status" aria-hidden>
        {item.status === "succeeded" ? "✓" : "✕"}
      </span>
      <span className="tool-name mono">{item.name}</span>
      <span className="tool-summary">{item.summary}</span>
      {item.output && <pre className="tool-output">{item.output}</pre>}
    </div>
  );
}

function ToolViewLive({ tool }: { tool: { toolCallId: string; name: string; summary: string; output: string; status: string; error: string | null } }) {
  return (
    <div className={`tool-row tool-live tool-${tool.status}`}>
      <span className="tool-status" aria-hidden>
        {tool.status === "finished" ? (tool.error ? "✕" : "✓") : "…"}
      </span>
      <span className="tool-name mono">{tool.name}</span>
      <span className="tool-summary">{tool.summary}</span>
      {tool.output && <pre className="tool-output">{tool.output}</pre>}
      {tool.error && <div className="tool-error">{tool.error}</div>}
    </div>
  );
}

function NoticeView({ level, text }: { level: string; text: string }) {
  return <div className={`notice notice-${level}`}>{text}</div>;
}
