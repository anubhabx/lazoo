import type { DesktopSnapshot } from "../model";

export const PROJECT_ID = "00000000-0000-0000-0000-000000000002";
export const EVENT_ID = "00000000-0000-0000-0000-000000000003";

interface SnapshotOverrides {
  capturedAt?: string;
  status?: Partial<DesktopSnapshot["status"]>;
  projects?: DesktopSnapshot["projects"];
  events?: DesktopSnapshot["events"];
}

export function desktopSnapshot(
  overrides: SnapshotOverrides = {},
): DesktopSnapshot {
  return {
    capturedAt: overrides.capturedAt ?? "2026-08-28T11:59:55Z",
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
      ...overrides.status,
    },
    projects: overrides.projects ?? [
      {
        id: PROJECT_ID,
        repositoryIdentity: "example",
        canonicalPath: "C:\\workspace\\Example",
        displayName: "Example",
        checkoutMode: "isolated",
        configurationRevision: 1,
        createdAt: "2026-08-28T11:58:00Z",
      },
    ],
    events: overrides.events ?? [
      {
        sequence: 7,
        id: EVENT_ID,
        eventType: "project.registered",
        payload: { displayName: "Example" },
        createdAt: "2026-08-28T11:59:00Z",
      },
    ],
  };
}
