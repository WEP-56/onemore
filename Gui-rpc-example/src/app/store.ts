// Zustand 全局 store：连接生命周期、事件投影（reducer）、命令调用、指标与导出。

import { create } from "zustand";
import {
  rpcDiagnosticsTail,
  rpcRequest,
  rpcSnapshot,
  rpcStart,
  rpcStop,
  subscribeBackend,
  toErrorMessage,
} from "../rpc/client";
import {
  applyEvent,
  applyHello,
  applyProcessExit,
  applyRequestError,
  applySnapshot,
  applyStderr,
  applyTransportError,
  freshViewState,
} from "../rpc/reducer";
import type { ApprovalDecision, SessionSummaryView } from "../rpc/protocol";
import type { Mode, SessionViewState } from "./types";

export type SendKind = "prompt" | "steer" | "follow_up";

export interface ScheduledInput {
  atSec: number;
  kind: SendKind;
  text: string;
}

export interface ConnectOptions {
  executable: string;
  config: string;
  workspace: string;
}

interface AppStore extends SessionViewState {
  initialized: boolean;
  mode: Mode;
  connectOptions: ConnectOptions;
  draft: string;
  queueKind: "steer" | "follow_up";
  quickPrompt: string;
  longPrompt: string;
  maxObserveMinutes: number;
  autoSnapshotAfterSettled: boolean;
  scheduled: ScheduledInput[];
  longStartedAt: number | null;
  busy: boolean;

  init(): Promise<void>;
  setMode(mode: Mode): void;
  setConnectOptions(patch: Partial<ConnectOptions>): void;
  setDraft(v: string): void;
  setQueueKind(kind: "steer" | "follow_up"): void;
  setQuickPrompt(v: string): void;
  setLongPrompt(v: string): void;
  setMaxObserveMinutes(v: number): void;
  setAutoSnapshot(v: boolean): void;
  setScheduled(v: ScheduledInput[]): void;
  setLongStartedAt(v: number | null): void;

  connect(): Promise<void>;
  disconnect(): Promise<void>;
  sendPrompt(text: string): Promise<void>;
  sendSteer(text: string): Promise<void>;
  sendFollowUp(text: string): Promise<void>;
  sendAbort(): Promise<void>;
  setModel(provider: string, model: string, effort: string): Promise<void>;
  listSessions(): Promise<SessionSummaryView[] | null>;
  loadSession(id: string): Promise<void>;
  clearConversation(): Promise<void>;
  respondApproval(decision: ApprovalDecision): Promise<void>;
  snapshotNow(): Promise<void>;
  refreshDiagnostics(): Promise<void>;
  clearLogs(): void;
  exportReport(): void;
}

