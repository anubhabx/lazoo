import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";

import { App } from "./App";
import type { DesktopBridge } from "./bridge";
import type { DesktopSnapshot } from "./model";

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
