import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";

import { App } from "./App";
import type { DesktopBridge } from "./bridge";
import type { DesktopSnapshot } from "./model";
import { desktopSnapshot } from "./test/fixtures";

function pendingBridge() {
  const pendingSnapshot = new Promise<DesktopSnapshot>(() => undefined);
  return {
    loadSnapshot: vi.fn(() => pendingSnapshot),
    close: vi.fn().mockResolvedValue(undefined),
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
    isMaximized: vi.fn().mockResolvedValue(false),
  } satisfies DesktopBridge;
}

function bridgeWithSnapshot(value = desktopSnapshot()) {
  return {
    ...pendingBridge(),
    loadSnapshot: vi.fn().mockResolvedValue(value),
  } satisfies DesktopBridge;
}

function deferredSnapshot() {
  let resolve!: (snapshot: DesktopSnapshot) => void;
  let reject!: (failure: unknown) => void;
  const promise = new Promise<DesktopSnapshot>(
    (resolvePromise, rejectPromise) => {
      resolve = resolvePromise;
      reject = rejectPromise;
    },
  );
  return { promise, resolve, reject };
}

describe("desktop shell", () => {
  test("renders stable titlebar, navigation, workspace, and status landmarks while loading", () => {
    render(<App bridge={pendingBridge()} refreshIntervalMs={0} />);

    expect(
      screen.getByRole("banner", { name: "Application titlebar" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("navigation", { name: "Primary" }),
    ).toBeInTheDocument();
    expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "true");
    expect(
      screen.getByRole("status", { name: "Desktop status" }),
    ).toBeInTheDocument();
    expect(screen.getAllByText("Connecting")).toHaveLength(2);
    expect(screen.getByText("Reading project registry")).toBeInTheDocument();
  });

  test("exposes named custom window controls and forwards their actions", async () => {
    const user = userEvent.setup();
    const bridge = pendingBridge();
    render(<App bridge={bridge} refreshIntervalMs={0} />);

    await user.click(screen.getByRole("button", { name: "Minimize window" }));
    await user.click(
      screen.getByRole("button", { name: "Maximize or restore window" }),
    );
    await user.click(screen.getByRole("button", { name: "Close window" }));

    expect(bridge.minimize).toHaveBeenCalledOnce();
    expect(bridge.toggleMaximize).toHaveBeenCalledOnce();
    expect(bridge.close).toHaveBeenCalledOnce();
  });

  test("navigates among truthful read-only snapshot views", async () => {
    const user = userEvent.setup();
    render(<App bridge={pendingBridge()} refreshIntervalMs={0} />);

    expect(
      screen.getByRole("button", { name: "Control room" }),
    ).toHaveAttribute("aria-current", "page");
    await user.click(screen.getByRole("button", { name: "Projects" }));

    expect(screen.getByRole("button", { name: "Projects" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(
      screen.getByRole("heading", { level: 1, name: "Projects" }),
    ).toBeInTheDocument();
  });

  test("keeps the global command entry visible and keyboard reachable", async () => {
    const user = userEvent.setup();
    render(<App bridge={pendingBridge()} refreshIntervalMs={0} />);

    const commandTrigger = screen.getByRole("button", {
      name: "Open command palette",
    });
    expect(commandTrigger).toHaveTextContent("Search or run a command");
    await user.tab();
    expect(document.activeElement).not.toBe(document.body);
  });
});

describe("operational snapshot", () => {
  test("renders a healthy snapshot without replacing the shell", async () => {
    render(<App bridge={bridgeWithSnapshot()} refreshIntervalMs={0} />);

    expect(
      await screen.findByRole("heading", { name: "Control plane steady" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Example")).toBeInTheDocument();
    expect(screen.getByText("Project registered")).toBeInTheDocument();
    expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "false");
    expect(
      screen.getByRole("navigation", { name: "Primary" }),
    ).toBeInTheDocument();
  });

  test("surfaces explicit daemon degradation", async () => {
    const bridge = bridgeWithSnapshot(
      desktopSnapshot({ status: { degraded: ["Outbox delivery is delayed"] } }),
    );
    render(<App bridge={bridge} refreshIntervalMs={0} />);

    expect(
      await screen.findByRole("heading", { name: "Attention needed" }),
    ).toBeInTheDocument();
    expect(screen.getByText("Outbox delivery is delayed")).toBeInTheDocument();
    expect(screen.getAllByText("Needs attention")).toHaveLength(2);
  });

  test("renders a useful disconnected state when no snapshot exists", async () => {
    const bridge = {
      ...pendingBridge(),
      loadSnapshot: vi.fn().mockRejectedValue({
        code: "LZ-DESKTOP-DAEMON-OFFLINE",
        message: "Lazoo cannot reach the local daemon.",
      }),
    } satisfies DesktopBridge;
    render(<App bridge={bridge} refreshIntervalMs={0} />);

    expect(
      await screen.findByRole("heading", { name: "Control room disconnected" }),
    ).toBeInTheDocument();
    expect(screen.getByText("LZ-DESKTOP-DAEMON-OFFLINE")).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Retry connection" }),
    ).toBeInTheDocument();
  });

  test("keeps a successful snapshot visible after a later failure", async () => {
    const bridge = bridgeWithSnapshot();
    bridge.loadSnapshot
      .mockResolvedValueOnce(desktopSnapshot())
      .mockRejectedValueOnce({
        code: "LZ-DESKTOP-DAEMON-OFFLINE",
        message: "Daemon unavailable",
      });
    const user = userEvent.setup();
    render(<App bridge={bridge} refreshIntervalMs={0} />);
    await screen.findByText("Example");

    await user.click(screen.getByRole("button", { name: "Refresh snapshot" }));

    expect(
      await screen.findByText("Showing last known snapshot"),
    ).toBeInTheDocument();
    expect(screen.getByText("Example")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent(
      "LZ-DESKTOP-DAEMON-OFFLINE",
    );
  });

  test("marks manual refresh busy and prevents concurrent requests", async () => {
    const pending = deferredSnapshot();
    const bridge = bridgeWithSnapshot();
    bridge.loadSnapshot
      .mockResolvedValueOnce(desktopSnapshot())
      .mockReturnValueOnce(pending.promise);
    const user = userEvent.setup();
    render(<App bridge={bridge} refreshIntervalMs={0} />);
    await screen.findByText("Example");

    const refresh = screen.getByRole("button", { name: "Refresh snapshot" });
    await user.click(refresh);

    const busyRefresh = screen.getByRole("button", {
      name: "Refresh snapshot",
    });
    expect(busyRefresh).toBeDisabled();
    expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "true");
    fireEvent.click(busyRefresh);
    expect(bridge.loadSnapshot).toHaveBeenCalledTimes(2);

    pending.resolve(desktopSnapshot());
    await waitFor(() =>
      expect(screen.getByRole("main")).toHaveAttribute("aria-busy", "false"),
    );
  });

  test("explains empty project and activity collections", async () => {
    const bridge = bridgeWithSnapshot(
      desktopSnapshot({
        status: { projectCount: 0, eventHighWater: 0 },
        projects: [],
        events: [],
      }),
    );
    render(<App bridge={bridge} refreshIntervalMs={0} />);

    expect(
      await screen.findByText("No registered projects"),
    ).toBeInTheDocument();
    expect(screen.getByText("No recent activity")).toBeInTheDocument();
  });

  test("renders dedicated projects, activity, and system views from one snapshot", async () => {
    const user = userEvent.setup();
    render(<App bridge={bridgeWithSnapshot()} refreshIntervalMs={0} />);
    await screen.findByText("Example");

    await user.click(screen.getByRole("button", { name: "Projects" }));
    expect(
      screen.getByRole("heading", { level: 2, name: "Registered projects" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "Activity" }));
    expect(
      screen.getByRole("heading", { level: 2, name: "Recent activity" }),
    ).toBeVisible();
    await user.click(screen.getByRole("button", { name: "System" }));
    expect(
      screen.getByRole("heading", { level: 2, name: "System pulse" }),
    ).toBeVisible();
    expect(screen.getByText("v0.1.0")).toBeInTheDocument();
  });

  test("opens a project inspector and restores focus when it closes", async () => {
    const user = userEvent.setup();
    render(<App bridge={bridgeWithSnapshot()} refreshIntervalMs={0} />);
    const project = await screen.findByRole("option", { name: /^Example/u });

    await user.click(project);
    expect(
      screen.getByRole("complementary", { name: "Inspector" }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: "Example" }),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close inspector" }));

    expect(
      screen.queryByRole("complementary", { name: "Inspector" }),
    ).not.toBeInTheDocument();
    expect(project).toHaveFocus();
  });

  test("opens an event inspector and dismisses it with Escape", async () => {
    const user = userEvent.setup();
    render(<App bridge={bridgeWithSnapshot()} refreshIntervalMs={0} />);
    await screen.findByText("Example");
    const event = screen.getByRole("option", { name: /^Project registered/u });

    await user.click(event);
    expect(
      screen.getByRole("complementary", { name: "Inspector" }),
    ).toBeInTheDocument();
    await user.keyboard("{Escape}");

    expect(
      screen.queryByRole("complementary", { name: "Inspector" }),
    ).not.toBeInTheDocument();
    expect(event).toHaveFocus();
  });

  test("refreshes on the configured interval only while visible", async () => {
    vi.useFakeTimers();
    try {
      const bridge = bridgeWithSnapshot();
      render(<App bridge={bridge} refreshIntervalMs={10_000} />);

      await act(async () => Promise.resolve());
      expect(bridge.loadSnapshot).toHaveBeenCalledTimes(1);
      await act(async () => vi.advanceTimersByTimeAsync(10_000));
      expect(bridge.loadSnapshot).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  test("refreshes when a hidden window becomes visible again", async () => {
    const bridge = bridgeWithSnapshot();
    render(<App bridge={bridge} refreshIntervalMs={0} />);
    await waitFor(() => expect(bridge.loadSnapshot).toHaveBeenCalledTimes(1));

    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
    expect(bridge.loadSnapshot).toHaveBeenCalledTimes(1);
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      value: "visible",
    });
    document.dispatchEvent(new Event("visibilitychange"));

    await waitFor(() => expect(bridge.loadSnapshot).toHaveBeenCalledTimes(2));
    Reflect.deleteProperty(document, "visibilityState");
  });
});
