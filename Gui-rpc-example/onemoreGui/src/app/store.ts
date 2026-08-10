// Zustand 全局 store：RPC 连接、事件投影、workspace 管理、session 列表、config、git。

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { playErrorSound, playSuccessSound } from "@/lib/sounds";
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
import type { ApprovalDecision, NoticeLevel, SkillMetadataView } from "../rpc/protocol";
import type {
  ConfigDto,
  GitStatus,
  SessionEntry,
  SessionViewState,
  WorkspaceEntry,
  WorkspaceGroup,
  WorkspaceList,
} from "./types";
import { AUTO_CONTINUE_PROMPT, normalizeWorkspace, workspaceKey } from "./util";

interface AppStore extends SessionViewState {
  // workspace 管理
  workspaces: WorkspaceEntry[];
  workspaceGroups: WorkspaceGroup[];
  activeWorkspace: string | null;
  // session 列表（跨 workspace 扫描）
  sessions: SessionEntry[];
  skills: SkillMetadataView[];
  // session UI 状态
  pinnedSessions: Record<string, number>;
  // config
  configText: string;
  configDirty: boolean;
  configDto: ConfigDto | null;
  // git
  gitStatus: GitStatus | null;
  // UI
  initialized: boolean;
  settingsOpen: boolean;
  draft: string;
  searchQuery: string;
  busy: boolean;

  init(): Promise<void>;
  setDraft(v: string): void;
  setSearchQuery(v: string): void;
  setSettingsOpen(open: boolean): void;
  notify(level: NoticeLevel, text: string): void;

  // workspace
  loadWorkspaces(): Promise<void>;
  addWorkspace(path: string): Promise<void>;
  removeWorkspace(path: string): Promise<void>;
  selectWorkspace(path: string): Promise<void>;
  renameWorkspace(path: string, label: string): Promise<void>;
  createGroup(name: string): Promise<void>;
  renameGroup(id: string, name: string): Promise<void>;
  deleteGroup(id: string): Promise<void>;
  assignGroup(path: string, groupId: string): Promise<void>;

  // session
  loadSessions(): Promise<void>;
  loadSession(id: string): Promise<void>;
  newConversation(): Promise<void>;
  clearConversation(): Promise<void>;
  renameSession(id: string, title: string): Promise<void>;
  deleteSession(id: string): Promise<void>;
  togglePinSession(id: string): void;
  isSessionPinned(id: string): boolean;

  // config
  loadConfig(): Promise<void>;
  saveConfig(text: string): Promise<void>;
  loadConfigDto(): Promise<void>;
  saveConfigDto(dto: ConfigDto): Promise<void>;

  // git
  loadGitStatus(workspace: string): Promise<void>;

  // RPC
  connect(workspace: string): Promise<void>;
  disconnect(): Promise<void>;
  sendPrompt(text: string): Promise<void>;
  sendSteer(text: string): Promise<void>;
  sendAbort(): Promise<void>;
  sendCompact(): Promise<void>;
  sendSkill(name: string): Promise<void>;
  setModel(provider: string, model: string, effort: string): Promise<void>;
  respondApproval(decision: ApprovalDecision): Promise<void>;
  snapshotNow(): Promise<void>;
  refreshDiagnostics(): Promise<void>;
}

const PINNED_KEY = "onemore-gui:pinned-sessions";
const LAST_WORKSPACE_KEY = "onemore-gui:last-workspace";
const LAST_SESSION_KEY = "onemore-gui:last-session";
const MAX_AUTO_PLAN_CONTINUATIONS = 12;
const MAX_STAGNANT_PLAN_CONTINUATIONS = 2;