export const useStore = create<AppStore>((set, get) => {
  async function withBusy<T>(fn: () => Promise<T>): Promise<T> {
    set({ busy: true });
    try {
      return await fn();
    } finally {
      set({ busy: false });
    }
  }

  async function sendCommand(
    command: string,
    params: Record<string, unknown> | null,
  ): Promise<void> {
    try {
      const res = await withBusy(() => rpcRequest<{ command?: string; command_id?: string }>(command, params ?? undefined));
      const metrics = { ...get().metrics, acceptedCommands: get().metrics.acceptedCommands + 1 };
      set({ metrics, lastError: null });
      if (res?.command_id) {
        set({ run: { commandId: res.command_id, startedAt: Date.now() } });
      }
    } catch (e) {
      const err = toErrorMessage(e);
      set(applyRequestError(get(), err.code, err.message));
    }
  }

  return {
    ...freshViewState(),
    initialized: false,
    mode: "quick",
    connectOptions: { executable: "onemore", config: "", workspace: "" },
    draft: "",
    queueKind: "steer",
    quickPrompt:
      "请只读检查当前 workspace：概括项目用途，指出三个关键模块，并说明你会先运行哪项验证。不要修改文件。",
    longPrompt:
      "请调研当前项目并完成一个中等规模、可验证的改进。先建立计划，再实现、运行完整相关测试，最后总结改动、验证结果和剩余风险。遵守项目 AGENTS.md，不要跳过审批。",
    maxObserveMinutes: 30,
    autoSnapshotAfterSettled: true,
    scheduled: [],
    longStartedAt: null,
    busy: false,

    init: async () => {
      if (get().initialized) return;
      set({ initialized: true });
      await subscribeBackend((ev) => {
        const s = get();
        switch (ev.kind) {
          case "hello":
            set(applyHello(s, ev.server, ev.snapshot));
            break;
          case "event":
            set(applyEvent(s, ev.event));
            if (ev.event.type === "settled" && get().autoSnapshotAfterSettled) {
              void rpcRequest("get_snapshot").catch(() => {});
            }
            break;
          case "stderr":
            set(applyStderr(s, ev.line));
            break;
          case "transport_error":
            set(applyTransportError(s, ev.code, ev.message));
            break;
          case "process_exit":
            set(applyProcessExit(s, ev.code));
            break;
        }
      });
      // 热重载/刷新后恢复 backend 持有的最新 snapshot
      try {
        const snap = await rpcSnapshot();
        if (snap) set(applySnapshot(get(), snap));
      } catch {
        // 未连接时忽略
      }
    },

    setMode: (mode) => set({ mode }),
    setConnectOptions: (patch) => set({ connectOptions: { ...get().connectOptions, ...patch } }),
    setDraft: (draft) => set({ draft }),
    setQueueKind: (queueKind) => set({ queueKind }),
    setQuickPrompt: (quickPrompt) => set({ quickPrompt }),
    setLongPrompt: (longPrompt) => set({ longPrompt }),
    setMaxObserveMinutes: (maxObserveMinutes) => set({ maxObserveMinutes }),
    setAutoSnapshot: (autoSnapshotAfterSettled) => set({ autoSnapshotAfterSettled }),
    setScheduled: (scheduled) => set({ scheduled }),
    setLongStartedAt: (longStartedAt) => set({ longStartedAt }),

    connect: async () => {
      const { connectOptions } = get();
      const workspace = connectOptions.workspace.trim();
      if (!workspace) {
        set({ lastError: { code: "missing_workspace", message: "请先选择工作区目录" } });
        return;
      }
      set({ ...freshViewState(), conn: "spawning" });
      try {
        await rpcStart({
          executable: connectOptions.executable.trim() || "onemore",
          config: connectOptions.config.trim() || null,
          workspace,
        });
        set({ conn: "handshaking" });
        void get().refreshDiagnostics();
      } catch (e) {
        const err = toErrorMessage(e);
        set({
          conn: "disconnected",
          lastError: err,
          transportIssues: [{ code: err.code, message: err.message, at: Date.now() }],
        });
      }
    },

    disconnect: async () => {
      set({ conn: "shutting_down" });
      try {
        await rpcStop();
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
      // process_exit 事件会把 conn 置回 disconnected 并清空会话
    },

    sendPrompt: (text) => sendCommand("prompt", { text }),
    sendSteer: (text) => sendCommand("steer", { text }),
    sendFollowUp: (text) => sendCommand("follow_up", { text }),
    sendAbort: () => sendCommand("abort", null),
    setModel: (provider, model, effort) => sendCommand("set_model", { provider, model, effort }),

    listSessions: async () => {
      try {
        const res = await withBusy(() =>
          rpcRequest<{ command?: string; sessions?: SessionSummaryView[] }>("list_sessions"),
        );
        return res?.sessions ?? null;
      } catch (e) {
        const err = toErrorMessage(e);
        set(applyRequestError(get(), err.code, err.message));
        return null;
      }
    },
    loadSession: (id) => sendCommand("load_session", { session_id: id }),
    clearConversation: () => sendCommand("clear_conversation", null),

    respondApproval: async (decision) => {
      const req = get().liveApproval ?? get().snapshot?.pending_approval ?? null;
      if (!req) return;
      set({ liveApproval: null });
      await sendCommand("approval_response", { request_id: req.request_id, decision });
    },

    snapshotNow: async () => {
      try {
        await withBusy(() => rpcRequest("get_snapshot"));
      } catch (e) {
        const err = toErrorMessage(e);
        set(applyRequestError(get(), err.code, err.message));
      }
    },

    refreshDiagnostics: async () => {
      try {
        const lines = await rpcDiagnosticsTail(400);
        set({ stderrLines: lines });
      } catch (e) {
        const err = toErrorMessage(e);
        set(applyRequestError(get(), err.code, err.message));
      }
    },

    clearLogs: () => set({ stderrLines: [], transportIssues: [] }),

    exportReport: () => {
      const s = get();
      const report = {
        generated_at: new Date().toISOString(),
        mode: s.mode,
        server_id: s.server?.server_id ?? null,
        session_id: s.snapshot?.session_id ?? null,
        revision: s.snapshot?.revision ?? null,
        phase: s.snapshot?.phase ?? null,
        usage: s.snapshot?.usage ?? null,
        queues: s.snapshot?.queues ?? null,
        pending_approval: s.snapshot?.pending_approval ?? null,
        metrics: s.metrics,
        last_terminal: s.lastTerminal,
        last_error: s.lastError,
        transport_issues: s.transportIssues,
        stderr_tail: s.stderrLines.slice(-50),
      };
      const blob = new Blob([JSON.stringify(report, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `onemore-rpc-report-${new Date().toISOString().replace(/[:.]/g, "-")}.json`;
      a.click();
      URL.revokeObjectURL(url);
    },
  };
});
