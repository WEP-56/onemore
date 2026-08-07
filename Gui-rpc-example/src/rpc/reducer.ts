// 状态投影 reducer：snapshot 是权威值，progress 是瞬时增量。
// 所有输入都是纯函数，重复/乱序事件不会破坏 UI；snapshot 到达时纠正本地组装状态。

import type {
  ProgressEvent,
  SessionEvent,
  SessionPhase,
  SessionSnapshot,
  ServerInfo,
} from "./protocol";
import type { LiveNotice, LiveStream, LiveTool, RunMetrics, SessionViewState } from "../app/types";

const MAX_STDERR = 400;
const MAX_ISSUES = 50;

const now = () => Date.now();

export function freshMetrics(): RunMetrics {
  return {
    startedAt: null,
    endedAt: null,
    events: 0,
    snapshots: 0,
    progresses: 0,
    commandFinished: 0,
    settled: 0,
    assistantDeltaChars: 0,
    toolsStarted: 0,
    toolsFinished: 0,
    acceptedCommands: 0,
    terminals: 0,
    maxQueue: 0,
    phaseMs: {},
    lastPhase: null,
    lastPhaseAt: null,
  };
}

export function freshViewState(): SessionViewState {
  return {
    conn: "disconnected",
    server: null,
    snapshot: null,
    liveStreams: {},
    liveTools: {},
    liveUsers: {},
    liveNotices: [],
    liveApproval: null,
    run: { commandId: null, startedAt: null },
    lastTerminal: null,
    lastError: null,
    stderrLines: [],
    transportIssues: [],
    metrics: freshMetrics(),
  };
}

function bumpPhase(metrics: RunMetrics, phase: SessionPhase): RunMetrics {
  const t = now();
  const next = { ...metrics };
  if (next.lastPhase && next.lastPhaseAt != null && phase !== next.lastPhase) {
    next.phaseMs = {
      ...next.phaseMs,
      [next.lastPhase]: (next.phaseMs[next.lastPhase] ?? 0) + (t - next.lastPhaseAt),
    };
  }
  next.lastPhase = phase;
  next.lastPhaseAt = t;
  return next;
}

/// 权威 snapshot 到达：整体替换 transcript/phase/queue/approval，
/// 只保留尚未被提交的流式与工具增量（纠正本地组装状态）。
export function applySnapshot(state: SessionViewState, snapshot: SessionSnapshot): SessionViewState {
  let metrics = bumpPhase(state.metrics, snapshot.phase);
  metrics = { ...metrics, events: metrics.events + 1, snapshots: metrics.snapshots + 1 };
  const queueLen = snapshot.queues.steering.length + snapshot.queues.follow_up.length;
  metrics = { ...metrics, maxQueue: Math.max(metrics.maxQueue, queueLen) };

  const committedStreamIds = new Set<string>();
  const committedToolIds = new Set<string>();
  for (const item of snapshot.transcript) {
    if (item.type === "assistant_message") committedStreamIds.add(item.id);
    if (item.type === "tool") committedToolIds.add(item.tool_call_id);
  }

  const liveStreams: Record<string, LiveStream> = {};
  for (const [key, s] of Object.entries(state.liveStreams)) {
    if (!committedStreamIds.has(s.messageId)) liveStreams[key] = s;
  }
  const liveTools: Record<string, LiveTool> = {};
  for (const [key, t] of Object.entries(state.liveTools)) {
    if (!committedToolIds.has(t.toolCallId)) liveTools[key] = t;
  }

  let run = state.run;
  if (snapshot.phase === "idle" && run.commandId) {
    run = { commandId: null, startedAt: null };
  }

  return {
    ...state,
    snapshot,
    liveStreams,
    liveTools,
    liveApproval: snapshot.pending_approval,
    run,
    metrics,
  };
}

export function applyHello(state: SessionViewState, server: ServerInfo, snapshot: SessionSnapshot): SessionViewState {
  return { ...applySnapshot(state, snapshot), conn: "connected", server, lastError: null };
}

export function applyEvent(state: SessionViewState, event: SessionEvent): SessionViewState {
  switch (event.type) {
    case "session_snapshot":
      return applySnapshot(state, event.snapshot);
    case "progress": {
      const metrics = { ...state.metrics, events: state.metrics.events + 1, progresses: state.metrics.progresses + 1 };
      return applyProgress({ ...state, metrics }, event.progress);
    }
    case "command_finished": {
      const metrics = {
        ...state.metrics,
        events: state.metrics.events + 1,
        commandFinished: state.metrics.commandFinished + 1,
        terminals: state.metrics.terminals + 1,
      };
      const terminal = { commandId: event.command_id, status: event.status, error: event.error, at: now() };
      const lastError =
        event.status === "failed" && event.error
          ? { code: event.error.code, message: event.error.message }
          : state.lastError;
      return { ...state, metrics, lastTerminal: terminal, lastError };
    }
    case "settled": {
      const metrics = { ...state.metrics, events: state.metrics.events + 1, settled: state.metrics.settled + 1 };
      return { ...state, metrics };
    }
  }
}

