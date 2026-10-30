import { beforeEach, describe, expect, test, vi } from "vitest";

import { Store, isTransient, modelsForEvent } from "./store";
import { createFakeBridge } from "../test/fakeBridge";
import {
  CONVERSATION_ID,
  LEAD_ID,
  conversation,
  conversations,
  event,
  inboxItem,
  launcher,
  status,
} from "../test/fixtures";

function scripted() {
  const bridge = createFakeBridge();
  bridge.on("system.status", status({ eventHighWater: 40 }));
  bridge.on("project.list", []);
  bridge.on("workspace.conversations", conversations());
  bridge.on("workspace.launcher", launcher({ eventHighWater: 38 }));
  bridge.on("inbox.counts", {
    version: 1,
    counts: {
      unresolved: 1,
      approvals: 1,
      permissions: 0,
      incidents: 0,
      watching: 0,
    },
    eventHighWater: 39,
  });
  bridge.on("native.trust.list", []);
  bridge.on("workspace.conversation", conversation());
  bridge.on("inbox.list", {
    version: 1,
    items: [inboxItem()],
    counts: {
      unresolved: 1,
      approvals: 1,
      permissions: 0,
      incidents: 0,
      watching: 0,
    },
    eventHighWater: 41,
    truncated: false,
    nextBefore: null,
  });
  return bridge;
}

beforeEach(() => {
  localStorage.clear();
  vi.useRealTimers();
});

describe("modelsForEvent", () => {
  test("maps the contract index's families to the models they touch", () => {
    expect(modelsForEvent("approval.requested")).toEqual([
      "conversations",
      "conversation",
      "inbox",
      "inboxCounts",
    ]);
    expect(modelsForEvent("session.started")).toEqual([
      "launcher",
      "conversation",
    ]);
    expect(modelsForEvent("conversation.created")).toEqual([
      "conversations",
      "conversation",
      "launcher",
    ]);
    expect(modelsForEvent("native.trust.requested")).toEqual([
      "trust",
      "conversation",
    ]);
    expect(modelsForEvent("native-trust.requested")).toEqual([
      "trust",
      "conversation",
    ]);
    expect(modelsForEvent("memory.archived")).toEqual([]);
  });
});

