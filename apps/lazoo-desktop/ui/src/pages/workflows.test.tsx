import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { App } from "../App";
import { Store } from "../lib/store";
import { edge, node, workflow } from "../lib/workflows/fixtures";
import { createFakeBridge, type FakeBridge } from "../test/fakeBridge";
import {
  conversation,
  conversations,
  launcher,
  project,
  status,
  terminalStatus,
  turnRecord,
} from "../test/fixtures";

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

function mount(bridge: FakeBridge) {
  const store = new Store(bridge, { statusPollMs: 0, debounceMs: 10 });
  const view = render(<App store={store} bridge={bridge} />);
  return { store, ...view };
}

type User = ReturnType<typeof userEvent.setup>;

/** A manual trigger into a notice: it runs without any daemon call. */
function manualWorkflow() {
  return workflow(
    [
      node("t", "trigger.manual"),
      node("n", "notify", { title: "Ping", body: "hello" }),
    ],
    [edge("t", "n")],
    { name: "Test", enabled: false },
  );
}

function stored(
  bridge: FakeBridge,
  ...workflows: ReturnType<typeof workflow>[]
) {
  bridge.workflows.stored.document = {
    version: 1,
    workflows,
    runs: [],
    lastFired: {},
  };
}

async function openWorkflows(user: User) {
  await user.click(await screen.findByRole("button", { name: /^Workflows/ }));
  return screen.findByRole("heading", { name: "Workflows" });
}

/** React Flow lays nothing out in jsdom, so a node is selected by a plain click on it. */
function clickNode(container: HTMLElement, id: string) {
  const element = container.querySelector(`.react-flow__node[data-id="${id}"]`);
  expect(element).not.toBeNull();
  fireEvent.click(element!);
}

beforeEach(() => {
  localStorage.clear();
});

describe("workflows index", () => {
  test("with nothing saved it offers the templates and starting from scratch", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await openWorkflows(user);
    expect(await screen.findByText("No workflows yet")).toBeInTheDocument();
    expect(screen.getByText("Preview")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Use template: Daily summary" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Use template: Nightly checks" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Start from scratch" }),
    ).toBeInTheDocument();
  });

  test("lists a saved workflow with its summary and next run", async () => {
    const bridge = scripted();
    stored(bridge, manualWorkflow());
    const user = userEvent.setup();
    mount(bridge);
    await openWorkflows(user);
    expect(
      await screen.findByRole("button", { name: "Test" }),
    ).toBeInTheDocument();
    expect(screen.getByText(/Manual · 1 step/)).toBeInTheDocument();
    expect(screen.getByText("Never run")).toBeInTheDocument();
    expect(screen.getByText("Off")).toBeInTheDocument();
  });

  test("deleting asks for confirmation first", async () => {
    const bridge = scripted();
    stored(bridge, manualWorkflow());
    const user = userEvent.setup();
    mount(bridge);
    await openWorkflows(user);
    await user.click(
      await screen.findByRole("button", { name: "Actions for Test" }),
    );
    await user.click(screen.getByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog", { name: /Delete Test/ });
    // Cancelling keeps it.
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(screen.queryByRole("dialog")).not.toBeInTheDocument(),
    );
    expect(screen.getByRole("button", { name: "Test" })).toBeInTheDocument();
    // Confirming removes it.
    await user.click(screen.getByRole("button", { name: "Actions for Test" }));
    await user.click(screen.getByRole("menuitem", { name: "Delete" }));
    await user.click(
      within(await screen.findByRole("dialog")).getByRole("button", {
        name: "Delete",
      }),
    );
    expect(await screen.findByText("No workflows yet")).toBeInTheDocument();
  });
});

