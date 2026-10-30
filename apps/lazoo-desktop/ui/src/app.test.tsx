import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { App } from "./App";
import { Store } from "./lib/store";
import { createFakeBridge, type FakeBridge } from "./test/fakeBridge";
import {
  CONVERSATION_ID,
  TASK_ID,
  conversation,
  conversations,
  event,
  inboxItem,
  launcher,
  project,
  status,
  terminalStatus,
  turnRecord,
  workerCard,
} from "./test/fixtures";

vi.mock("@xterm/xterm", () => {
  class Terminal {
    element: HTMLElement | null = null;
    unicode = { activeVersion: "6" };
    options = {};
    loadAddon() {}
    onData() {
      return { dispose() {} };
    }
    onBinary() {
      return { dispose() {} };
    }
    onResize() {
      return { dispose() {} };
    }
    open(container: HTMLElement) {
      this.element = document.createElement("div");
      this.element.className = "xterm";
      container.appendChild(this.element);
    }
    write() {}
    reset() {}
    dispose() {}
  }
  return { Terminal };
});
vi.mock("@xterm/addon-fit", () => ({
  FitAddon: class {
    activate() {}
    dispose() {}
    fit() {}
    proposeDimensions() {
      return { cols: 100, rows: 30 };
    }
  },
}));
vi.mock("@xterm/addon-webgl", () => ({
  WebglAddon: class {
    activate() {}
    dispose() {}
    onContextLoss() {}
  },
}));
vi.mock("@xterm/addon-unicode11", () => ({
  Unicode11Addon: class {
    activate() {}
    dispose() {}
  },
}));

function scripted(): FakeBridge {
  const bridge = createFakeBridge();
  bridge.on("system.status", status());
  bridge.on("project.list", [project()]);
  bridge.on("workspace.conversations", conversations());
  bridge.on("workspace.launcher", launcher());
  bridge.on("inbox.counts", {
    version: 1,
    counts: {
      unresolved: 0,
      approvals: 0,
      permissions: 0,
      incidents: 0,
      watching: 0,
    },
    eventHighWater: 40,
  });
  bridge.on("native.trust.list", []);
  bridge.on("workspace.conversation", conversation());
  bridge.on("turn.show", turnRecord());
  bridge.on(
    "session.terminal.status",
    terminalStatus({
      viewable: false,
      viewBlockingReason:
        "the provider does not support a native read-only view",
    }),
  );
  bridge.on("inbox.list", {
    version: 1,
    items: [],
    counts: {
      unresolved: 0,
      approvals: 0,
      permissions: 0,
      incidents: 0,
      watching: 0,
    },
    eventHighWater: 40,
    truncated: false,
    nextBefore: null,
  });
  bridge.on("session.list", []);
  bridge.on("session.resources", () => {
    throw { code: "LZ-SESSION-NOT-LIVE", message: "not live" };
  });
  return bridge;
}

async function expectNoSeriousViolations(container: HTMLElement) {
  const results = await axe.run(container, {
    runOnly: { type: "tag", values: ["wcag2a", "wcag2aa"] },
  });
  const serious = results.violations.filter(
    (violation) =>
      violation.impact === "critical" || violation.impact === "serious",
  );
  expect(serious).toEqual([]);
}

function mount(bridge: FakeBridge) {
  const store = new Store(bridge, { statusPollMs: 0, debounceMs: 10 });
  const view = render(<App store={store} bridge={bridge} />);
  return { store, ...view };
}

beforeEach(() => {
  localStorage.clear();
});

describe("shell", () => {
  test("renders the titlebar controls, the rail, and the launcher with the remembered lead", async () => {
    const bridge = scripted();
    const { container } = mount(bridge);
    expect(
      await screen.findByLabelText("Open command palette"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Minimize window")).toBeInTheDocument();
    expect(
      screen.getByLabelText("Maximize or restore window"),
    ).toBeInTheDocument();
    expect(screen.getByLabelText("Close window")).toBeInTheDocument();
    expect(screen.getByText("New work")).toBeInTheDocument();
    expect(
      await screen.findByText("fix flaky daemon test"),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "What should the team work on?" }),
    ).toBeInTheDocument();
    expect(await screen.findByRole("radio", { name: /mira/ })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(
      screen.getByText(/led your last conversation here/),
    ).toBeInTheDocument();
    await expectNoSeriousViolations(container);
  });

  test("a blocked lead cannot start and the first blocking reason is shown", async () => {
    const bridge = scripted();
    bridge.on(
      "workspace.launcher",
      launcher({
        leads: [
          launcher().leads[0]!,
          {
            ...launcher().leads[0]!,
            employeeId: "other",
            name: "ash",
            startable: false,
            blockingReason: "OpenCode has no verified model",
            provider: "opencode",
          },
        ],
      }),
    );
    const user = userEvent.setup();
    mount(bridge);
    await screen.findByRole("radio", { name: /mira/ });
    await user.click(screen.getByRole("radio", { name: /ash/ }));
    await user.type(
      screen.getByLabelText("What should the team work on?"),
      "do a thing",
    );
    await user.click(screen.getByRole("button", { name: "Start" }));
    expect(bridge.callsTo("conversation.create")).toHaveLength(0);
    expect(
      screen.getAllByText("OpenCode has no verified model").length,
    ).toBeGreaterThan(0);
  });

  test("the command palette opens with Ctrl+K, filters, and navigates", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await screen.findByLabelText("Open command palette");
    await user.keyboard("{Control>}k{/Control}");
    const search = await screen.findByLabelText("Search commands");
    await user.type(search, "inbox");
    await user.keyboard("{Enter}");
    expect(
      await screen.findByRole("heading", { name: "Inbox" }),
    ).toBeInTheDocument();
    await waitFor(() =>
      expect(
        screen.queryByLabelText("Search commands"),
      ).not.toBeInTheDocument(),
    );
  });
});

