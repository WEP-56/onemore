// reducer fixture 测试：response/event 交错、最终 snapshot 校正、重复 terminal 不破坏 UI。

import { describe, expect, it } from "vitest";
import type { ServerInfo, SessionSnapshot } from "./protocol";
import {
  applyEvent,
  applyHello,
  applyProcessExit,
  applySnapshot,
  freshViewState,
} from "./reducer";

function snapshot(overrides: Partial<SessionSnapshot> = {}): SessionSnapshot {
  return {
    session_id: "session-1",
    revision: 1,
    workspace: "E:\\work",
    phase: "idle",
    model: {
      provider: "deepseek",
      model: "deepseek-v4-flash",
      effort: "medium",
      label: "deepseek / deepseek-v4-flash",
    },
    usage: { input_tokens: 0, output_tokens: 0, cache_read_tokens: null, cache_write_tokens: null },
    transcript: [],
    plan: { revision: 0, items: [], explanation: null },
    queues: { steering: [], follow_up: [] },
    pending_approval: null,
    ...overrides,
  };
}

function serverInfo(): ServerInfo {
  return {
    server_id: "srv-1",
    protocol_version: 1,
    capabilities: {
      compaction: true,
      session_management: true,
      interactive_approval: true,
      steering: true,
      follow_up: true,
    },
    models: [],
  };
}

describe("reducer：流式增量组装", () => {
  it("assistant_delta 按序累积，字符数统计正确", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: { type: "run_started", command_id: "cmd-1" },
    });
    const s2 = applyEvent(s1, {
      type: "progress",
      progress: {
        type: "assistant_delta",
        message_id: "msg-1",
        content_index: 0,
        kind: "text",
        delta: "你好",
      },
    });
    const s3 = applyEvent(s2, {
      type: "progress",
      progress: {
        type: "assistant_delta",
        message_id: "msg-1",
        content_index: 0,
        kind: "text",
        delta: "世界",
      },
    });
    const streams = Object.values(s3.liveStreams);
    expect(streams).toHaveLength(1);
    expect(streams[0].text).toBe("你好世界");
    expect(s3.metrics.assistantDeltaChars).toBe(4);
    expect(s3.run.commandId).toBe("cmd-1");
  });

  it("assistant_finished 封口流式消息", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: {
        type: "assistant_delta",
        message_id: "msg-1",
        content_index: 0,
        kind: "text",
        delta: "hi",
      },
    });
    const s2 = applyEvent(s1, {
      type: "progress",
      progress: { type: "assistant_finished", message_id: "msg-1", text: "hi" },
    });
    expect(Object.values(s2.liveStreams)[0].sealed).toBe(true);
  });
});