function readStoredValue(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStoredValue(key: string, value: string | null) {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch {
    // ignore
  }
}

function readPinnedSessions(): Record<string, number> {
  try {
    const raw = localStorage.getItem(PINNED_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw);
    return parsed && typeof parsed === "object" ? parsed : {};
  } catch {
    return {};
  }
}

function writePinnedSessions(pinned: Record<string, number>) {
  try {
    localStorage.setItem(PINNED_KEY, JSON.stringify(pinned));
  } catch {
    // ignore
  }
}

export const useStore = create<AppStore>((set, get) => {
  let restoreSessionId: string | null = null;
  let autoContinueTimer: ReturnType<typeof setTimeout> | null = null;
  let autoPlan = {
    enabled: false,
    total: 0,
    lastRevision: null as number | null,
    stagnant: 0,
  };

  function resetAutoPlan(enabled = false) {
    if (autoContinueTimer) clearTimeout(autoContinueTimer);
    autoContinueTimer = null;
    autoPlan = { enabled, total: 0, lastRevision: null, stagnant: 0 };
  }

  function pushNotice(level: NoticeLevel, text: string) {
    const at = Date.now();
    const notice = { key: `client:${at}:${Math.random()}`, level, text, at };
    set({ liveNotices: [...get().liveNotices, notice].slice(-50) });
  }

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
      const res = await withBusy(() =>
        rpcRequest<{ command?: string; command_id?: string }>(command, params ?? undefined),
      );
      const metrics = {
        ...get().metrics,
        acceptedCommands: get().metrics.acceptedCommands + 1,
      };
      set({ metrics, lastError: null });
      if (res?.command_id) {
        set({ run: { commandId: res.command_id, startedAt: Date.now() } });
      }
    } catch (e) {
      const err = toErrorMessage(e);
      set(applyRequestError(get(), err.code, err.message));
      if (command === "prompt" || command === "follow_up") resetAutoPlan();
    }
  }

  function continueActivePlan() {
    const state = get();
    if (!autoPlan.enabled) return;
    const activeItems = state.snapshot?.plan.items.filter((item) => item.status !== "completed") ?? [];
    if (activeItems.length === 0) {
      resetAutoPlan();
      return;
    }
    if (state.lastTerminal?.status !== "succeeded") {
      resetAutoPlan();
      return;
    }

    const revision = state.snapshot?.plan.revision ?? 0;
    autoPlan.stagnant = autoPlan.lastRevision === revision ? autoPlan.stagnant + 1 : 0;
    autoPlan.lastRevision = revision;
    if (
      autoPlan.total >= MAX_AUTO_PLAN_CONTINUATIONS
      || autoPlan.stagnant >= MAX_STAGNANT_PLAN_CONTINUATIONS
    ) {
      const reason = autoPlan.total >= MAX_AUTO_PLAN_CONTINUATIONS
        ? `已达到自动续跑上限（${MAX_AUTO_PLAN_CONTINUATIONS} 次）`
        : "计划连续两轮没有推进";
      resetAutoPlan();
      pushNotice("warning", `${reason}，已停止自动续跑；请检查当前结果后再决定是否继续。`);
      return;
    }

    autoPlan.total += 1;
    const attempt = autoPlan.total;
    pushNotice("info", `计划仍有 ${activeItems.length} 项未完成，正在自动续跑（${attempt}/${MAX_AUTO_PLAN_CONTINUATIONS}）`);
    autoContinueTimer = setTimeout(() => {
      autoContinueTimer = null;
      if (!autoPlan.enabled || get().conn !== "connected") return;
      void sendCommand("follow_up", { text: AUTO_CONTINUE_PROMPT });
    }, 250);
  }

  return {
    ...freshViewState(),
    workspaces: [],
    workspaceGroups: [],
    activeWorkspace: null,
    sessions: [],
    skills: [],
    pinnedSessions: readPinnedSessions(),
    configText: "",
    configDirty: false,
    configDto: null,
    gitStatus: null,
    initialized: false,
    settingsOpen: false,
    draft: "",
    searchQuery: "",
    busy: false,

    init: async () => {
      if (get().initialized) return;
      set({ initialized: true });
      await subscribeBackend((ev) => {
        const s = get();
        switch (ev.kind) {
          case "hello":
            set(applyHello(s, ev.server, ev.snapshot));
            writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(ev.snapshot.workspace));
            writeStoredValue(LAST_SESSION_KEY, ev.snapshot.session_id);
            void get().loadSessions();
            void get().loadGitStatus(ev.snapshot.workspace);
            if (restoreSessionId && restoreSessionId !== ev.snapshot.session_id) {
              const requested = restoreSessionId;
              restoreSessionId = null;
              void sendCommand("load_session", { session_id: requested });
            } else {
              restoreSessionId = null;
            }
            break;
          case "event":
            set(applyEvent(s, ev.event));
            if (ev.event.type === "progress" && ev.event.progress.type === "skills_discovered") {
              set({ skills: ev.event.progress.skills });
              for (const warning of ev.event.progress.warnings) pushNotice("warning", warning);
            }
            if (ev.event.type === "settled") {
              void get().loadSessions();
              const state = get();
              const hasActivePlan = state.snapshot?.plan.items.some((item) => item.status !== "completed") ?? false;
              if (!hasActivePlan && state.lastTerminal?.status === "succeeded") playSuccessSound();
              continueActivePlan();
            }
            if (ev.event.type === "session_snapshot") {
              writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(ev.event.snapshot.workspace));
              writeStoredValue(LAST_SESSION_KEY, ev.event.snapshot.session_id);
            }
            if (ev.event.type === "command_finished") {
              if (ev.event.status === "failed") playErrorSound();
            }
            break;
          case "stderr":
            set(applyStderr(s, ev.line));
            break;
          case "transport_error":
            set(applyTransportError(s, ev.code, ev.message));
            break;
          case "process_exit":
            resetAutoPlan();
            set(applyProcessExit(s, ev.code));
            break;
        }
      });
      try {
        const snap = await rpcSnapshot();
        if (snap) set(applySnapshot(get(), snap));
      } catch {
        // 未连接时忽略
      }
      await get().loadWorkspaces();
      await get().loadSessions();

      const lastSessionId = readStoredValue(LAST_SESSION_KEY);
      const lastWorkspace = readStoredValue(LAST_WORKSPACE_KEY);
      const session = lastSessionId ? get().sessions.find((item) => item.id === lastSessionId) : null;
      const targetKey = workspaceKey(session?.workspace ?? lastWorkspace ?? "");
      const workspace = get().workspaces.find((item) => workspaceKey(item.path) === targetKey);
      if (workspace) {
        restoreSessionId = session?.id ?? null;
        await get().connect(workspace.path);
      }
    },

    setDraft: (draft) => set({ draft }),
    setSearchQuery: (searchQuery) => set({ searchQuery }),
    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),
    notify: (level, text) => pushNotice(level, text),

    loadWorkspaces: async () => {
      try {
        const list = await invoke<WorkspaceList>("list_workspaces");
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch {
        // ignore
      }
    },

    addWorkspace: async (path) => {
      try {
        const list = await invoke<WorkspaceList>("add_workspace", { path });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    removeWorkspace: async (path) => {
      try {
        const list = await invoke<WorkspaceList>("remove_workspace", { path });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
        if (get().activeWorkspace === path) set({ activeWorkspace: null });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    renameWorkspace: async (path, label) => {
      try {
        const list = await invoke<WorkspaceList>("rename_workspace", { path, label });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    createGroup: async (name) => {
      try {
        const list = await invoke<WorkspaceList>("create_group", { name });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    renameGroup: async (id, name) => {
      try {
        const list = await invoke<WorkspaceList>("rename_group", { id, name });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    deleteGroup: async (id) => {
      try {
        const list = await invoke<WorkspaceList>("delete_group", { id });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    assignGroup: async (path, groupId) => {
      try {
        const list = await invoke<WorkspaceList>("assign_group", { path, groupId });
        set({ workspaces: list.workspaces, workspaceGroups: list.groups ?? [] });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    selectWorkspace: async (path) => {
      set({ activeWorkspace: path });
      writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(path));
      await get().loadGitStatus(path);
    },

    loadSessions: async () => {
      try {
        const sessions = await invoke<SessionEntry[]>("list_all_sessions");
        set({ sessions });
      } catch {
        // ignore
      }
    },

    loadSession: async (id) => {
      resetAutoPlan();
      const session = get().sessions.find((item) => item.id === id);
      if (!session) return;
      const targetKey = workspaceKey(session.workspace);
      const registered = get().workspaces.find((item) => workspaceKey(item.path) === targetKey);
      const target = registered?.path ?? normalizeWorkspace(session.workspace);
      writeStoredValue(LAST_WORKSPACE_KEY, target);
      writeStoredValue(LAST_SESSION_KEY, id);

      if (get().conn !== "connected" || workspaceKey(get().activeWorkspace ?? "") !== targetKey) {
        restoreSessionId = id;
        await get().connect(target);
        return;
      }
      await sendCommand("load_session", { session_id: id });
    },

    newConversation: async () => {
      resetAutoPlan();
      const active = get().activeWorkspace;
      if (!active) return;
      await get().loadSessions();
      writeStoredValue(LAST_SESSION_KEY, null);
      await get().connect(active);
    },

    clearConversation: () => sendCommand("clear_conversation", null),

    renameSession: async (id, title) => {
      try {
        await invoke("rename_session", { sessionId: id, title });
        await get().loadSessions();
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    deleteSession: async (id) => {
      try {
        await invoke("delete_session", { sessionId: id });
        await get().loadSessions();
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    togglePinSession: (id) => {
      const pinned = { ...get().pinnedSessions };
      if (pinned[id]) {
        delete pinned[id];
      } else {
        pinned[id] = Date.now();
      }
      writePinnedSessions(pinned);
      set({ pinnedSessions: pinned });
    },

    isSessionPinned: (id) => Boolean(get().pinnedSessions[id]),

    loadConfig: async () => {
      try {
        const text = await invoke<string>("read_config");
        set({ configText: text, configDirty: false });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    saveConfig: async (text) => {
      try {
        await invoke("write_config", { content: text });
        set({ configText: text, configDirty: false });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    loadConfigDto: async () => {
      try {
        const dto = await invoke<ConfigDto>("get_config_dto");
        set({ configDto: dto });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    saveConfigDto: async (dto) => {
      try {
        await invoke("update_config_dto", { dto });
        set({ configDto: dto });
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    loadGitStatus: async (workspace) => {
      try {
        const status = await invoke<GitStatus>("get_git_status", { workspace });
        set({ gitStatus: status });
      } catch {
        // ignore
      }
    },

    connect: async (workspace) => {
      resetAutoPlan();
      const normalized = normalizeWorkspace(workspace);
      writeStoredValue(LAST_WORKSPACE_KEY, normalized);
      set({ activeWorkspace: workspace, skills: [], ...freshViewState(), conn: "spawning" });
      try {
        await rpcStart({
          executable: "onemore",
          config: null,
          workspace: normalized,
        });
        set({ conn: "handshaking" });
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
      resetAutoPlan();
      set({ conn: "shutting_down" });
      try {
        await rpcStop();
      } catch (e) {
        set({ lastError: toErrorMessage(e) });
      }
    },

    sendPrompt: (text) => {
      resetAutoPlan(true);
      return sendCommand("prompt", { text });
    },
    sendSteer: (text) => sendCommand("steer", { text }),
    sendAbort: () => {
      resetAutoPlan();
      return sendCommand("abort", null);
    },
    sendCompact: () => {
      resetAutoPlan();
      return sendCommand("compact", null);
    },
    sendSkill: async (name) => {
      const skill = get().skills.find((item) => item.name === name);
      if (!skill) {
        pushNotice("error", `未发现技能 ${JSON.stringify(name)}，请检查技能目录后重启会话。`);
        return;
      }
      resetAutoPlan(true);
      const phase = get().snapshot?.phase ?? "idle";
      const command = ["running", "retrying", "compacting", "waiting_approval"].includes(phase)
        ? "steer"
        : "prompt";
      await sendCommand(command, {
        text: `请先加载并严格遵循技能 ${JSON.stringify(skill.name)}，然后继续处理当前请求。`,
      });
    },
    setModel: (provider, model, effort) =>
      sendCommand("set_model", { provider, model, effort }),

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
  };
});