describe("Store", () => {
  test("init reads every model and subscribes from the lowest high-water mark", async () => {
    const bridge = scripted();
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    const state = store.getSnapshot();
    expect(state.connection).toBe("connected");
    expect(state.status?.schemaVersion).toBe(19);
    expect(state.conversations?.conversations).toHaveLength(1);
    expect(state.launcher?.leads[0]?.name).toBe("mira");
    expect(state.inboxCounts?.unresolved).toBe(1);
    expect(bridge.subscribed).toEqual([38]);
    store.dispose();
  });

  test("an event re-reads its models once per 250 ms and advances the cursor", async () => {
    vi.useFakeTimers();
    const bridge = scripted();
    const store = new Store(bridge, { statusPollMs: 0, debounceMs: 250 });
    await store.init();
    const before = bridge.callsTo("workspace.conversations").length;
    bridge.emit(event(41, "turn.accepted"));
    bridge.emit(event(42, "turn.completed"));
    bridge.emit(event(43, "task.started"));
    expect(bridge.callsTo("workspace.conversations").length).toBe(before);
    await vi.advanceTimersByTimeAsync(260);
    expect(bridge.callsTo("workspace.conversations").length).toBe(before + 1);
    expect(bridge.callsTo("inbox.counts").length).toBe(2);
    store.dispose();
  });

  test("a closed subscription marks the connection uncertain and resubscribes after the last event", async () => {
    vi.useFakeTimers();
    const bridge = scripted();
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    bridge.emit(event(45, "turn.accepted"));
    bridge.closeEvents("pipe closed");
    expect(store.getSnapshot().connection).toBe("uncertain");
    await vi.advanceTimersByTimeAsync(600);
    expect(bridge.subscribed).toEqual([38, 45]);
    expect(store.getSnapshot().connection).toBe("connected");
    store.dispose();
  });

  test("act sends the daemon's params plus one idempotency key and re-reads afterwards", async () => {
    vi.useFakeTimers();
    const bridge = scripted();
    bridge.on("approval.decide", { ok: true });
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    const item = inboxItem();
    await store.act(item.actions[0]!);
    const sent = bridge.callsTo("approval.decide")[0] as Record<
      string,
      unknown
    >;
    expect(sent.id).toBe(item.id);
    expect(sent.expectedRevision).toBe(3);
    expect(sent.decision).toBe("approve");
    expect(typeof sent.idempotencyKey).toBe("string");
    expect(Object.keys(sent).sort()).toEqual([
      "decision",
      "expectedRevision",
      "id",
      "idempotencyKey",
    ]);
    await vi.advanceTimersByTimeAsync(300);
    expect(bridge.callsTo("inbox.counts").length).toBeGreaterThan(1);
    store.dispose();
  });

  test("a lost reply reuses the same key; a refusal does not", async () => {
    const bridge = scripted();
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    const item = inboxItem();
    bridge.fail("approval.decide", {
      code: "LZ-DESKTOP-DAEMON-OFFLINE",
      message: "offline",
    });
    await store.act(item.actions[0]!);
    bridge.on("approval.decide", { ok: true });
    await store.act(item.actions[0]!);
    const [first, second] = bridge.callsTo("approval.decide") as Record<
      string,
      unknown
    >[];
    expect(first!.idempotencyKey).toBe(second!.idempotencyKey);

    bridge.fail("approval.decide", {
      code: "LZ-APPROVAL-STALE",
      message: "stale",
    });
    await store.act(item.actions[1]!);
    bridge.on("approval.decide", { ok: true });
    await store.act(item.actions[1]!);
    const declines = bridge.callsTo("approval.decide").slice(2) as Record<
      string,
      unknown
    >[];
    expect(declines[0]!.idempotencyKey).not.toBe(declines[1]!.idempotencyKey);
    expect(
      store
        .getSnapshot()
        .toasts.some((toast) => toast.title === "LZ-APPROVAL-STALE"),
    ).toBe(true);
    store.dispose();
  });

  test("start creates the conversation, then submits the first turn with activateIfNeeded", async () => {
    const bridge = scripted();
    bridge.on("conversation.create", { conversation: { id: CONVERSATION_ID } });
    bridge.on("turn.submit", { turn: { id: "t" } });
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    await store.start({
      leadId: LEAD_ID,
      authority: "self-decision",
      text: "  Ship it  ",
    });
    const create = bridge.callsTo("conversation.create")[0] as Record<
      string,
      unknown
    >;
    const submit = bridge.callsTo("turn.submit")[0] as Record<string, unknown>;
    expect(create.leadEmployeeId).toBe(LEAD_ID);
    expect(create.projectId).toBeNull();
    expect(submit).toMatchObject({
      conversationId: CONVERSATION_ID,
      author: { kind: "user" },
      authorityMode: "self-decision",
      text: "Ship it",
      activateIfNeeded: true,
      maxAttempts: 3,
    });
    expect(create.idempotencyKey).not.toBe(submit.idempotencyKey);
    expect(store.getSnapshot().route).toEqual({
      kind: "conversation",
      id: CONVERSATION_ID,
      tab: "lead",
    });
    store.dispose();
  });

  test("a refused first turn keeps the conversation open and shows the daemon's code", async () => {
    const bridge = scripted();
    bridge.on("conversation.create", { conversation: { id: CONVERSATION_ID } });
    bridge.fail("turn.submit", {
      code: "LZ-TASK-AUTHORITY-UNSUPPORTED",
      message: "Claude Code serves full-autonomy only",
      data: { turnId: "t" },
    });
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    await store.start({ leadId: LEAD_ID, authority: "ask", text: "hello" });
    const state = store.getSnapshot();
    expect(state.route.kind).toBe("conversation");
    expect(state.composerError?.code).toBe("LZ-TASK-AUTHORITY-UNSUPPORTED");
    store.dispose();
  });

  test("opening a conversation marks it seen so the completed marker clears", async () => {
    const bridge = scripted();
    bridge.on(
      "workspace.conversations",
      conversations([
        {
          ...conversations().conversations[0]!,
          marker: "completed",
          lastResultAt: "2026-10-31T09:00:00Z",
        },
      ]),
    );
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    expect(store.getSnapshot().seen[CONVERSATION_ID]).toBeUndefined();
    store.openConversation(CONVERSATION_ID);
    expect(store.getSnapshot().seen[CONVERSATION_ID]).toBe(
      "2026-10-31T09:00:00Z",
    );
    store.dispose();
  });

  test("selecting a space resets the models and re-reads for that project", async () => {
    const bridge = scripted();
    const store = new Store(bridge, { statusPollMs: 0 });
    await store.init();
    store.selectSpace("project-1");
    await Promise.resolve();
    const params = bridge.callsTo("workspace.conversations").at(-1) as Record<
      string,
      unknown
    >;
    expect(params.projectId).toBe("project-1");
    expect(store.getSnapshot().route).toEqual({ kind: "launcher" });
    store.dispose();
  });

  test("transient failures are the only ones that keep a key", () => {
    expect(
      isTransient({ code: "LZ-DESKTOP-DAEMON-OFFLINE", message: "" }),
    ).toBe(true);
    expect(isTransient({ code: "LZ-TURN-STALE", message: "" })).toBe(false);
  });
});