describe("reducer：snapshot 权威校正", () => {
  it("已提交的流式消息被 snapshot 纠正并清除", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: {
        type: "assistant_delta",
        message_id: "msg-1",
        content_index: 0,
        kind: "text",
        delta: "本地拼装",
      },
    });
    expect(Object.keys(s1.liveStreams)).toHaveLength(1);

    const snap = snapshot({
      phase: "running",
      transcript: [
        {
          type: "assistant_message",
          id: "msg-1",
          parent_id: null,
          created_at: 1,
          blocks: [{ type: "text", text: "权威文本" }],
          status: "complete",
        },
      ],
    });
    const s2 = applySnapshot(s1, snap);
    expect(Object.keys(s2.liveStreams)).toHaveLength(0);
    expect(s2.snapshot?.transcript[0]).toEqual(expect.objectContaining({ id: "msg-1" }));
  });

  it("尚未提交的流式增量在 snapshot 后保留（交错输入）", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: {
        type: "assistant_delta",
        message_id: "msg-9",
        content_index: 0,
        kind: "text",
        delta: "还在流式",
      },
    });
    const snap = snapshot({ phase: "running", transcript: [] });
    const s2 = applySnapshot(s1, snap);
    expect(Object.keys(s2.liveStreams)).toHaveLength(1);
    expect(s2.liveStreams["msg-9:0:text"].text).toBe("还在流式");
  });

  it("工具增量在 tool_finished 后闭合，snapshot 提交后清理", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: { type: "tool_started", tool_call_id: "call-1", name: "read_file", summary: "read src" },
    });
    const s2 = applyEvent(s1, {
      type: "progress",
      progress: { type: "tool_finished", tool_call_id: "call-1", name: "read_file", output: "data", error: null },
    });
    expect(s2.metrics.toolsStarted).toBe(1);
    expect(s2.metrics.toolsFinished).toBe(1);
    expect(s2.liveTools["call-1"].sealed).toBe(true);

    const snap = snapshot({
      phase: "idle",
      transcript: [
        {
          type: "tool",
          tool_call_id: "call-1",
          name: "read_file",
          summary: "read src",
          status: "succeeded",
          output: "data",
        },
      ],
    });
    const s3 = applySnapshot(s2, snap);
    expect(Object.keys(s3.liveTools)).toHaveLength(0);
  });

  it("pending approval 由 snapshot 权威校正", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: {
        type: "approval_requested",
        request: {
          request_id: "approval-1",
          tool: "run_command",
          summary: "cargo test",
          reason: "requires approval",
          scopes: ["once", "session"],
        },
      },
    });
    expect(s1.liveApproval?.request_id).toBe("approval-1");

    const cleared = applySnapshot(s1, snapshot({ pending_approval: null }));
    expect(cleared.liveApproval).toBeNull();

    const pending = applySnapshot(s1, snapshot({ pending_approval: {
      request_id: "approval-1",
      tool: "run_command",
      summary: "cargo test",
      reason: "requires approval",
      scopes: ["once", "session"],
    } }));
    expect(pending.liveApproval?.request_id).toBe("approval-1");
  });
});

describe("reducer：重复/乱序输入不破坏 UI", () => {
  it("重复 command_finished 只累计计数，不抛错", () => {
    const s0 = freshViewState();
    const f1 = applyEvent(s0, {
      type: "command_finished",
      command_id: "cmd-1",
      status: "succeeded",
      error: null,
    });
    const f2 = applyEvent(f1, {
      type: "command_finished",
      command_id: "cmd-1",
      status: "succeeded",
      error: null,
    });
    expect(f2.metrics.terminals).toBe(2);
    expect(f2.lastTerminal?.commandId).toBe("cmd-1");
    expect(f2.lastTerminal?.status).toBe("succeeded");
  });

  it("settled 事件累计且不早于 terminal 出错", () => {
    const s0 = freshViewState();
    const st = applyEvent(s0, { type: "settled", revision: 5 });
    expect(st.metrics.settled).toBe(1);
    expect(st.snapshot).toBeNull();
  });

  it("run_started 重复出现时以最新 command 为准", () => {
    const s0 = freshViewState();
    const s1 = applyEvent(s0, {
      type: "progress",
      progress: { type: "run_started", command_id: "cmd-1" },
    });
    const s2 = applyEvent(s1, {
      type: "progress",
      progress: { type: "run_started", command_id: "cmd-2" },
    });
    expect(s2.run.commandId).toBe("cmd-2");
  });
});

describe("reducer：hello 与进程退出", () => {
  it("hello 建立连接并写入权威 snapshot", () => {
    const s = applyHello(freshViewState(), serverInfo(), snapshot());
    expect(s.conn).toBe("connected");
    expect(s.server?.server_id).toBe("srv-1");
    expect(s.snapshot?.session_id).toBe("session-1");
  });

  it("非零退出码记录为 process_exit 错误", () => {
    const s = applyProcessExit(freshViewState(), 1);
    expect(s.conn).toBe("disconnected");
    expect(s.lastError?.code).toBe("process_exit");
  });
});
