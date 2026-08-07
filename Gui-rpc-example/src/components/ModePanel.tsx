import { useEffect, useRef, useState } from "react";
import { useStore } from "../app/store";
import type { ScheduledInput } from "../app/store";
import type { Mode } from "../app/types";
import type { SessionSummaryView } from "../rpc/protocol";
import { formatDuration, formatTokens, phaseLabel } from "../app/util";
import CopyId from "./CopyId";

const MODES: { id: Mode; label: string; hint: string }[] = [
  { id: "quick", label: "快速示范", hint: "一次 prompt 走通 RPC" },
  { id: "long", label: "长任务", hint: "观察与运行中控制" },
  { id: "diagnostics", label: "诊断", hint: "stderr 与协议错误" },
];

export default function ModePanel() {
  const mode = useStore((s) => s.mode);
  const setMode = useStore((s) => s.setMode);
  return (
    <aside className="mode-panel">
      <nav className="mode-nav" aria-label="模式">
        {MODES.map((m) => (
          <button
            key={m.id}
            type="button"
            className={`mode-nav-item ${mode === m.id ? "active" : ""}`}
            onClick={() => setMode(m.id)}
          >
            <span className="mode-nav-label">{m.label}</span>
            <span className="mode-nav-hint">{m.hint}</span>
          </button>
        ))}
      </nav>
      <div className="mode-content">
        {mode === "quick" && <QuickPanel />}
        {mode === "long" && <LongPanel />}
        {mode === "diagnostics" && <DiagSummaryPanel />}
      </div>
    </aside>
  );
}

function QuickPanel() {
  const conn = useStore((s) => s.conn);
  const server = useStore((s) => s.server);
  const snapshot = useStore((s) => s.snapshot);
  const quickPrompt = useStore((s) => s.quickPrompt);
  const setDraft = useStore((s) => s.setDraft);
  const snapshotNow = useStore((s) => s.snapshotNow);
  const clearLogs = useStore((s) => s.clearLogs);
  const setModel = useStore((s) => s.setModel);
  const listSessions = useStore((s) => s.listSessions);
  const loadSession = useStore((s) => s.loadSession);
  const clearConversation = useStore((s) => s.clearConversation);

  const current = snapshot?.model;
  const currentValue = current ? `${current.provider}|${current.model}|${current.effort}` : "";
  const models = server?.models ?? [];
  const idle = snapshot?.phase === "idle";

  const [sessions, setSessions] = useState<SessionSummaryView[] | null>(null);
  useEffect(() => {
    if (conn === "connected") void listSessions().then(setSessions);
  }, [conn, listSessions]);

  return (
    <div className="mode-card">
      <h3>连接信息</h3>
      <dl className="kv">
        <dt>server</dt>
        <dd>{server ? <CopyId value={server.server_id} len={10} /> : "—"}</dd>
        <dt>workspace</dt>
        <dd className="mono" title={snapshot?.workspace}>{snapshot?.workspace ?? "—"}</dd>
        <dt>model</dt>
        <dd>{snapshot?.model.label ?? "—"}</dd>
        <dt>session</dt>
        <dd>{snapshot ? <CopyId value={snapshot.session_id} len={10} /> : "—"}</dd>
      </dl>

      {models.length > 0 && (
        <>
          <h3>模型</h3>
          <select
            className="input"
            value={currentValue}
            onChange={(e) => {
              const [provider, model, effort] = e.target.value.split("|");
              if (provider && model && effort) void setModel(provider, model, effort);
            }}
          >
            {models.map((m) => (
              <option key={`${m.provider}|${m.model}`} value={`${m.provider}|${m.model}|${m.default_effort}`}>
                {m.label}（effort {m.default_effort}）
              </option>
            ))}
          </select>
        </>
      )}

      <h3>会话</h3>
      <div className="session-list">
        {sessions === null ? (
          <p className="muted small">未加载</p>
        ) : sessions.length === 0 ? (
          <p className="muted small">暂无历史会话</p>
        ) : (
          sessions.map((s) => (
            <button
              key={s.id}
              type="button"
              className="session-row"
              disabled={!idle}
              title={`加载会话 ${s.id}`}
              onClick={() => void loadSession(s.id)}
            >
              <span className="session-title">{s.title || "（无标题）"}</span>
              <span className="muted mono small">
                {s.message_count} msg · {new Date(s.updated_at * 1000).toLocaleDateString()}
              </span>
            </button>
          ))
        )}
      </div>
      <div className="btn-grid">
        <button type="button" className="btn btn-quiet" onClick={() => void listSessions().then(setSessions)}>
          刷新
        </button>
        <button type="button" className="btn btn-quiet" disabled={!idle} onClick={() => void clearConversation()}>
          清空当前会话
        </button>
      </div>

      <h3>预置 prompt</h3>
      <p className="preset">{quickPrompt}</p>
      <button type="button" className="btn btn-block" onClick={() => setDraft(quickPrompt)}>
        填入预置 prompt
      </button>

      <h3>动作</h3>
      <button type="button" className="btn btn-block" onClick={() => void snapshotNow()}>
        获取快照
      </button>
      <button type="button" className="btn btn-quiet btn-block" onClick={clearLogs}>
        清空本地 UI 日志
      </button>
    </div>
  );
}

