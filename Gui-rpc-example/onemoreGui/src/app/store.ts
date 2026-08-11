// Zustand 全局 store：RPC 连接、事件投影、workspace 管理、session 列表、config、git。

import { create } from "zustand";
import { invoke } from "@tauri-apps/api/core";
import { playErrorSound, playSuccessSound } from "@/lib/sounds";
import {
  rpcDiagnosticsTail,
  rpcRequest,
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
  applyStderr,
  applyTransportError,
  freshViewState,
} from "../rpc/reducer";
import type {
  ApprovalDecision,
  NoticeLevel,
  SessionSummaryView,
  SkillMetadataView,
} from "../rpc/protocol";
import type {
  ConfigDto,
  GitStatus,
  ManagedRpcTask,
  SessionEntry,
  SessionViewState,
  WorkspaceEntry,
  WorkspaceGroup,
  WorkspaceList,
} from "./types";
import { AUTO_CONTINUE_PROMPT, normalizeWorkspace, workspaceKey } from "./util";

interface AppStore extends SessionViewState {
  activeConnectionId: string | null;
  rpcTasks: Record<string, ManagedRpcTask>;
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

function mergeRpcSessions(
  current: SessionEntry[],
  incoming: SessionSummaryView[],
): SessionEntry[] {
  const merged = new Map(current.map((session) => [session.id, session]));
  for (const session of incoming) {
    const previous = merged.get(session.id);
    merged.set(session.id, {
      id: session.id,
      workspace: session.workspace,
      title: session.title,
      created_at: previous?.created_at ?? session.updated_at,
      updated_at: session.updated_at,
      input_tokens: previous?.input_tokens ?? 0,
      output_tokens: previous?.output_tokens ?? 0,
      message_count: session.message_count,
    });
  }
  return [...merged.values()];
}

function mergeManagedSessions(
  current: SessionEntry[],
  tasks: ManagedRpcTask[],
): SessionEntry[] {
  const merged = new Map(current.map((session) => [session.id, session]));
  for (const task of tasks) {
    const snapshot = task.view.snapshot;
    if (!snapshot) continue;
    const previous = merged.get(snapshot.session_id);
    const rpcSummary = task.view.rpcSessions.find((session) => session.id === snapshot.session_id);
    const updatedAt = Math.floor(task.updatedAt / 1000);
    merged.set(snapshot.session_id, {
      id: snapshot.session_id,
      workspace: snapshot.workspace,
      title: previous?.title ?? rpcSummary?.title ?? "新会话",
      created_at: previous?.created_at ?? updatedAt,
      updated_at: Math.max(previous?.updated_at ?? 0, updatedAt),
      input_tokens: snapshot.usage.input_tokens,
      output_tokens: snapshot.usage.output_tokens,
      message_count: snapshot.transcript.length,
    });
  }
  return [...merged.values()];
}

export const useStore = create<AppStore>((set, get) => {
  const restoreSessions = new Map<string, string>();
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
    const connectionId = get().activeConnectionId;
    if (connectionId && get().rpcTasks[connectionId]) {
      updateTask(connectionId, (view) => ({
        ...view,
        liveNotices: [...view.liveNotices, notice].slice(-50),
      }));
    } else {
      set({ liveNotices: [...get().liveNotices, notice].slice(-50) });
    }
  }

  async function withBusy<T>(fn: () => Promise<T>): Promise<T> {
    set({ busy: true });
    try {
      return await fn();
    } finally {
      set({ busy: false });
    }
  }

  function updateTask(
    connectionId: string,
    update: (view: SessionViewState) => SessionViewState,
    fields: Partial<Omit<ManagedRpcTask, "connectionId" | "view">> = {},
  ): SessionViewState | null {
    const state = get();
    const current = state.rpcTasks[connectionId];
    if (!current) return null;
    const view = update(current.view);
    const task = { ...current, ...fields, view, updatedAt: Date.now() };
    const patch: Partial<AppStore> = {
      rpcTasks: { ...state.rpcTasks, [connectionId]: task },
    };
    if (state.activeConnectionId === connectionId) Object.assign(patch, view, { skills: task.skills });
    set(patch);
    return view;
  }

  function activateTask(task: ManagedRpcTask) {
    resetAutoPlan();
    writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(task.workspace));
    if (task.view.snapshot?.session_id) {
      writeStoredValue(LAST_SESSION_KEY, task.view.snapshot.session_id);
    }
    set({
      activeConnectionId: task.connectionId,
      activeWorkspace: task.workspace,
      skills: task.skills,
      ...task.view,
    });
    void get().loadGitStatus(task.workspace);
  }

