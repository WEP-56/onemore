// 协议视图类型：与 onemore RPC 协议保持一致。
// 这些是 GUI 展示用的封闭 view types，绝不包含 provider raw、thinking raw、
// 工具原始参数或任意 details。

// ---- ServerInfo ----
export interface Capabilities {
  compaction: boolean;
  session_management: boolean;
  interactive_approval: boolean;
  steering: boolean;
  follow_up: boolean;
}

export interface ModelMetadata {
  provider: string;
  model: string;
  label: string;
  supported_efforts: string[];
  default_effort: string;
}

export interface ServerInfo {
  server_id: string;
  protocol_version: number;
  capabilities: Capabilities;
  models: ModelMetadata[];
}

export interface SkillMetadataView {
  name: string;
  description: string;
  scope: "repo" | "user";
}

// ---- SessionSnapshot ----
export type SessionPhase =
  | "idle"
  | "running"
  | "retrying"
  | "compacting"
  | "waiting_approval"
  | "shutting_down";

export interface ModelSelectionView {
  provider: string;
  model: string;
  effort: string;
  label: string;
}

export interface UsageView {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number | null;
  cache_write_tokens: number | null;
}

export interface ToolMetadataView {
  command: string | null;
  cwd: string | null;
  elapsed_ms: number | null;
  exit_code: number | null;
}

export interface ToolOutputView {
  content: string;
  summary: string;
  metadata: ToolMetadataView;
}

export interface QueuedInputView {
  command_id: string;
  text: string;
}

export interface QueueView {
  steering: QueuedInputView[];
  follow_up: QueuedInputView[];
}

export type PlanStatus = "pending" | "in_progress" | "completed";

export interface PlanItemView {
  id: string;
  text: string;
  status: PlanStatus;
}

export interface PlanView {
  revision: number;
  items: PlanItemView[];
  explanation: string | null;
}

export type NoticeLevel = "info" | "warning" | "error";

export type AssistantBlock =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "tool_call"; id: string; name: string; summary: string };

export type TranscriptItem =
  | {
      type: "user_message";
      id: string;
      parent_id: string | null;
      created_at: number;
      text: string;
      command_id: string | null;
    }
  | {
      type: "assistant_message";
      id: string;
      parent_id: string | null;
      created_at: number;
      blocks: AssistantBlock[];
      status: "complete";
    }
  | {
      type: "tool";
      tool_call_id: string;
      name: string;
      summary: string;
      status: "succeeded" | "failed";
      output: string | null;
    }
  | { type: "notice"; id: string; created_at: number; level: NoticeLevel; text: string };

export type ApprovalScope = "once" | "session";

export interface ApprovalRequestView {
  request_id: string;
  tool: string;
  summary: string;
  reason: string;
  scopes: ApprovalScope[];
  command: string | null;
  cwd: string | null;
  targets: string[];
}

export type ApprovalDecision = "allow_once" | "allow_session" | "deny";

export interface SessionSnapshot {
  session_id: string;
  revision: number;
  workspace: string;
  phase: SessionPhase;
  model: ModelSelectionView;
  usage: UsageView;
  transcript: TranscriptItem[];
  plan: PlanView;
  queues: QueueView;
  pending_approval: ApprovalRequestView | null;
}

export interface SessionSummaryView {
  id: string;
  title: string;
  workspace: string;
  message_count: number;
  updated_at: number;
}

// ---- Events ----
export type CommandStatus = "succeeded" | "failed" | "cancelled";
export type CompactionTrigger = "automatic" | "manual";

export interface CommandErrorView {
  code: string;
  message: string;
}

export type SessionEvent =
  | { type: "session_snapshot"; snapshot: SessionSnapshot }
  | { type: "progress"; progress: ProgressEvent }
  | { type: "command_finished"; command_id: string; status: CommandStatus; error: CommandErrorView | null }
  | { type: "settled"; revision: number };

export type ProgressEvent =
  | { type: "user_message"; text: string }
  | { type: "run_started"; command_id: string }
  | { type: "retry_scheduled"; attempt: number; max_retries: number; delay_ms: number; error: string }
  | { type: "retry_started"; attempt: number; max_retries: number }
  | { type: "compaction_started"; compaction_id: string; trigger: CompactionTrigger; estimated_tokens: number; available_tokens: number | null }
  | { type: "compaction_finished"; compaction_id: string; trigger: CompactionTrigger; tokens_before: number; summary_chars: number; retained_messages: number }
  | { type: "compaction_failed"; compaction_id: string; trigger: CompactionTrigger; error: string; cancelled: boolean; history_changed: boolean }
  | { type: "assistant_delta"; message_id: string; content_index: number; kind: string; delta: string }
  | { type: "assistant_finished"; message_id: string; text: string }
  | { type: "tool_call_pending"; name: string }
  | { type: "tool_started"; tool_call_id: string; name: string; summary: string }
  | { type: "tool_updated"; tool_call_id: string; name: string; output: ToolOutputView }
  | { type: "tool_finished"; tool_call_id: string; name: string; output: ToolOutputView; error: CommandErrorView | null }
  | { type: "approval_requested"; request: ApprovalRequestView }
  | { type: "approval_resolved"; request_id: string; allowed: boolean }
  | { type: "notice"; level: NoticeLevel; text: string }
  | { type: "error"; error: CommandErrorView }
  | { type: "plan_updated"; plan: PlanView }
  | { type: "skills_discovered"; skills: SkillMetadataView[]; warnings: string[] }
  | { type: "usage"; usage: UsageView }
  | { type: "conversation_cleared" }
  | { type: "model_selection_changed"; selection: ModelSelectionView }
  | { type: "sessions_listed"; current_id: string; sessions: unknown[] };