describe("workflow editor", () => {
  async function openTemplate(user: User) {
    await openWorkflows(user);
    await user.click(
      await screen.findByRole("button", {
        name: "Use template: Daily summary",
      }),
    );
    return screen.findByRole("toolbar", { name: "Workflow" });
  }

  test("a template opens with its steps", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { container } = mount(bridge);
    await openTemplate(user);
    for (const id of ["schedule", "agent", "notify"]) {
      expect(
        container.querySelector(`.react-flow__node[data-id="${id}"]`),
      ).not.toBeNull();
    }
    expect(screen.getAllByText("Summarise yesterday").length).toBeGreaterThan(
      0,
    );
    expect(await screen.findByText("Saved")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run now" })).toBeInTheDocument();
  });

  test("turning on a workflow with problems is refused and says why", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await openTemplate(user);
    expect(screen.getByRole("button", { name: /1 issue/ })).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: "Enable workflow" });
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-checked", "false");
    expect(
      await screen.findByText("Choose an agent for this step."),
    ).toBeInTheDocument();
  });

  test("Run now is refused on a workflow with problems and selects the step", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await openTemplate(user);
    await user.click(screen.getByRole("button", { name: "Run now" }));
    expect(
      await screen.findByText("Choose an agent for this step."),
    ).toBeInTheDocument();
    expect(screen.getByText("Daily summary was not run")).toBeInTheDocument();
    const inspector = await screen.findByRole("complementary", {
      name: "Inspector",
    });
    expect(within(inspector).getByLabelText("Step name")).toHaveValue(
      "Summarise yesterday",
    );
    expect(bridge.callsTo("conversation.create")).toHaveLength(0);
    expect(screen.queryByText("Succeeded")).not.toBeInTheDocument();
  });

  test("the issues menu selects the step it is about", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    mount(bridge);
    await openTemplate(user);
    await user.click(screen.getByRole("button", { name: /1 issue/ }));
    await user.click(
      screen.getByRole("menuitem", { name: "Choose an agent for this step." }),
    );
    const inspector = await screen.findByRole("complementary", {
      name: "Inspector",
    });
    expect(within(inspector).getByLabelText("Step name")).toHaveValue(
      "Summarise yesterday",
    );
    expect(within(inspector).getByLabelText("Agent")).toBeInTheDocument();
  });

  test("editing a schedule in the inspector updates its summary", async () => {
    const bridge = scripted();
    const user = userEvent.setup();
    const { container } = mount(bridge);
    await openTemplate(user);
    clickNode(container, "schedule");
    const inspector = await screen.findByRole("complementary", {
      name: "Inspector",
    });
    expect(
      within(inspector).getByText("Weekdays at 09:00"),
    ).toBeInTheDocument();
    fireEvent.change(within(inspector).getByLabelText("Time"), {
      target: { value: "10:30" },
    });
    expect(
      within(inspector).getByText("Weekdays at 10:30"),
    ).toBeInTheDocument();
    await user.click(
      within(inspector).getByRole("button", { name: "Every day" }),
    );
    expect(
      within(inspector).getByText("Every day at 10:30"),
    ).toBeInTheDocument();
    // The node on the canvas reads the same.
    expect(
      container.querySelector(
        '.react-flow__node[data-id="schedule"] .wf-node__headline',
      ),
    ).toHaveTextContent("Every day at 10:30");
  });

  test("Run now records a run that the Runs panel lists", async () => {
    const bridge = scripted();
    stored(bridge, manualWorkflow());
    const user = userEvent.setup();
    mount(bridge);
    await openWorkflows(user);
    await user.click(await screen.findByRole("button", { name: "Test" }));
    await user.click(await screen.findByRole("button", { name: "Runs" }));
    const panel = await screen.findByRole("complementary", { name: "Runs" });
    expect(within(panel).getByText("No runs yet.")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Run now" }));
    expect(await within(panel).findByText("Succeeded")).toBeInTheDocument();
    expect(within(panel).getByText(/Manual/)).toBeInTheDocument();
    expect(await screen.findByText("hello")).toBeInTheDocument();
  });

  test("renaming commits on Enter and the workflow is saved", async () => {
    const bridge = scripted();
    stored(bridge, manualWorkflow());
    const user = userEvent.setup();
    mount(bridge);
    await openWorkflows(user);
    await user.click(await screen.findByRole("button", { name: "Test" }));
    await user.click(
      await screen.findByRole("button", { name: "Rename workflow: Test" }),
    );
    const input = screen.getByLabelText("Workflow name");
    await user.clear(input);
    await user.type(input, "Morning check{Enter}");
    expect(
      await screen.findByRole("button", {
        name: "Rename workflow: Morning check",
      }),
    ).toBeInTheDocument();
    await waitFor(() => expect(bridge.workflows.save).toHaveBeenCalled());
    expect(
      (bridge.workflows.stored.document as { workflows: { name: string }[] })
        .workflows[0]!.name,
    ).toBe("Morning check");
  });
});

describe("workflows in the shell", () => {
  test("the command palette goes to Workflows and lists each workflow", async () => {
    const bridge = scripted();
    stored(bridge, manualWorkflow());
    const user = userEvent.setup();
    mount(bridge);
    await screen.findByLabelText("Open command palette");
    await screen.findByRole("button", { name: /^Workflows/ });
    await user.keyboard("{Control>}k{/Control}");
    const search = await screen.findByLabelText("Search commands");
    await user.type(search, "workflow");
    expect(
      await screen.findByRole("option", { name: /^Workflows/ }),
    ).toBeInTheDocument();
    await user.clear(search);
    await user.type(search, "Test");
    await act(async () => undefined);
    expect(
      await screen.findByRole("option", { name: /Test/ }),
    ).toBeInTheDocument();
  });
});