function applyProgress(state: SessionViewState, p: ProgressEvent): SessionViewState {
  switch (p.type) {
    case "run_started":
      return { ...state, run: { commandId: p.command_id, startedAt: now() } };
    case "assistant_delta": {
      const key = `${p.message_id}:${p.content_index}:${p.kind}`;
      const prev = state.liveStreams[key];
      const stream: LiveStream = prev
        ? { ...prev, text: prev.text + p.delta, updatedAt: now() }
        : { key, messageId: p.message_id, contentIndex: p.content_index, kind: p.kind, text: p.delta, sealed: false, updatedAt: now() };
      const metrics = { ...state.metrics, assistantDeltaChars: state.metrics.assistantDeltaChars + p.delta.length };
      return { ...state, metrics, liveStreams: { ...state.liveStreams, [key]: stream } };
    }
    case "assistant_finished": {
      const liveStreams = { ...state.liveStreams };
      for (const [key, s] of Object.entries(liveStreams)) {
        if (s.messageId === p.message_id) {
          liveStreams[key] = { ...s, sealed: true, text: p.text || s.text, updatedAt: now() };
        }
      }
      return { ...state, liveStreams };
    }
    case "tool_started": {
      const tool: LiveTool = {
        toolCallId: p.tool_call_id,
        name: p.name,
        summary: p.summary,
        output: "",
        status: "started",
        error: null,
        sealed: false,
        updatedAt: now(),
      };
      const metrics = { ...state.metrics, toolsStarted: state.metrics.toolsStarted + 1 };
      return { ...state, metrics, liveTools: { ...state.liveTools, [p.tool_call_id]: tool } };
    }
    case "tool_updated": {
      const prev = state.liveTools[p.tool_call_id];
      const tool: LiveTool = prev
        ? { ...prev, output: p.output, status: "updated", updatedAt: now() }
        : { toolCallId: p.tool_call_id, name: p.name, summary: p.name, output: p.output, status: "updated", error: null, sealed: false, updatedAt: now() };
      return { ...state, liveTools: { ...state.liveTools, [p.tool_call_id]: tool } };
    }
    case "tool_finished": {
      const prev = state.liveTools[p.tool_call_id];
      const tool: LiveTool = prev
        ? { ...prev, output: p.output, status: "finished", sealed: true, error: p.error?.message ?? null, updatedAt: now() }
        : { toolCallId: p.tool_call_id, name: p.name, summary: p.name, output: p.output, status: "finished", error: p.error?.message ?? null, sealed: true, updatedAt: now() };
      const metrics = { ...state.metrics, toolsFinished: state.metrics.toolsFinished + 1 };
      return { ...state, metrics, liveTools: { ...state.liveTools, [p.tool_call_id]: tool } };
    }
    case "approval_requested":
      return { ...state, liveApproval: p.request };
    case "approval_resolved":
      return {
        ...state,
        liveApproval: state.liveApproval?.request_id === p.request_id ? null : state.liveApproval,
      };
    case "user_message": {
      const key = `user:${p.text}:${now()}`;
      return { ...state, liveUsers: { ...state.liveUsers, [key]: { key, text: p.text, at: now() } } };
    }
    case "notice": {
      const n: LiveNotice = { key: `notice:${now()}`, level: p.level, text: p.text, at: now() };
      return { ...state, liveNotices: [...state.liveNotices, n].slice(-50) };
    }
    case "error":
      return { ...state, lastError: { code: p.error.code, message: p.error.message } };
    default:
      // usage / plan_updated / skills_discovered / 其它增量最终由 snapshot 权威校正
      return state;
  }
}

export function applyStderr(state: SessionViewState, line: string): SessionViewState {
  const stderrLines = [...state.stderrLines, line].slice(-MAX_STDERR);
  return { ...state, stderrLines };
}

export function applyTransportError(state: SessionViewState, code: string, message: string): SessionViewState {
  const issue = { code, message, at: now() };
  return {
    ...state,
    conn: "disconnected",
    lastError: { code, message },
    transportIssues: [...state.transportIssues, issue].slice(-MAX_ISSUES),
  };
}

export function applyProcessExit(state: SessionViewState, code: number | null): SessionViewState {
  const metrics = { ...state.metrics, endedAt: now() };
  const lastError =
    code != null && code !== 0
      ? { code: "process_exit", message: `子进程退出码 ${code}` }
      : state.lastError;
  return {
    ...state,
    conn: "disconnected",
    server: null,
    snapshot: null,
    liveStreams: {},
    liveTools: {},
    liveUsers: {},
    liveNotices: [],
    liveApproval: null,
    run: { commandId: null, startedAt: null },
    metrics,
    lastError,
  };
}

export function applyRequestError(state: SessionViewState, code: string, message: string): SessionViewState {
  return { ...state, lastError: { code, message } };
}
