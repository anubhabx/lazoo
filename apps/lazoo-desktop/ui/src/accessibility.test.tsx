import { render, screen } from "@testing-library/react";
import axe from "axe-core";
import { describe, expect, test, vi } from "vitest";

import { App } from "./App";
import type { DesktopBridge } from "./bridge";
import type { DesktopSnapshot } from "./model";
import { desktopSnapshot } from "./test/fixtures";

function bridge(loadSnapshot: () => Promise<DesktopSnapshot>): DesktopBridge {
  return {
    loadSnapshot: vi.fn(loadSnapshot),
    close: vi.fn().mockResolvedValue(undefined),
    minimize: vi.fn().mockResolvedValue(undefined),
    toggleMaximize: vi.fn().mockResolvedValue(undefined),
    startDragging: vi.fn().mockResolvedValue(undefined),
    isMaximized: vi.fn().mockResolvedValue(false),
  };
}

async function expectNoSeriousViolations(container: HTMLElement) {
  const results = await axe.run(container, {
    runOnly: {
      type: "tag",
      values: ["wcag2a", "wcag2aa"],
    },
  });
  const seriousViolations = results.violations.filter(
    (violation) =>
      violation.impact === "critical" || violation.impact === "serious",
  );
  expect(seriousViolations).toEqual([]);
}

describe("desktop accessibility states", () => {
  test("loading has no serious or critical violations", async () => {
    const pending = new Promise<DesktopSnapshot>(() => undefined);
    const { container } = render(
      <App bridge={bridge(() => pending)} refreshIntervalMs={0} />,
    );

    expect(screen.getByText("Reading project registry")).toBeInTheDocument();
    await expectNoSeriousViolations(container);
  });

  test("healthy has no serious or critical violations", async () => {
    const { container } = render(
      <App
        bridge={bridge(() => Promise.resolve(desktopSnapshot()))}
        refreshIntervalMs={0}
      />,
    );

    await screen.findByText("Control plane steady");
    await expectNoSeriousViolations(container);
  });

  test("empty has no serious or critical violations", async () => {
    const snapshot = desktopSnapshot({
      status: { projectCount: 0, eventHighWater: 0 },
      projects: [],
      events: [],
    });
    const { container } = render(
      <App
        bridge={bridge(() => Promise.resolve(snapshot))}
        refreshIntervalMs={0}
      />,
    );

    await screen.findByText("No registered projects");
    await expectNoSeriousViolations(container);
  });

  test("degraded has no serious or critical violations", async () => {
    const snapshot = desktopSnapshot({
      status: { degraded: ["Outbox delivery is delayed"] },
    });
    const { container } = render(
      <App
        bridge={bridge(() => Promise.resolve(snapshot))}
        refreshIntervalMs={0}
      />,
    );

    await screen.findByRole("heading", { name: "Attention needed" });
    await expectNoSeriousViolations(container);
  });

  test("disconnected has no serious or critical violations", async () => {
    const { container } = render(
      <App
        bridge={bridge(() =>
          Promise.reject({
            code: "LZ-DESKTOP-DAEMON-OFFLINE",
            message: "Lazoo cannot reach the local daemon.",
          }),
        )}
        refreshIntervalMs={0}
      />,
    );

    await screen.findByRole("heading", { name: "Control room disconnected" });
    await expectNoSeriousViolations(container);
  });
});
