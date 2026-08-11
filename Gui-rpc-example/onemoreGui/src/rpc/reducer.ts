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

let lastEventAt = 0;

// Date.now() can return the same value for several RPC frames in one render burst.
// Keep timestamps strictly increasing so live projections preserve arrival order.
const now = () => {
  lastEventAt = Math.max(Date.now(), lastEventAt + 1);
  return lastEventAt;
};

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
    liveActivity: null,
    rpcSessions: [],
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

export function applySnapshot(state: SessionViewState, snapshot: SessionSnapshot): SessionViewState {
  if (
    state.snapshot?.session_id === snapshot.session_id
    && snapshot.revision < state.snapshot.revision
  ) {
    return state;
  }
  let metrics = bumpPhase(state.metrics, snapshot.phase);
  metrics = { ...metrics, events: metrics.events + 1, snapshots: metrics.snapshots + 1 };
  const queueLen = snapshot.queues.steering.length + snapshot.queues.follow_up.length;
  metrics = { ...metrics, maxQueue: Math.max(metrics.maxQueue, queueLen) };

  // progress 中的 message_id 是传输期 ID，与持久化 transcript entry ID 不同。
  // idle 快照代表本轮已提交完成，此时必须整体丢弃实时投影，否则会重复并错序。
  const settled = snapshot.phase === "idle";
  const liveStreams = settled ? {} : state.liveStreams;
  const liveTools = settled ? {} : state.liveTools;

  let run = state.run;
  if (snapshot.phase === "idle" && run.commandId) {
    run = { commandId: null, startedAt: null };
  }

  return {
    ...state,
    snapshot,
    liveStreams,
    liveTools,
    liveUsers: settled ? {} : state.liveUsers,
    liveNotices: settled ? [] : state.liveNotices,
    liveApproval: snapshot.pending_approval,
    liveActivity: settled ? null : state.liveActivity,
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
      return { ...state, run: { commandId: p.command_id, startedAt: now() }, liveActivity: null };
    case "retry_scheduled": {
      return addNotice(
        {
          ...state,
          liveActivity: {
            kind: "retry",
            attempt: p.attempt,
            maxRetries: p.max_retries,
            scheduled: true,
          },
        },
        "info",
        `${p.error}，${(p.delay_ms / 1000).toFixed(1)}s 后重试(${p.attempt}/${p.max_retries})`,
        `retry:${p.attempt}`,
      );
    }
    case "retry_started":
      return {
        ...state,
        liveActivity: {
          kind: "retry",
          attempt: p.attempt,
          maxRetries: p.max_retries,
          scheduled: false,
        },
      };
    case "compaction_started":
      return {
        ...state,
        liveActivity: {
          kind: "compaction",
          compactionId: p.compaction_id,
          trigger: p.trigger,
        },
      };
    case "compaction_finished":
      return addNotice(
        clearCompactionActivity(state, p.compaction_id),
        "info",
        `上下文压缩完成：${p.tokens_before} tokens，保留 ${p.retained_messages} 条消息`,
        `compaction:${p.compaction_id}`,
      );
    case "compaction_failed":
      return addNotice(
        clearCompactionActivity(state, p.compaction_id),
        p.cancelled ? "warning" : "error",
        p.cancelled ? "上下文压缩已取消" : `上下文压缩失败：${p.error}`,
        `compaction:${p.compaction_id}`,
      );
    case "assistant_delta": {
      const key = `${p.message_id}:${p.content_index}:${p.kind}`;
      const prev = state.liveStreams[key];
      if (prev?.sealed) return state;
      const timestamp = now();
      const stream: LiveStream = prev
        ? { ...prev, text: prev.text + p.delta, updatedAt: timestamp }
        : { key, messageId: p.message_id, contentIndex: p.content_index, kind: p.kind, text: p.delta, sealed: false, createdAt: timestamp, updatedAt: timestamp };
      const metrics = { ...state.metrics, assistantDeltaChars: state.metrics.assistantDeltaChars + p.delta.length };
      return { ...state, metrics, liveStreams: { ...state.liveStreams, [key]: stream } };
    }
    case "assistant_finished": {
      const liveStreams = { ...state.liveStreams };
      const messageStreams = Object.entries(liveStreams)
        .filter(([, stream]) => stream.messageId === p.message_id)
        .sort(([, left], [, right]) => left.contentIndex - right.contentIndex);
      const textStreams = messageStreams.filter(([, stream]) => stream.kind !== "thinking");
      const timestamp = now();
      for (const [key, stream] of messageStreams) {
        liveStreams[key] = { ...stream, sealed: true, updatedAt: timestamp };
      }
      if (textStreams.length > 0) {
        for (const [index, [key, stream]] of textStreams.entries()) {
          liveStreams[key] = {
            ...stream,
            text: index === 0 ? p.text : "",
            sealed: true,
            updatedAt: timestamp,
          };
        }
      } else if (p.text) {
        const key = `${p.message_id}:0:text`;
        liveStreams[key] = {
          key,
          messageId: p.message_id,
          contentIndex: 0,
          kind: "text",
          text: p.text,
          sealed: true,
          createdAt: timestamp,
          updatedAt: timestamp,
        };
      }
      return { ...state, liveStreams };
    }
    case "tool_call_pending":
      return { ...state, liveActivity: { kind: "tool_call_pending", name: p.name } };
    case "tool_started": {
      if (state.liveTools[p.tool_call_id]) return state;
      const timestamp = now();
      const tool: LiveTool = {
        toolCallId: p.tool_call_id,
        name: p.name,
        summary: p.summary,
        output: "",
        outputSummary: "",
        metadata: { command: null, cwd: null, elapsed_ms: null, exit_code: null },
        status: "started",
        error: null,
        sealed: false,
        createdAt: timestamp,
        updatedAt: timestamp,
      };
      const metrics = { ...state.metrics, toolsStarted: state.metrics.toolsStarted + 1 };
      return {
        ...state,
        metrics,
        liveActivity: { kind: "tool", toolCallId: p.tool_call_id, name: p.name },
        liveTools: { ...state.liveTools, [p.tool_call_id]: tool },
      };
    }
    case "tool_updated": {
      const prev = state.liveTools[p.tool_call_id];
      if (prev?.sealed) return state;
      const timestamp = now();
      const tool: LiveTool = prev
        ? { ...prev, output: p.output.content, outputSummary: p.output.summary, metadata: p.output.metadata, status: "updated", updatedAt: timestamp }
        : { toolCallId: p.tool_call_id, name: p.name, summary: p.name, output: p.output.content, outputSummary: p.output.summary, metadata: p.output.metadata, status: "updated", error: null, sealed: false, createdAt: timestamp, updatedAt: timestamp };
      return { ...state, liveTools: { ...state.liveTools, [p.tool_call_id]: tool } };
    }
    case "tool_finished": {
      const prev = state.liveTools[p.tool_call_id];
      if (prev?.sealed) return state;
      const timestamp = now();
      const tool: LiveTool = prev
        ? { ...prev, output: p.output.content, outputSummary: p.output.summary, metadata: p.output.metadata, status: "finished", sealed: true, error: p.error?.message ?? null, updatedAt: timestamp }
        : { toolCallId: p.tool_call_id, name: p.name, summary: p.name, output: p.output.content, outputSummary: p.output.summary, metadata: p.output.metadata, status: "finished", error: p.error?.message ?? null, sealed: true, createdAt: timestamp, updatedAt: timestamp };
      const metrics = { ...state.metrics, toolsFinished: state.metrics.toolsFinished + 1 };
      const liveActivity =
        state.liveActivity?.kind === "tool" && state.liveActivity.toolCallId === p.tool_call_id
          ? null
          : state.liveActivity;
      return { ...state, metrics, liveActivity, liveTools: { ...state.liveTools, [p.tool_call_id]: tool } };
    }
    case "approval_requested": {
      const snapshot = state.snapshot
        ? { ...state.snapshot, phase: "waiting_approval" as const, pending_approval: p.request }
        : null;
      return { ...state, snapshot, liveApproval: p.request, liveActivity: null };
    }
    case "approval_resolved": {
      const snapshot =
        state.snapshot?.pending_approval?.request_id === p.request_id
          ? { ...state.snapshot, phase: "running" as const, pending_approval: null }
          : state.snapshot;
      return {
        ...state,
        snapshot,
        liveApproval: state.liveApproval?.request_id === p.request_id ? null : state.liveApproval,
      };
    }
    case "user_message": {
      const timestamp = now();
      const key = `user:${p.text}:${timestamp}`;
      return { ...state, liveUsers: { ...state.liveUsers, [key]: { key, text: p.text, at: timestamp } } };
    }
    case "notice": {
      return addNotice(state, p.level, p.text, "notice");
    }
    case "error":
      return {
        ...state,
        liveActivity: null,
        lastError: { code: p.error.code, message: p.error.message },
      };
    case "plan_updated":
      return state.snapshot
        ? { ...state, snapshot: { ...state.snapshot, plan: p.plan } }
        : state;
    case "skills_discovered":
      // Skill metadata belongs to the app store rather than the session snapshot.
      return state;
    case "usage":
      return state.snapshot
        ? { ...state, snapshot: { ...state.snapshot, usage: p.usage } }
        : state;
    case "conversation_cleared":
      return {
        ...state,
        snapshot: state.snapshot
          ? {
              ...state.snapshot,
              transcript: [],
              plan: { revision: 0, items: [], explanation: null },
              usage: {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: null,
                cache_write_tokens: null,
              },
              pending_approval: null,
            }
          : null,
        liveStreams: {},
        liveTools: {},
        liveUsers: {},
        liveNotices: [],
        liveApproval: null,
        liveActivity: null,
        lastError: null,
      };
    case "model_selection_changed":
      return state.snapshot
        ? { ...state, snapshot: { ...state.snapshot, model: p.selection } }
        : state;
    case "sessions_listed":
      return { ...state, rpcSessions: p.sessions };
  }

  const exhaustive: never = p;
  return exhaustive;
}

