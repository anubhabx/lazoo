import { describe, expect, test, vi } from "vitest";

import { createDesktopBridge, type DesktopWindowAdapter } from "./bridge";

function windowAdapter(): DesktopWindowAdapter {
  return {
    close: vi.fn().mockResolvedValue(undefined),
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
    isMaximized: vi.fn().mockResolvedValue(false),
  };
}

describe("desktop bridge", () => {
  test("invokes only the fixed desktop snapshot command", async () => {
    const invoke = vi
      .fn()
      .mockResolvedValue({ capturedAt: "2026-08-28T12:00:00Z" });
    const bridge = createDesktopBridge({ invoke, window: windowAdapter() });

    await bridge.loadSnapshot();

    expect(invoke).toHaveBeenCalledOnce();
    expect(invoke).toHaveBeenCalledWith("load_desktop_snapshot");
  });

  test("reports a stable public failure when the Tauri runtime is absent", async () => {
    const bridge = createDesktopBridge({
      window: windowAdapter(),
      runtimeAvailable: () => false,
    });

    await expect(bridge.loadSnapshot()).rejects.toEqual({
      code: "LZ-DESKTOP-BRIDGE-MISSING",
      message: "The secure Tauri bridge is unavailable in this window.",
    });
  });

  test("forwards native window controls through the narrow adapter", async () => {
    const adapter = windowAdapter();
    const bridge = createDesktopBridge({
      invoke: vi.fn(),
      window: adapter,
      runtimeAvailable: () => true,
    });

    await bridge.minimize();
    await bridge.toggleMaximize();
    await bridge.startDragging();
    await bridge.close();
    await expect(bridge.isMaximized()).resolves.toBe(false);

    expect(adapter.minimize).toHaveBeenCalledOnce();
    expect(adapter.toggleMaximize).toHaveBeenCalledOnce();
    expect(adapter.startDragging).toHaveBeenCalledOnce();
    expect(adapter.close).toHaveBeenCalledOnce();
    expect(adapter.isMaximized).toHaveBeenCalledOnce();
  });
});