describe("conversation", () => {
  test("opens with the lead as the main tab, shows the transcript, and adds a worker tab when one starts", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { store, container } = mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    const tabs = await screen.findByRole("tablist", { name: "Agents" });
    expect(
      within(tabs).getByRole("tab", { name: /mira, lead, busy/ }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Transcript" }));
    expect(
      await screen.findByText(
        "Fix the flaky daemon test without widening timeouts.",
      ),
    ).toBeInTheDocument();
    expect(
      await screen.findByText("Reproduced locally in 4 of 20 runs."),
    ).toBeInTheDocument();

    bridge.on(
      "workspace.conversation",
      conversation({ workers: [workerCard()], eventHighWater: 41 }),
    );
    act(() => bridge.emit(event(41, "task.started")));
    expect(
      await within(tabs).findByRole("tab", {
        name: /rune, Reproduce across 20 runs, running/,
      }),
    ).toBeInTheDocument();
    expect(store.getSnapshot().conversation?.workers).toHaveLength(1);
    await expectNoSeriousViolations(container);
  });

  test("the native pane states the daemon's reason when no view is possible", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    expect(
      await screen.findByText("Native view unavailable"),
    ).toBeInTheDocument();
    expect(
      screen.getByText("the provider does not support a native read-only view"),
    ).toBeInTheDocument();
    expect(bridge.callsTo("session.terminal.view")).toHaveLength(0);
  });

  test("a viewable session opens the native view once with measured dimensions", async () => {
    const bridge = scripted();
    bridge.on("session.terminal.status", terminalStatus());
    bridge.on("session.terminal.view", () => ({
      sessionId: terminalStatus().sessionId,
      terminal: {
        id: "term-1",
        bootId: terminalStatus().bootId,
        pid: 1,
        executable: "codex",
        args: [],
        cwd: "",
        columns: 100,
        rows: 30,
        revision: 1,
        live: true,
        exitCode: null,
      },
    }));
    const user = userEvent.setup();
    mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    await waitFor(() =>
      expect(bridge.callsTo("session.terminal.view")).toHaveLength(1),
    );
    const params = bridge.callsTo("session.terminal.view")[0] as Record<
      string,
      unknown
    >;
    expect(params).toMatchObject({
      id: terminalStatus().sessionId,
      expectedRevision: 3,
      columns: 100,
      rows: 30,
    });
    expect(typeof params.idempotencyKey).toBe("string");
    expect(
      await screen.findByText("Native view · read-only"),
    ).toBeInTheDocument();
    await waitFor(() => expect(bridge.terminalFrames).toHaveLength(1));
  });

  test("the attention strip offers the item's actions and sends the first one as data", async () => {
    const bridge = scripted();
    bridge.on(
      "workspace.conversation",
      conversation({ attention: [inboxItem()], workers: [workerCard()] }),
    );
    bridge.on("approval.decide", { ok: true });
    const user = userEvent.setup();
    mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    const strip = await screen.findByLabelText("Needs you");
    await user.click(
      within(strip).getByRole("button", { name: "Approve once" }),
    );
    const sent = bridge.callsTo("approval.decide")[0] as Record<
      string,
      unknown
    >;
    expect(sent.decision).toBe("approve");
    expect(sent.expectedRevision).toBe(3);
  });

  test("the composer sends a turn to the lead on Enter and keeps newlines on Shift+Enter", async () => {
    const bridge = scripted();
    bridge.on("turn.submit", { turn: { id: "t2" } });
    const user = userEvent.setup();
    mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    const box = await screen.findByLabelText("Message to the lead");
    await user.type(box, "first line{Shift>}{Enter}{/Shift}second line");
    expect(bridge.callsTo("turn.submit")).toHaveLength(0);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(bridge.callsTo("turn.submit")).toHaveLength(1));
    const sent = bridge.callsTo("turn.submit")[0] as Record<string, unknown>;
    expect(sent.text).toBe("first line\nsecond line");
    expect(sent.conversationId).toBe(CONVERSATION_ID);
    expect(sent.activateIfNeeded).toBe(true);
  });

  test("destructive worker actions ask for confirmation before sending", async () => {
    const bridge = scripted();
    bridge.on(
      "workspace.conversation",
      conversation({ workers: [workerCard()] }),
    );
    bridge.on("task.cancel", { ok: true });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    await screen.findByRole("tablist", { name: "Agents" });
    act(() => store.focusTab(TASK_ID));
    const cancel = await screen.findByRole("button", { name: "Cancel" });
    await user.click(cancel);
    expect(bridge.callsTo("task.cancel")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Confirm: Cancel" }));
    await waitFor(() => expect(bridge.callsTo("task.cancel")).toHaveLength(1));
  });
});