  async function startTask(workspace: string, targetSessionId: string | null): Promise<void> {
    resetAutoPlan();
    const normalized = normalizeWorkspace(workspace);
    const connectionId = crypto.randomUUID();
    const view = { ...freshViewState(), conn: "spawning" as const };
    const task: ManagedRpcTask = {
      connectionId,
      workspace: normalized,
      targetSessionId,
      view,
      skills: [],
      updatedAt: Date.now(),
    };
    if (targetSessionId) restoreSessions.set(connectionId, targetSessionId);
    writeStoredValue(LAST_WORKSPACE_KEY, normalized);
    writeStoredValue(LAST_SESSION_KEY, targetSessionId);
    set({
      activeConnectionId: connectionId,
      activeWorkspace: normalized,
      rpcTasks: { ...get().rpcTasks, [connectionId]: task },
      skills: [],
      ...view,
    });
    try {
      await rpcStart(connectionId, {
        executable: "onemore",
        config: null,
        workspace: normalized,
      });
      updateTask(connectionId, (current) => ({ ...current, conn: "handshaking" }));
    } catch (e) {
      restoreSessions.delete(connectionId);
      const err = toErrorMessage(e);
      updateTask(connectionId, (current) => ({
        ...current,
        conn: "disconnected",
        lastError: err,
        transportIssues: [{ code: err.code, message: err.message, at: Date.now() }],
      }));
    }
  }

