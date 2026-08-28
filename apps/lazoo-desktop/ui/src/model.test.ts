import { describe, expect, test } from "vitest";

import {
  buildCommandGroups,
  buildViewModel,
  createInitialDesktopState,
  describeSelection,
  formatCheckoutMode,
  formatRelativeTime,
  formatTimestamp,
  formatUptime,
  humanizeEventType,
  middleTruncate,
  reduceDesktopState,
  summarizeEvent,
  type CommandDefinition,
  type DesktopSnapshot,
} from "./model";

const NOW = Date.parse("2026-08-28T12:00:00Z");

function snapshot(
  statusOverrides: Partial<DesktopSnapshot["status"]> = {},
): DesktopSnapshot {
  return {
    capturedAt: "2026-08-28T11:59:55Z",
    status: {
      daemonVersion: "0.1.0",
      bootId: "00000000-0000-0000-0000-000000000001",
      uptimeMillis: 3_660_000,
      protocolVersion: 1,
      dataRoot: "C:\\Users\\owner\\AppData\\Local\\Lazoo",
      schemaVersion: 1,
      journalMode: "wal",
      projectCount: 1,
      eventHighWater: 7,
      pendingOutbox: 0,
      emergencyIncidentCount: 0,
      keeper: null,
      degraded: [],
      ...statusOverrides,
    },
    projects: [
      {
        id: "00000000-0000-0000-0000-000000000002",
        repositoryIdentity: "example",
        canonicalPath: "C:\\workspace\\Example",
        displayName: "Example",
        checkoutMode: "isolated",
        configurationRevision: 1,
        createdAt: "2026-08-28T11:58:00Z",
      },
    ],
    events: [
      {
        sequence: 7,
        id: "00000000-0000-0000-0000-000000000003",
        eventType: "project.registered",
        payload: { displayName: "Example" },
        createdAt: "2026-08-28T11:59:00Z",
      },
      {
        sequence: 4,
        id: "00000000-0000-0000-0000-000000000004",
        eventType: "daemon.started",
        payload: { summary: "Daemon came online" },
        createdAt: "2026-08-28T11:50:00Z",
      },
    ],
  };
}

describe("formatting", () => {
  test("formats daemon uptime without sub-minute noise", () => {
    expect(formatUptime(0)).toBe("< 1 min");
    expect(formatUptime(59_999)).toBe("< 1 min");
    expect(formatUptime(3_660_000)).toBe("1 hr 1 min");
    expect(formatUptime(183_600_000)).toBe("2 days 3 hrs");
  });

  test("formats timestamps and relative time defensively", () => {
    expect(formatRelativeTime("2026-08-28T11:59:30Z", NOW)).toBe("30 sec ago");
    expect(formatRelativeTime("2026-08-28T11:55:00Z", NOW)).toBe("5 min ago");
    expect(formatRelativeTime("not-a-date", NOW)).toBe("Time unavailable");
    expect(formatTimestamp("not-a-date")).toBe("Time unavailable");
    expect(formatTimestamp("2026-08-28T11:59:30Z")).toContain("2026");
  });

  test("labels checkout modes and event types for humans", () => {
    expect(formatCheckoutMode("isolated")).toBe("Isolated worktree");
    expect(formatCheckoutMode("lead-main")).toBe("Lead uses main");
    expect(formatCheckoutMode("future-mode")).toBe("Future mode");
    expect(humanizeEventType("project.registered")).toBe("Project registered");
    expect(humanizeEventType("LZ_DAEMON_RECOVERED")).toBe(
      "LZ daemon recovered",
    );
  });

  test("summarizes events without dumping unbounded payloads", () => {
    expect(
      summarizeEvent({
        eventType: "project.registered",
        payload: { displayName: "Orchestrator" },
      }),
    ).toBe("Orchestrator joined the zoo");
    expect(
      summarizeEvent({
        eventType: "incident.recovered",
        payload: { code: "LZ-DAEMON-UNAVAILABLE" },
      }),
    ).toBe("Recovered LZ-DAEMON-UNAVAILABLE");
    expect(
      summarizeEvent({
        eventType: "custom.event",
        payload: { detail: "x".repeat(200) },
      }),
    ).toMatch(/^x{120,}…$/u);
  });

  test("truncates long technical paths through the middle", () => {
    const value = middleTruncate(
      "C:\\workspace\\very-long-project-name\\.git\\worktrees\\agent",
      34,
    );

    expect(value.length).toBeLessThanOrEqual(34);
    expect(value).toMatch(/^C:\\workspace/u);
    expect(value).toMatch(/worktrees\\agent$/u);
    expect(value).toContain("…");
  });
});

