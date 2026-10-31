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
  bridge.on("provider.accounts.list", {
    version: 1,
    accounts: [],
    policies: [],
  });
  bridge.on("provider.accounts.put", { ok: true });
  bridge.on("provider.accounts.remove", { ok: true });
  bridge.on("provider.accounts.policy", { ok: true });
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

describe("provider accounts", () => {
  test("verifies routing only on explicit click and includes the selected model", async () => {
    const bridge = scripted();
    bridge.on("provider.accounts.verify", {
      modelAnswered: true,
      routeVerified: true,
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await screen.findByRole("heading", { name: "Accounts" });
    expect(bridge.callsTo("provider.accounts.verify")).toHaveLength(0);
    await user.selectOptions(
      screen.getByLabelText("Routing provider"),
      "github-copilot",
    );
    await user.type(screen.getByLabelText("Verification model"), "test-model");
    await user.click(screen.getByRole("button", { name: "Verify routing" }));
    expect(await screen.findByText("Routing verified")).toBeInTheDocument();
    expect(bridge.callsTo("provider.accounts.verify")[0]).toMatchObject({
      provider: "github-copilot",
      model: "test-model",
    });
  });
  test("adds a redacted API account and clears its secret", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await screen.findByRole("heading", { name: "Accounts" });
    await user.type(screen.getByLabelText("Label"), "work key");
    await user.selectOptions(screen.getByLabelText("Credential"), "api-key");
    await user.type(screen.getByLabelText("Secret"), "do-not-keep");
    await user.click(screen.getByRole("button", { name: "Add account" }));
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.put")).toHaveLength(1),
    );
    const request = bridge.callsTo("provider.accounts.put")[0] as Record<
      string,
      unknown
    >;
    expect(request.secret).toBe("do-not-keep");
    expect(request.expectedRevision).toBe(0);
    expect(screen.getByLabelText("Secret")).toHaveValue("");
  });

  test("sends the current revision for policy edits", async () => {
    const bridge = scripted();
    bridge.on("provider.accounts.list", {
      version: 1,
      accounts: [],
      policies: [
        { provider: "codex", mode: "inherited", accountId: null, revision: 7 },
      ],
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await user.selectOptions(
      (await screen.findAllByLabelText("New sessions"))[0]!,
      "balanced",
    );
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.policy")).toHaveLength(1),
    );
    expect(bridge.callsTo("provider.accounts.policy")[0]).toMatchObject({
      provider: "codex",
      mode: "balanced",
      revision: 7,
    });
  });

  test("pins to an enabled account and clears the pin when returning to inherited", async () => {
    const bridge = scripted();
    let policy = {
      provider: "codex",
      mode: "inherited",
      accountId: null,
      revision: 7,
    };
    const account = {
      spec: {
        id: "pin-me",
        provider: "codex",
        label: "Primary",
        credentialKind: "native-profile",
        profileDirectory: "",
        credentialEnvironment: null,
        baseUrl: null,
        apiProtocol: null,
        backendProvider: null,
        models: [],
        enabled: true,
      },
      revision: 1,
      hasSecret: false,
      deleted: false,
      selections: 0,
      requests: 0,
      failures: 0,
      cooldownUntil: 0,
      lastStatus: null,
      lastLatencyMillis: null,
      updatedAt: "2026-11-01T00:00:00Z",
    };
    bridge.on("provider.accounts.list", () => ({
      version: 1,
      accounts: [account],
      policies: [policy],
    }));
    bridge.on("provider.accounts.policy", (params: unknown) => {
      policy = { ...(params as typeof policy), revision: policy.revision + 1 };
      return policy;
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    const mode = await screen.findByLabelText("New sessions");
    await user.selectOptions(mode, "pinned");
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.policy")[0]).toMatchObject({
        provider: "codex",
        mode: "pinned",
        accountId: "pin-me",
        revision: 7,
      }),
    );
    await user.selectOptions(mode, "inherited");
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.policy")[1]).toMatchObject({
        provider: "codex",
        mode: "inherited",
        accountId: null,
      }),
    );
  });

  test("hides and clears Copilot custom base URLs", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await screen.findByRole("heading", { name: "Accounts" });
    await user.selectOptions(screen.getByLabelText("Credential"), "api-key");
    await user.type(screen.getByLabelText("Base URL"), "https://example.test");
    await user.selectOptions(
      screen.getByLabelText("Provider"),
      "github-copilot",
    );
    expect(screen.queryByLabelText("Base URL")).not.toBeInTheDocument();
    await user.type(screen.getByLabelText("Label"), "Copilot token");
    await user.type(screen.getByLabelText("Secret"), "token");
    await user.click(screen.getByRole("button", { name: "Add account" }));
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.put")).toHaveLength(1),
    );
    expect(
      (
        bridge.callsTo("provider.accounts.put")[0] as {
          spec: { baseUrl: string | null };
        }
      ).spec.baseUrl,
    ).toBeNull();
  });

  test("shows a stale account revision error instead of hiding it", async () => {
    const bridge = scripted();
    bridge.fail("provider.accounts.policy", {
      code: "LZ-REVISION-CONFLICT",
      message: "the account policy changed in another window",
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await user.selectOptions(
      (await screen.findAllByLabelText("New sessions"))[0]!,
      "balanced",
    );
    expect(
      await screen.findByText("the account policy changed in another window"),
    ).toBeInTheDocument();
  });

  test("opens a writable native provider sign-in terminal", async () => {
    const bridge = scripted();
    const native = {
      spec: {
        id: "account-1",
        provider: "github-copilot",
        label: "Personal",
        credentialKind: "native-profile",
        profileDirectory: "",
        credentialEnvironment: null,
        baseUrl: null,
        apiProtocol: null,
        backendProvider: null,
        models: [],
        enabled: true,
      },
      revision: 3,
      hasSecret: false,
      deleted: false,
      selections: 0,
      requests: 0,
      failures: 0,
      cooldownUntil: 0,
      lastStatus: null,
      lastLatencyMillis: null,
      updatedAt: "2026-11-01T00:00:00Z",
    };
    bridge.on("provider.accounts.list", {
      version: 1,
      accounts: [native],
      policies: [],
    });
    bridge.on("provider.accounts.login", {
      id: "login-term",
      bootId: "boot",
      pid: 1,
      executable: "copilot",
      args: [],
      cwd: "",
      columns: 100,
      rows: 30,
      revision: 1,
      live: true,
      exitCode: null,
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await user.click(await screen.findByRole("button", { name: "Sign in" }));
    expect(
      await screen.findByRole("heading", { name: "Provider sign-in" }),
    ).toBeInTheDocument();
    expect(bridge.callsTo("provider.accounts.login")[0]).toMatchObject({
      id: "account-1",
      expectedRevision: 3,
    });
  });

  test("rotates an API key with the account revision and clears the entered secret", async () => {
    const bridge = scripted();
    const account = {
      spec: {
        id: "api-account-1",
        provider: "codex",
        label: "Work",
        credentialKind: "api-key",
        profileDirectory: "",
        credentialEnvironment: "OPENAI_API_KEY",
        baseUrl: null,
        apiProtocol: "responses",
        backendProvider: null,
        models: [],
        enabled: true,
      },
      revision: 4,
      hasSecret: true,
      deleted: false,
      selections: 0,
      requests: 3,
      failures: 0,
      cooldownUntil: 0,
      lastStatus: 200,
      lastLatencyMillis: 20,
      updatedAt: "2026-11-01T00:00:00Z",
    };
    bridge.on("provider.accounts.list", {
      version: 1,
      accounts: [account],
      policies: [],
    });
    const user = userEvent.setup();
    const { store } = mount(bridge);
    store.navigate({ kind: "settings", section: "accounts" });
    await user.click(await screen.findByRole("button", { name: "Rotate key" }));
    const secret = screen.getByLabelText("New secret for Work");
    await user.type(secret, "replacement-key");
    await user.click(screen.getByRole("button", { name: "Save new key" }));
    await waitFor(() =>
      expect(bridge.callsTo("provider.accounts.put")).toHaveLength(1),
    );
    expect(bridge.callsTo("provider.accounts.put")[0]).toMatchObject({
      spec: account.spec,
      expectedRevision: 4,
      secret: "replacement-key",
    });
    await waitFor(() =>
      expect(
        screen.queryByLabelText("New secret for Work"),
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

describe("projects in the sidebar", () => {
  test("a project opens its home, expands its conversations, and reads them by project", async () => {
    const bridge = scripted();
    bridge.on("workspace.conversations", (params: unknown) => {
      const projectId = (params as { projectId: string | null }).projectId;
      return projectId
        ? {
            ...conversations([
              {
                ...conversations().conversations[0]!,
                id: "33333333-3333-4333-8333-333333333399",
                title: "tidy the project readme",
              },
            ]),
            projectId,
          }
        : conversations();
    });
    bridge.on("workspace.launcher", (params: unknown) =>
      launcher({
        project: (params as { projectId: string | null }).projectId
          ? project()
          : null,
      }),
    );
    const user = userEvent.setup();
    const { store, container } = mount(bridge);
    const projects = await screen.findByRole("region", { name: "Projects" });
    await user.click(
      await within(projects).findByRole("button", {
        name: "Desktop acceptance",
      }),
    );
    expect(store.getSnapshot().spaceId).toBe(project().id);
    expect(
      await within(projects).findByText("tidy the project readme"),
    ).toBeInTheDocument();
    expect(
      within(projects).getByRole("button", {
        name: "Collapse Desktop acceptance",
      }),
    ).toHaveAttribute("aria-expanded", "true");
    const reads = bridge.callsTo("workspace.conversations") as {
      projectId: string | null;
    }[];
    expect(reads.some((read) => read.projectId === project().id)).toBe(true);
    await expectNoSeriousViolations(container);
  });

  test("Ctrl+B collapses and restores the sidebar", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { container } = mount(bridge);
    await screen.findByText("New work");
    const shell = container.querySelector(".shell")!;
    expect(shell).toHaveAttribute("data-sidebar", "open");
    await user.keyboard("{Control>}b{/Control}");
    expect(shell).toHaveAttribute("data-sidebar", "collapsed");
    await user.click(screen.getByRole("button", { name: "Show sidebar" }));
    expect(shell).toHaveAttribute("data-sidebar", "open");
  });
});

describe("first-run setup", () => {
  function withHarnesses(bridge: FakeBridge) {
    bridge.on("provider.list", [
      {
        provider: "claude-code",
        detected: true,
        executable: "claude.exe",
        discoverySource: "path",
      },
      {
        provider: "codex",
        detected: true,
        executable: "codex.cmd",
        discoverySource: "path",
      },
      {
        provider: "opencode",
        detected: false,
        executable: null,
        discoverySource: null,
      },
    ]);
    bridge.on("employee.list", []);
    bridge.on("employee.create", (params: unknown) => ({
      employee: { id: `id-${(params as { name: string }).name}` },
      rosterRevision: 2,
      eventSequence: 41,
    }));
    return bridge;
  }

  test("offers the recommended team once and creates every agent as data", async () => {
    const bridge = withHarnesses(scripted());
    const user = userEvent.setup();
    const { store, container, unmount } = mount(bridge);
    const dialog = await screen.findByRole("dialog", {
      name: "Set up your team",
    });
    expect(within(dialog).getByText("atlas")).toBeInTheDocument();
    expect(within(dialog).getByText("opus · high")).toBeInTheDocument();
    await expectNoSeriousViolations(container);
    await user.click(
      within(dialog).getByRole("button", { name: "Create 4 agents" }),
    );
    expect(
      await screen.findByRole("dialog", { name: "Your team is ready" }),
    ).toBeInTheDocument();
    const created = bridge.callsTo("employee.create") as Record<
      string,
      unknown
    >[];
    expect(created.map((params) => params.name)).toEqual([
      "atlas",
      "forge",
      "pixel",
      "sentinel",
    ]);
    expect(created[0]).toMatchObject({
      provider: "claude-code",
      role: "lead",
      model: "opus",
      reasoningEffort: "high",
      authorityMode: "self-decision",
      activationPolicy: "notice-turns",
      enabled: true,
    });
    expect(typeof created[0]!.idempotencyKey).toBe("string");
    expect(created[3]).toMatchObject({
      provider: "codex",
      model: "gpt-6-astra",
      checkoutPolicy: "read-only",
    });
    expect(store.getSnapshot().preferredLeadId).toBe("id-atlas");
    unmount();

    // Offered once: the next launch does not open it by itself.
    mount(withHarnesses(scripted()));
    await screen.findByText("New work");
    await waitFor(() =>
      expect(bridge.callsTo("provider.list").length).toBeGreaterThan(0),
    );
    expect(
      screen.queryByRole("dialog", { name: "Set up your team" }),
    ).not.toBeInTheDocument();
  });

  test("a row can be edited and left out before creating", async () => {
    const bridge = withHarnesses(scripted());
    const user = userEvent.setup();
    mount(bridge);
    const dialog = await screen.findByRole("dialog", {
      name: "Set up your team",
    });
    await user.click(
      within(dialog).getByRole("checkbox", { name: "Include pixel" }),
    );
    await user.click(
      within(dialog).getByRole("button", { name: "Edit atlas" }),
    );
    const name = await within(dialog).findByLabelText("Name");
    await user.clear(name);
    await user.type(name, "orion");
    await user.click(
      within(dialog).getByRole("button", { name: "Create 3 agents" }),
    );
    await screen.findByRole("dialog", { name: "Your team is ready" });
    const created = bridge.callsTo("employee.create") as { name: string }[];
    expect(created.map((params) => params.name)).toEqual([
      "orion",
      "forge",
      "sentinel",
    ]);
  });
});