describe("finished workers", () => {
  test("a finished worker without a card session is looked up among suspended sessions for attach", async () => {
    const bridge = scripted();
    const suspendedId = "ababab00-0000-4000-8000-00000000ab01";
    bridge.on("session.list", [
      {
        id: suspendedId,
        employeeId: workerCard().employeeId,
        employeeConfigurationRevision: 1,
        provider: "codex",
        state: "suspended",
        revision: 4,
        ownerBootId: null,
        nativeSessionId: "thread-9",
        nativeHostPid: null,
        failureCode: null,
        failureMessage: null,
        createdAt: "2026-10-31T10:00:00Z",
        updatedAt: "2026-10-31T10:05:00Z",
        startedAt: "2026-10-31T10:00:00Z",
        stoppedAt: "2026-10-31T10:05:00Z",
      },
    ]);
    bridge.on(
      "workspace.conversation",
      conversation({
        workers: [
          workerCard({
            state: "complete",
            taskState: "completed",
            session: null,
            actions: [],
          }),
        ],
      }),
    );
    bridge.on("session.terminal.status", (params: unknown) =>
      terminalStatus({
        sessionId: String((params as { id: string }).id),
        viewable: false,
        viewBlockingCode: "LZ-SESSION-STATE",
        viewBlockingReason: "only daemon-owned idle sessions can be viewed",
        attachable: true,
        blockingCode: null,
        blockingReason: null,
      }),
    );
    const user = userEvent.setup();
    const { store } = mount(bridge);
    await user.click(await screen.findByText("fix flaky daemon test"));
    await screen.findByRole("tablist", { name: "Agents" });
    act(() => store.focusTab(TASK_ID));
    await waitFor(() =>
      expect(
        bridge
          .callsTo("session.terminal.status")
          .some((p) => (p as { id: string }).id === suspendedId),
      ).toBe(true),
    );
    expect(
      await screen.findByRole("button", { name: "Attach and take over" }),
    ).toBeInTheDocument();
  });
});

describe("inbox", () => {
  test("lists items with their actions and deep-links into the conversation", async () => {
    const bridge = scripted();
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
      eventHighWater: 40,
      truncated: false,
      nextBefore: null,
    });
    const user = userEvent.setup();
    const { store, container } = mount(bridge);
    await user.click(await screen.findByRole("button", { name: /^Inbox/ }));
    expect(
      await screen.findByRole("button", { name: "Approve once" }),
    ).toBeInTheDocument();
    await expectNoSeriousViolations(container);
    await user.click(screen.getByRole("button", { name: /Open wants to run/ }));
    expect(store.getSnapshot().route).toEqual({
      kind: "conversation",
      id: CONVERSATION_ID,
      tab: TASK_ID,
    });
  });
});

describe("offline", () => {
  test("a missing bridge is stated plainly and nothing is simulated", async () => {
    const bridge = createFakeBridge();
    for (const method of [
      "system.status",
      "project.list",
      "workspace.conversations",
      "workspace.launcher",
      "inbox.counts",
      "native.trust.list",
    ]) {
      bridge.fail(method, {
        code: "LZ-DESKTOP-BRIDGE-MISSING",
        message: "The secure Tauri bridge is unavailable in this window.",
      });
    }
    bridge.subscribeEvents = async () => {
      throw {
        code: "LZ-DESKTOP-BRIDGE-MISSING",
        message: "The secure Tauri bridge is unavailable in this window.",
      };
    };
    mount(bridge);
    expect(await screen.findByText("Daemon offline")).toBeInTheDocument();
    expect(
      screen.getAllByText(/secure Tauri bridge is unavailable/).length,
    ).toBeGreaterThan(0);
  });
});