function LongPanel() {
  const snapshot = useStore((s) => s.snapshot);
  const longPrompt = useStore((s) => s.longPrompt);
  const setLongPrompt = useStore((s) => s.setLongPrompt);
  const maxObserveMinutes = useStore((s) => s.maxObserveMinutes);
  const setMaxObserveMinutes = useStore((s) => s.setMaxObserveMinutes);
  const autoSnapshot = useStore((s) => s.autoSnapshotAfterSettled);
  const setAutoSnapshot = useStore((s) => s.setAutoSnapshot);
  const scheduled = useStore((s) => s.scheduled);
  const setScheduled = useStore((s) => s.setScheduled);
  const longStartedAt = useStore((s) => s.longStartedAt);
  const setLongStartedAt = useStore((s) => s.setLongStartedAt);
  const sendPrompt = useStore((s) => s.sendPrompt);
  const sendSteer = useStore((s) => s.sendSteer);
  const sendFollowUp = useStore((s) => s.sendFollowUp);
  const sendAbort = useStore((s) => s.sendAbort);
  const snapshotNow = useStore((s) => s.snapshotNow);
  const exportReport = useStore((s) => s.exportReport);
  const metrics = useStore((s) => s.metrics);

  const [script, setScript] = useState(
    scheduled.map((s) => `${s.atSec}|${s.kind}|${s.text}`).join("\n"),
  );
  const phase = snapshot?.phase ?? "idle";
  const running = phase === "running" || phase === "retrying" || phase === "compacting" || phase === "waiting_approval";

  const parseScript = (text: string) => {
    const out: ScheduledInput[] = [];
    for (const line of text.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const [atSec, kind, ...rest] = trimmed.split("|");
      const at = Number(atSec);
      const k = kind === "follow_up" ? "follow_up" : "steer";
      if (Number.isFinite(at) && rest.length > 0) out.push({ atSec: at, kind: k, text: rest.join("|") });
    }
    setScheduled(out);
  };

  const start = async () => {
    const t = longPrompt.trim();
    if (!t) return;
    parseScript(script);
    setLongStartedAt(Date.now());
    await sendPrompt(t);
  };

  // 定时发送脚本（相对长任务开始时间）
  const firedRef = useRef<Set<string>>(new Set());
  useEffect(() => {
    firedRef.current = new Set();
  }, [longStartedAt]);

  useEffect(() => {
    if (!longStartedAt || scheduled.length === 0) return;
    const id = setInterval(() => {
      const elapsedSec = (Date.now() - longStartedAt) / 1000;
      for (const s of scheduled) {
        const key = `${s.atSec}|${s.kind}|${s.text}`;
        if (!firedRef.current.has(key) && elapsedSec >= s.atSec) {
          firedRef.current.add(key);
          if (s.kind === "follow_up") void sendFollowUp(s.text);
          else void sendSteer(s.text);
        }
      }
    }, 1000);
    return () => clearInterval(id);
  }, [longStartedAt, scheduled, sendSteer, sendFollowUp]);

  const unclosedTools = Math.max(0, metrics.toolsStarted - metrics.toolsFinished);
  const observeElapsed = longStartedAt ? Date.now() - longStartedAt : 0;
  const overObserve = maxObserveMinutes > 0 && observeElapsed > maxObserveMinutes * 60_000;

  return (
    <div className="mode-card">
      <h3>长任务 prompt</h3>
      <textarea
        className="input long-prompt"
        rows={4}
        value={longPrompt}
        onChange={(e) => setLongPrompt(e.target.value)}
        spellCheck={false}
      />
      <button type="button" className="btn btn-accent btn-block" disabled={running} onClick={() => void start()}>
        {longStartedAt ? "重新开始长任务" : "开始长任务"}
      </button>
      {longStartedAt && (
        <p className="mono small">
          已观察 {formatDuration(observeElapsed)}
          {overObserve && <span className="warn-text"> · 超过最大观察时长</span>}
        </p>
      )}

      <h3>观察设置</h3>
      <label className="kv-row">
        <span>最大观察时长（分钟）</span>
        <input
          className="input num"
          type="number"
          min={0}
          value={maxObserveMinutes}
          onChange={(e) => setMaxObserveMinutes(Number(e.target.value))}
        />
      </label>
      <label className="kv-row">
        <span>settled 后自动请求 snapshot</span>
        <input type="checkbox" checked={autoSnapshot} onChange={(e) => setAutoSnapshot(e.target.checked)} />
      </label>

      <h3>定时输入脚本（秒|kind|文本，默认关闭）</h3>
      <textarea
        className="input script"
        rows={3}
        value={script}
        onChange={(e) => setScript(e.target.value)}
        placeholder={'20|steer|先不要改代码，只输出风险\n90|follow_up|补充测试'}
        spellCheck={false}
      />

      <h3>运行中控制</h3>
      <div className="btn-grid">
        <button type="button" className="btn" disabled={!running} onClick={() => void snapshotNow()}>
          快照
        </button>
        <button type="button" className="btn btn-danger" disabled={!running} onClick={() => void sendAbort()}>
          中止
        </button>
      </div>

      <h3>观测指标</h3>
      <dl className="kv">
        <dt>accepted / terminal</dt>
        <dd className={metrics.acceptedCommands !== metrics.terminals ? "warn-text" : ""}>
          {metrics.acceptedCommands} / {metrics.terminals}
          {metrics.acceptedCommands !== metrics.terminals && " ⚠"}
        </dd>
        <dt>tool started / finished</dt>
        <dd className={unclosedTools > 0 ? "warn-text" : ""}>
          {metrics.toolsStarted} / {metrics.toolsFinished}
          {unclosedTools > 0 && `（未闭合 ${unclosedTools}）`}
        </dd>
        <dt>delta 字符</dt>
        <dd>{formatTokens(metrics.assistantDeltaChars)}</dd>
        <dt>max queue</dt>
        <dd>{metrics.maxQueue}</dd>
        <dt>events / snapshot</dt>
        <dd>{metrics.events} / {metrics.snapshots}</dd>
        <dt>settled / progress</dt>
        <dd>{metrics.settled} / {metrics.progresses}</dd>
        <dt>phase</dt>
        <dd className="mono">
          {phaseLabel(phase)}
          {metrics.lastPhase && metrics.phaseMs[metrics.lastPhase]
            ? `（${formatDuration(metrics.phaseMs[metrics.lastPhase] ?? 0)}）`
            : ""}
        </dd>
      </dl>

      <button type="button" className="btn btn-block" onClick={exportReport}>
        导出 JSON 测试报告
      </button>
    </div>
  );
}

function DiagSummaryPanel() {
  const metrics = useStore((s) => s.metrics);
  const stderrLines = useStore((s) => s.stderrLines);
  const transportIssues = useStore((s) => s.transportIssues);
  return (
    <div className="mode-card">
      <h3>诊断摘要</h3>
      <dl className="kv">
        <dt>事件总数</dt>
        <dd>{metrics.events}</dd>
        <dt>transport 错误</dt>
        <dd className={transportIssues.length ? "warn-text" : ""}>{transportIssues.length}</dd>
        <dt>stderr 行</dt>
        <dd>{stderrLines.length}</dd>
      </dl>
      <p className="muted small">
        stdout 只按 JSONL 协议解析；stderr 与 transport 错误使用 GUI 自己的 DTO，不伪装成协议事件。
      </p>
    </div>
  );
}