function addNotice(
  state: SessionViewState,
  level: LiveNotice["level"],
  text: string,
  keyPrefix: string,
): SessionViewState {
  const timestamp = now();
  const notice: LiveNotice = { key: `${keyPrefix}:${timestamp}`, level, text, at: timestamp };
  return { ...state, liveNotices: [...state.liveNotices, notice].slice(-50) };
}

function clearCompactionActivity(state: SessionViewState, compactionId: string): SessionViewState {
  if (
    state.liveActivity?.kind !== "compaction"
    || state.liveActivity.compactionId !== compactionId
  ) {
      return state;
  }
  return { ...state, liveActivity: null };
}

export function applyStderr(state: SessionViewState, line: string): SessionViewState {
  const stderrLines = [...state.stderrLines, line].slice(-400);
  return { ...state, stderrLines };
}

export function applyTransportError(state: SessionViewState, code: string, message: string): SessionViewState {
  const issue = { code, message, at: now() };
  return {
    ...state,
    conn: "disconnected",
    lastError: { code, message },
    transportIssues: [...state.transportIssues, issue].slice(-50),
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
    liveActivity: null,
    rpcSessions: [],
    run: { commandId: null, startedAt: null },
    metrics,
    lastError,
  };
}

export function applyRequestError(state: SessionViewState, code: string, message: string): SessionViewState {
  return { ...state, lastError: { code, message } };
}