describe("desktop view model", () => {
  test("builds healthy state and sorts newest activity first", () => {
    const model = buildViewModel(snapshot(), NOW);

    expect(model.health).toEqual({
      tone: "healthy",
      label: "Control plane steady",
      detail: "Daemon and event ledger are responding.",
    });
    expect(model.lastUpdated).toBe("5 sec ago");
    expect(model.projects[0]?.checkoutLabel).toBe("Isolated worktree");
    expect(model.events.map((event) => event.sequence)).toEqual([7, 4]);
    expect(model.events[0]?.summary).toBe("Example joined the zoo");
    expect(model.system.uptime).toBe("1 hr 1 min");
    expect(model.system.journalMode).toBe("WAL");
  });

  test("distinguishes daemon degradation from recovered evidence", () => {
    const degraded = buildViewModel(
      snapshot({ degraded: ["outbox delayed"] }),
      NOW,
    );
    const recovered = buildViewModel(
      snapshot({ emergencyIncidentCount: 2 }),
      NOW,
    );

    expect(degraded.health).toMatchObject({
      tone: "warning",
      label: "Attention needed",
    });
    expect(degraded.health.detail).toBe("outbox delayed");
    expect(recovered.health).toMatchObject({
      tone: "warning",
      label: "Recovery evidence found",
    });
    expect(recovered.health.detail).toContain("2 recovered incidents");
  });

  test("handles empty and malformed snapshot collections defensively", () => {
    const empty = snapshot({ projectCount: 0 });
    empty.projects = [];
    empty.events = [];
    empty.capturedAt = "invalid";

    const model = buildViewModel(empty, NOW);

    expect(model.projects).toEqual([]);
    expect(model.events).toEqual([]);
    expect(model.capturedAt).toBe("Time unavailable");
    expect(model.metrics.projects).toBe(0);
  });

  test("describes valid project and event selections", () => {
    const model = buildViewModel(snapshot(), NOW);

    expect(
      describeSelection(model, {
        kind: "project",
        id: "00000000-0000-0000-0000-000000000002",
      }),
    ).toMatchObject({ kindLabel: "Project", label: "Example" });
    expect(
      describeSelection(model, {
        kind: "event",
        id: "00000000-0000-0000-0000-000000000003",
      }),
    ).toMatchObject({ kindLabel: "Event", label: "Project registered" });
    expect(
      describeSelection(model, { kind: "project", id: "missing" }),
    ).toBeNull();
  });
});

describe("desktop state", () => {
  test("moves from loading to healthy or degraded connection truth", () => {
    const initial = createInitialDesktopState();
    const connected = reduceDesktopState(initial, {
      type: "loadSucceeded",
      snapshot: snapshot(),
      now: NOW,
    });
    const degraded = reduceDesktopState(connected, {
      type: "loadSucceeded",
      snapshot: snapshot({ degraded: ["outbox delayed"] }),
      now: NOW,
    });

    expect(initial).toMatchObject({
      connection: "loading",
      isRefreshing: true,
    });
    expect(connected).toMatchObject({
      connection: "connected",
      isRefreshing: false,
    });
    expect(degraded).toMatchObject({
      connection: "degraded",
      isRefreshing: false,
    });
  });

  test("retains the last good model after a later failure", () => {
    const connected = reduceDesktopState(createInitialDesktopState(), {
      type: "loadSucceeded",
      snapshot: snapshot(),
      now: NOW,
    });
    const refreshing = reduceDesktopState(connected, { type: "loadStarted" });
    const failed = reduceDesktopState(refreshing, {
      type: "loadFailed",
      failure: {
        code: "LZ-DESKTOP-DAEMON-OFFLINE",
        message: "Daemon unavailable",
      },
    });

    expect(refreshing).toMatchObject({
      connection: "connected",
      isRefreshing: true,
    });
    expect(failed).toMatchObject({
      connection: "offline",
      isRefreshing: false,
    });
    expect(failed.model).toBe(connected.model);
    expect(failed.failure?.code).toBe("LZ-DESKTOP-DAEMON-OFFLINE");
  });
});

describe("command results", () => {
  const commands: CommandDefinition[] = [
    {
      id: "navigate-control-room",
      label: "Control room",
      detail: "Overview",
      group: "Navigate",
      keywords: ["home"],
    },
    {
      id: "refresh",
      label: "Refresh snapshot",
      detail: "Sync daemon state",
      group: "Act",
    },
    {
      id: "inspect-project",
      label: "Inspect selected project",
      detail: "Open project details",
      group: "Inspect",
      selectionKind: "project",
    },
  ];

  test("ranks contextual groups before global navigation", () => {
    const groups = buildCommandGroups(commands, "", {
      view: "projects",
      selection: { kind: "project", id: "project-1" },
    });

    expect(groups.map((group) => group.label)).toEqual([
      "Inspect",
      "Act",
      "Navigate",
    ]);
    expect(groups[0]?.commands[0]?.id).toBe("inspect-project");
  });

  test("filters labels, details, and keywords while hiding invalid contextual actions", () => {
    const sync = buildCommandGroups(commands, "sync", {
      view: "control",
      selection: null,
    });
    const inspectWithoutSelection = buildCommandGroups(commands, "inspect", {
      view: "control",
      selection: null,
    });
    const home = buildCommandGroups(commands, "home", {
      view: "control",
      selection: null,
    });

    expect(
      sync.flatMap((group) => group.commands).map((command) => command.id),
    ).toEqual(["refresh"]);
    expect(inspectWithoutSelection).toEqual([]);
    expect(home[0]?.commands[0]?.id).toBe("navigate-control-room");
  });
});
