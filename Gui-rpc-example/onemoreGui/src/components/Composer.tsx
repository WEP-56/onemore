// Composer 输入框:@文件引用、输入历史、排队指示、中断。
// 视觉参照 cc-gui ChatInputBox。

import { useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useStore } from "@/app/store";
import { File as FileIcon, Send, Square } from "lucide-react";
import type { FileTreeNode } from "@/app/types";
import { formatTokens } from "@/app/util";
import { cn } from "@/lib/utils";

const RUNNING_PHASES = ["running", "retrying", "compacting", "waiting_approval"];
const HISTORY_KEY = "onemore-gui:composer-history";
const MAX_HISTORY = 50;

function readHistory(): string[] {
  try {
    const raw = localStorage.getItem(HISTORY_KEY);
    const parsed = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((x): x is string => typeof x === "string") : [];
  } catch {
    return [];
  }
}

function pushHistory(text: string) {
  const history = readHistory().filter((h) => h !== text);
  history.unshift(text);
  localStorage.setItem(HISTORY_KEY, JSON.stringify(history.slice(0, MAX_HISTORY)));
}

interface MentionState {
  query: string;
  startIndex: number; // '@' 的位置
}

export default function Composer() {
  const conn = useStore((s) => s.conn);
  const phase = useStore((s) => s.snapshot?.phase ?? "idle");
  const draft = useStore((s) => s.draft);
  const setDraft = useStore((s) => s.setDraft);
  const busy = useStore((s) => s.busy);
  const sendPrompt = useStore((s) => s.sendPrompt);
  const sendSteer = useStore((s) => s.sendSteer);
  const sendAbort = useStore((s) => s.sendAbort);
  const queues = useStore((s) => s.snapshot?.queues);
  const usage = useStore((s) => s.snapshot?.usage);
  const activeWorkspace = useStore((s) => s.activeWorkspace);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [history] = useState<string[]>(readHistory);
  const [historyIndex, setHistoryIndex] = useState(-1);
  const [mention, setMention] = useState<MentionState | null>(null);
  const [mentionFiles, setMentionFiles] = useState<{ path: string; name: string; is_dir: boolean }[]>([]);
  const [mentionLoading, setMentionLoading] = useState(false);
  const [mentionCursor, setMentionCursor] = useState(0);

  const connected = conn === "connected";
  const running = RUNNING_PHASES.includes(phase);
  const queuedCount = (queues?.steering.length ?? 0) + (queues?.follow_up.length ?? 0);

  const submit = (text?: string) => {
    const value = (text ?? draft).trim();
    if (!value || busy || !connected) return;
    setDraft("");
    setHistoryIndex(-1);
    pushHistory(value);
    void (running ? sendSteer(value) : sendPrompt(value));
  };

  /* ── 输入历史 ↑/↓ ── */
  const handleHistoryKey = (e: React.KeyboardEvent<HTMLTextAreaElement>, current: string) => {
    if (history.length === 0) return;
    if (e.key === "ArrowUp" && (e.ctrlKey || current === "")) {
      e.preventDefault();
      const next = Math.min(historyIndex + 1, history.length - 1);
      setHistoryIndex(next);
      setDraft(history[next]);
      requestAnimationFrame(() => {
        const el = textareaRef.current;
        if (el) el.setSelectionRange(el.value.length, el.value.length);
      });
    } else if (e.key === "ArrowDown" && historyIndex >= 0) {
      e.preventDefault();
      const next = historyIndex - 1;
      setHistoryIndex(next);
      setDraft(next >= 0 ? history[next] : "");
    }
  };

  /* ── @ 引用 ── */
  const loadMentionFiles = async (workspace: string) => {
    setMentionLoading(true);
    try {
      const tree = await invoke<FileTreeNode[]>("get_file_tree", { workspace, maxDepth: 5 });
      const flat: { path: string; name: string; is_dir: boolean }[] = [];
      const walk = (nodes: FileTreeNode[], prefix = "") => {
        for (const n of nodes) {
          flat.push({ path: n.path, name: prefix + n.name, is_dir: n.is_dir });
          if (n.is_dir) walk(n.children ?? [], prefix + n.name + "/");
        }
      };
      walk(tree);
      setMentionFiles(flat);
    } catch {
      setMentionFiles([]);
    } finally {
      setMentionLoading(false);
    }
  };

  const filteredMentions = useMemo(() => {
    if (!mention) return [];
    const q = mention.query.toLowerCase();
    return mentionFiles
      .filter((f) => f.name.toLowerCase().includes(q) && !f.is_dir)
      .slice(0, 40);
  }, [mention, mentionFiles]);

  const applyMention = (path: string) => {
    if (!mention || !textareaRef.current) return;
    const el = textareaRef.current;
    const before = draft.slice(0, mention.startIndex);
    const after = draft.slice(el.selectionStart ?? mention.startIndex + mention.query.length + 1);
    const next = `${before}@${path} ${after}`;
    setDraft(next);
    setMention(null);
    requestAnimationFrame(() => {
      el.focus();
      const pos = before.length + path.length + 2;
      el.setSelectionRange(pos, pos);
    });
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (mention) {
        const pick = filteredMentions[mentionCursor];
        if (pick) applyMention(pick.path);
        return;
      }
      submit();
      return;
    }
    if (mention) {
      if (e.key === "Escape") {
        setMention(null);
        return;
      }
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setMentionCursor((c) => Math.min(c + 1, filteredMentions.length - 1));
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setMentionCursor((c) => Math.max(c - 1, 0));
        return;
      }
    }
    handleHistoryKey(e, draft);
  };

  const handleChange = (value: string) => {
    setDraft(value);
    const el = textareaRef.current;
    const caret = el?.selectionStart ?? value.length;
    // 检测 @:向前找最近的 @(前面是空白或行首)
    const at = value.lastIndexOf("@", caret - 1);
    if (at >= 0 && (at === 0 || /\s/.test(value[at - 1]))) {
      const query = value.slice(at + 1, caret);
      if (!query.includes("\n") && !query.includes("@") && query.length <= 64) {
        if (activeWorkspace) void loadMentionFiles(activeWorkspace);
        setMention({ query, startIndex: at });
        setMentionCursor(0);
        return;
      }
    }
    setMention(null);
  };

  /* ── 已引用文件 chips(解析 @path) ── */
  const refs = useMemo(() => {
    const out: string[] = [];
    for (const m of draft.matchAll(/@([^\s@]+)/g)) {
      const p = m[1];
      if (p.includes("/") || p.includes("\\") || p.includes(".")) out.push(p);
    }
    return out;
  }, [draft]);

  return (
    <div className="flex shrink-0 flex-col gap-1.5 px-4 pb-2 pt-2.5" style={{ background: "var(--surface-composer)", borderTop: "1px solid var(--border-subtle)" }}>
      {refs.length > 0 && (
        <div className="flex flex-wrap gap-1.5 px-1">
          {refs.map((r) => (
            <span
              key={r}
              className="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px]"
              style={{ border: "1px solid var(--border-strong)", background: "var(--surface-item)", color: "var(--text-muted)" }}
            >
              <FileIcon size={11} className="text-[var(--status-success)]" />
              {r.split(/[\\/]/).pop()}
            </span>
          ))}
        </div>
      )}
      <div
        className="relative flex items-end gap-2 rounded-lg px-3.5 py-2 transition-colors"
        style={{ background: "var(--surface-messages)", border: "1px solid var(--border-strong)" }}
        onFocus={(e) => (e.currentTarget.style.borderColor = "var(--status-success)")}
        onBlur={(e) => {
          e.currentTarget.style.borderColor = "var(--border-strong)";
          // 点击浮层内不关闭
        }}
      >
        <textarea
          ref={textareaRef}
          className="flex-1 resize-none border-none bg-transparent text-[14px] outline-none placeholder:text-[var(--text-faint)]"
          style={{ lineHeight: 1.45, minHeight: 22, maxHeight: 160 }}
          rows={1}
          value={draft}
          onChange={(e) => handleChange(e.target.value)}
          onKeyDown={handleKeyDown}
          onBlur={() => {
            // 延迟关闭,允许点击浮层
            window.setTimeout(() => setMention((m) => (m ? null : m)), 120);
          }}
          placeholder={!connected ? "请先连接工作区" : running ? "运行中：输入消息注入 steer…" : "输入消息开始对话…(@ 引用文件)"}
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
            onClick={() => submit()}
          >
            <Send size={15} />
          </button>
        </div>

        {/* @ 引用浮层 */}
        {mention && (
          <div
            className="absolute bottom-[calc(100%+6px)] left-2 z-20 w-80 overflow-hidden rounded-lg"
            style={{ background: "var(--surface-popover)", border: "1px solid var(--border-strong)", boxShadow: "0 12px 32px rgba(0,0,0,0.35)" }}
            onMouseDown={(e) => e.preventDefault()}
          >
            <div className="border-b px-3 py-1.5 text-[11px] text-[var(--text-faint)]" style={{ borderColor: "var(--border-subtle)" }}>
              {mentionLoading ? "加载文件…" : `引用文件 — ${filteredMentions.length} 个匹配`}
            </div>
            <div className="max-h-64 overflow-y-auto py-1">
              {filteredMentions.length === 0 && (
                <div className="px-3 py-2 text-[12px] text-[var(--text-faint)]">无匹配文件</div>
              )}
              {filteredMentions.map((f, i) => (
                <button
                  key={f.path}
                  type="button"
                  className={cn(
                    "flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12.5px]",
                    i === mentionCursor ? "bg-[var(--surface-hover)] text-[var(--text-strong)]" : "text-[var(--text-muted)]",
                  )}
                  onMouseEnter={() => setMentionCursor(i)}
                  onClick={() => applyMention(f.path)}
                >
                  <FileIcon size={12} className="shrink-0 text-[var(--text-faint)]" />
                  <span className="truncate">{f.name}</span>
                </button>
              ))}
            </div>
            <div className="flex items-center gap-2 border-t px-3 py-1 text-[10px] text-[var(--text-faint)]" style={{ borderColor: "var(--border-subtle)" }}>
              <span>↑↓ 选择</span>
              <span>Enter 引用</span>
              <span>Esc 关闭</span>
            </div>
          </div>
        )}
      </div>
      <div className="flex items-center justify-center gap-3 text-[11px] text-[var(--text-faint)]">
        {queuedCount > 0 && (
          <span style={{ color: "var(--status-warning)" }}>
            排队 {queuedCount} 条{running ? " · Enter 发送 steer" : ""}
          </span>
        )}
        {usage && (usage.input_tokens > 0 || usage.output_tokens > 0) && (
          <span className="mono">
            ↑ {formatTokens(usage.input_tokens)} ↓ {formatTokens(usage.output_tokens)}
          </span>
        )}
        <span>{running ? "运行中 — Enter 发送 steer" : "Enter 发送 · Shift+Enter 换行 · ↑ 历史 · @ 引用文件"}</span>
      </div>
    </div>
  );
}