  async function sendCommand(
    command: string,
    params: Record<string, unknown> | null,
    connectionId = get().activeConnectionId,
  ): Promise<void> {
    if (!connectionId) return;
    try {
      const request = () => rpcRequest<{ command?: string; command_id?: string }>(
        connectionId,
        command,
        params ?? undefined,
      );
      const res = get().activeConnectionId === connectionId ? await withBusy(request) : await request();
      const task = get().rpcTasks[connectionId];
      if (!task) return;
      const metrics = {
        ...task.view.metrics,
        acceptedCommands: task.view.metrics.acceptedCommands + 1,
      };
      updateTask(connectionId, (current) => ({
        ...current,
        metrics,
        lastError: null,
        run: res?.command_id
          ? { commandId: res.command_id, startedAt: Date.now() }
          : current.run,
      }));
    } catch (e) {
      const err = toErrorMessage(e);
      updateTask(connectionId, (current) => applyRequestError(current, err.code, err.message));
      if (
        get().activeConnectionId === connectionId
        && (command === "prompt" || command === "follow_up")
      ) resetAutoPlan();
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
    activeConnectionId: null,
    rpcTasks: {},
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
        const isActive = get().activeConnectionId === ev.connection_id;
        switch (ev.kind) {
          case "hello": {
            updateTask(
              ev.connection_id,
              (view) => applyHello(view, ev.server, ev.snapshot),
              { workspace: normalizeWorkspace(ev.snapshot.workspace) },
            );
            if (isActive) {
              writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(ev.snapshot.workspace));
              writeStoredValue(LAST_SESSION_KEY, ev.snapshot.session_id);
              void get().loadGitStatus(ev.snapshot.workspace);
            }
            void get().loadSessions();
            const requested = restoreSessions.get(ev.connection_id);
            restoreSessions.delete(ev.connection_id);
            if (requested && requested !== ev.snapshot.session_id) {
              void sendCommand("load_session", { session_id: requested }, ev.connection_id);
            }
            break;
          }
          case "event": {
            const projected = updateTask(ev.connection_id, (view) => applyEvent(view, ev.event));
            if (ev.event.type === "progress" && ev.event.progress.type === "skills_discovered") {
              updateTask(ev.connection_id, (view) => view, { skills: ev.event.progress.skills });
              if (isActive) {
                for (const warning of ev.event.progress.warnings) pushNotice("warning", warning);
              }
            }
            if (ev.event.type === "progress" && ev.event.progress.type === "sessions_listed") {
              set({ sessions: mergeRpcSessions(get().sessions, ev.event.progress.sessions) });
              if (isActive) writeStoredValue(LAST_SESSION_KEY, ev.event.progress.current_id);
            }
            if (ev.event.type === "settled") {
              void get().loadSessions();
              const hasActivePlan = projected?.snapshot?.plan.items.some((item) => item.status !== "completed") ?? false;
              if (!hasActivePlan && projected?.lastTerminal?.status === "succeeded") playSuccessSound();
              if (isActive) continueActivePlan();
            }
            if (ev.event.type === "session_snapshot") {
              updateTask(ev.connection_id, (view) => view, {
                workspace: normalizeWorkspace(ev.event.snapshot.workspace),
                targetSessionId: ev.event.snapshot.session_id,
              });
              if (isActive) {
                writeStoredValue(LAST_WORKSPACE_KEY, normalizeWorkspace(ev.event.snapshot.workspace));
                writeStoredValue(LAST_SESSION_KEY, ev.event.snapshot.session_id);
              }
              void get().loadSessions();
            }
            if (ev.event.type === "command_finished") {
              if (ev.event.status === "failed") playErrorSound();
            }
            break;
          }
          case "stderr":
            updateTask(ev.connection_id, (view) => applyStderr(view, ev.line));
            break;
          case "transport_error":
            updateTask(
              ev.connection_id,
              (view) => applyTransportError(view, ev.code, ev.message),
            );
            break;
          case "process_exit":
            restoreSessions.delete(ev.connection_id);
            if (isActive) resetAutoPlan();
            updateTask(ev.connection_id, (view) => applyProcessExit(view, ev.code));
            break;
        }
      });
      await get().loadWorkspaces();
      await get().loadSessions();

      const lastSessionId = readStoredValue(LAST_SESSION_KEY);
      const lastWorkspace = readStoredValue(LAST_WORKSPACE_KEY);
      const session = lastSessionId ? get().sessions.find((item) => item.id === lastSessionId) : null;
      const targetKey = workspaceKey(session?.workspace ?? lastWorkspace ?? "");
      const workspace = get().workspaces.find((item) => workspaceKey(item.path) === targetKey);
      if (workspace) {
        if (session) await startTask(workspace.path, session.id);
        else await get().connect(workspace.path);
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
        set({ sessions: mergeManagedSessions(sessions, Object.values(get().rpcTasks)) });
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

      const existing = Object.values(get().rpcTasks)
        .filter((task) => task.view.conn !== "disconnected")
        .find((task) => task.view.snapshot?.session_id === id || task.targetSessionId === id);
      if (existing) {
        activateTask(existing);
        return;
      }
      await startTask(target, id);
    },

    newConversation: async () => {
      resetAutoPlan();
      const active = get().activeWorkspace;
      if (!active) return;
      await get().loadSessions();
      writeStoredValue(LAST_SESSION_KEY, null);
      await startTask(active, null);
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
      const normalized = normalizeWorkspace(workspace);
      const existing = Object.values(get().rpcTasks)
        .filter(
          (task) => task.view.conn !== "disconnected"
            && workspaceKey(task.workspace) === workspaceKey(normalized),
        )
        .sort((left, right) => right.updatedAt - left.updatedAt)[0];
      if (existing) {
        activateTask(existing);
        return;
      }
      await startTask(normalized, null);
    },

    disconnect: async () => {
      const connectionId = get().activeConnectionId;
      if (!connectionId) return;
      resetAutoPlan();
      updateTask(connectionId, (view) => ({ ...view, conn: "shutting_down" }));
      try {
        await rpcStop(connectionId);
      } catch (e) {
        const err = toErrorMessage(e);
        updateTask(connectionId, (view) => applyRequestError(view, err.code, err.message));
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
      const connectionId = get().activeConnectionId;
      if (connectionId) {
        updateTask(connectionId, (view) => ({ ...view, liveApproval: null }));
      }
      await sendCommand("approval_response", { request_id: req.request_id, decision });
    },

    snapshotNow: async () => {
      const connectionId = get().activeConnectionId;
      if (!connectionId) return;
      try {
        await withBusy(() => rpcRequest(connectionId, "get_snapshot"));
      } catch (e) {
        const err = toErrorMessage(e);
        updateTask(connectionId, (view) => applyRequestError(view, err.code, err.message));
      }
    },

    refreshDiagnostics: async () => {
      const connectionId = get().activeConnectionId;
      if (!connectionId) return;
      try {
        const lines = await rpcDiagnosticsTail(connectionId, 400);
        updateTask(connectionId, (view) => ({ ...view, stderrLines: lines }));
      } catch (e) {
        const err = toErrorMessage(e);
        updateTask(connectionId, (view) => applyRequestError(view, err.code, err.message));
      }
    },
  };
});
