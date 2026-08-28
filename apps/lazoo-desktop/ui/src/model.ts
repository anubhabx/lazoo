const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const SUMMARY_LIMIT = 122;

export type ConnectionState = "loading" | "connected" | "degraded" | "offline";
export type ViewId = "control" | "projects" | "activity" | "system";
export type Selection = { kind: "project" | "event"; id: string };

export interface DesktopFailure {
  code: string;
  message: string;
}

export interface DesktopSnapshot {
  capturedAt: string;
  status: {
    daemonVersion: string;
    bootId: string;
    uptimeMillis: number;
    protocolVersion: number;
    dataRoot: string;
    schemaVersion: number;
    journalMode: string;
    projectCount: number;
    eventHighWater: number;
    pendingOutbox: number;
    emergencyIncidentCount: number;
    keeper: unknown;
    degraded: string[];
  };
  projects: Array<{
    id: string;
    repositoryIdentity: string;
    canonicalPath: string;
    displayName: string;
    checkoutMode: string;
    configurationRevision: number;
    createdAt: string;
  }>;
  events: Array<{
    sequence: number;
    id: string;
    eventType: string;
    payload: Record<string, unknown>;
    createdAt: string;
  }>;
}

export interface ProjectViewModel {
  id: string;
  name: string;
  path: string;
  pathDisplay: string;
  checkoutLabel: string;
  revision: number;
  createdAt: string;
}

export interface EventViewModel {
  id: string;
  sequence: number;
  label: string;
  summary: string;
  occurredAt: string;
  timestamp: string;
}

export interface DesktopViewModel {
  health: {
    tone: "healthy" | "warning";
    label: string;
    detail: string;
  };
  lastUpdated: string;
  capturedAt: string;
  metrics: {
    projects: number;
    events: number;
    pendingOutbox: number;
    incidents: number;
  };
  projects: ProjectViewModel[];
  events: EventViewModel[];
  system: {
    daemonVersion: string;
    protocolVersion: number;
    schemaVersion: number;
    journalMode: string;
    bootId: string;
    uptime: string;
    dataRoot: string;
    dataRootDisplay: string;
    keeper: string;
  };
}

export interface DesktopState {
  connection: ConnectionState;
  isRefreshing: boolean;
  snapshot: DesktopSnapshot | null;
  model: DesktopViewModel | null;
  failure: DesktopFailure | null;
}

export type DesktopStateEvent =
  | { type: "loadStarted" }
  | { type: "loadSucceeded"; snapshot: DesktopSnapshot; now?: number }
  | { type: "loadFailed"; failure: DesktopFailure };

export type CommandGroupLabel =
  "Navigate" | "Create" | "Act" | "Inspect" | "Recent";

export interface CommandDefinition {
  id: string;
  label: string;
  detail: string;
  group: CommandGroupLabel;
  keywords?: string[];
  selectionKind?: Selection["kind"];
}

export interface CommandContext {
  view: ViewId;
  selection: Selection | null;
}

export interface CommandGroup {
  label: CommandGroupLabel;
  commands: CommandDefinition[];
}

function plural(value: number, singular: string, pluralForm = `${singular}s`) {
  return `${value} ${value === 1 ? singular : pluralForm}`;
}

function titleCase(value: unknown) {
  return String(value ?? "")
    .trim()
    .split(/[._\-\s]+/u)
    .filter(Boolean)
    .map((part, index) => {
      if (/^[A-Z0-9]{2,3}$/u.test(part)) return part;
      const lowered = part.toLocaleLowerCase();
      return index === 0
        ? `${lowered.charAt(0).toLocaleUpperCase()}${lowered.slice(1)}`
        : lowered;
    })
    .join(" ");
}

function truncate(value: unknown, limit = SUMMARY_LIMIT) {
  const normalized = String(value ?? "")
    .replace(/\s+/gu, " ")
    .trim();
  if (normalized.length <= limit) return normalized;
  return `${normalized.slice(0, limit - 1).trimEnd()}…`;
}

function payloadText(payload: Record<string, unknown>, key: string) {
  const value = payload[key];
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : "";
}

export function middleTruncate(value: unknown, limit: number) {
  const normalized = String(value ?? "");
  if (normalized.length <= limit) return normalized;
  if (limit <= 1) return "…".slice(0, Math.max(0, limit));
  const remaining = limit - 1;
  const startLength = Math.ceil(remaining / 2);
  const endLength = Math.floor(remaining / 2);
  return `${normalized.slice(0, startLength)}…${normalized.slice(-endLength)}`;
}

export function formatUptime(milliseconds: number) {
  const value = Number(milliseconds);
  if (!Number.isFinite(value) || value < MINUTE) return "< 1 min";

  if (value >= DAY) {
    const days = Math.floor(value / DAY);
    const hours = Math.floor((value % DAY) / HOUR);
    return hours > 0
      ? `${plural(days, "day")} ${plural(hours, "hr")}`
      : plural(days, "day");
  }

  if (value >= HOUR) {
    const hours = Math.floor(value / HOUR);
    const minutes = Math.floor((value % HOUR) / MINUTE);
    return minutes > 0
      ? `${plural(hours, "hr")} ${plural(minutes, "min")}`
      : plural(hours, "hr");
  }

  return plural(Math.floor(value / MINUTE), "min");
}

export function formatTimestamp(value: string) {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return "Time unavailable";
  return new Intl.DateTimeFormat(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(new Date(parsed));
}

export function formatRelativeTime(value: string, now = Date.now()) {
  const parsed = Date.parse(value);
  if (!Number.isFinite(parsed)) return "Time unavailable";
  const elapsed = Math.max(0, Number(now) - parsed);
  if (elapsed < MINUTE) return `${Math.floor(elapsed / 1000)} sec ago`;
  if (elapsed < HOUR) return `${Math.floor(elapsed / MINUTE)} min ago`;
  if (elapsed < DAY) return `${Math.floor(elapsed / HOUR)} hr ago`;
  return `${Math.floor(elapsed / DAY)} day ago`;
}

export function formatCheckoutMode(mode: string) {
  if (mode === "isolated") return "Isolated worktree";
  if (mode === "lead-main") return "Lead uses main";
  return titleCase(mode) || "Unknown checkout";
}

export function humanizeEventType(eventType: string) {
  return titleCase(eventType) || "Control-plane event";
}

export function summarizeEvent(event: {
  eventType?: string;
  payload?: Record<string, unknown>;
}) {
  const type = event.eventType;
  const payload = event.payload ?? {};

  if (type === "project.registered") {
    return `${truncate(payloadText(payload, "displayName") || payloadText(payload, "projectName") || "A project")} joined the zoo`;
  }
  if (type === "incident.recovered" || type === "emergency.incident.imported") {
    return `Recovered ${truncate(payloadText(payload, "code") || payloadText(payload, "incidentCode") || "an incident")}`;
  }

  const candidate =
    payloadText(payload, "summary") ||
    payloadText(payload, "message") ||
    payloadText(payload, "detail") ||
    payloadText(payload, "displayName") ||
    payloadText(payload, "code") ||
    payloadText(payload, "canonicalPath");
  return truncate(candidate || "Recorded by the control plane");
}

export function buildViewModel(
  snapshot: DesktopSnapshot,
  now = Date.now(),
): DesktopViewModel {
  const status = snapshot.status ?? ({} as DesktopSnapshot["status"]);
  const degraded = Array.isArray(status.degraded)
    ? status.degraded.filter(Boolean)
    : [];
  const incidentCount = Number(status.emergencyIncidentCount) || 0;
  let health: DesktopViewModel["health"] = {
    tone: "healthy",
    label: "Control plane steady",
    detail: "Daemon and event ledger are responding.",
  };
  if (degraded.length > 0) {
    health = {
      tone: "warning",
      label: "Attention needed",
      detail: degraded.join(" · "),
    };
  } else if (incidentCount > 0) {
    health = {
      tone: "warning",
      label: "Recovery evidence found",
      detail: `${incidentCount} recovered ${incidentCount === 1 ? "incident" : "incidents"} in the ledger.`,
    };
  }

  const projects = (
    Array.isArray(snapshot.projects) ? snapshot.projects : []
  ).map((project) => {
    const path = project.canonicalPath || "Path unavailable";
    return {
      id: project.id,
      name: project.displayName || "Unnamed project",
      path,
      pathDisplay: middleTruncate(path, 54),
      checkoutLabel: formatCheckoutMode(project.checkoutMode),
      revision: Number(project.configurationRevision) || 0,
      createdAt: formatTimestamp(project.createdAt),
    };
  });

  const events = (Array.isArray(snapshot.events) ? [...snapshot.events] : [])
    .sort((left, right) => Number(right.sequence) - Number(left.sequence))
    .map((event) => ({
      id: event.id,
      sequence: Number(event.sequence) || 0,
      label: humanizeEventType(event.eventType),
      summary: summarizeEvent(event),
      occurredAt: formatRelativeTime(event.createdAt, now),
      timestamp: formatTimestamp(event.createdAt),
    }));

  const dataRoot = status.dataRoot || "Unavailable";
  return {
    health,
    lastUpdated: formatRelativeTime(snapshot.capturedAt, now),
    capturedAt: formatTimestamp(snapshot.capturedAt),
    metrics: {
      projects: Number(status.projectCount) || projects.length,
      events: Number(status.eventHighWater) || 0,
      pendingOutbox: Number(status.pendingOutbox) || 0,
      incidents: incidentCount,
    },
    projects,
    events,
    system: {
      daemonVersion: status.daemonVersion || "Unknown",
      protocolVersion: Number(status.protocolVersion) || 0,
      schemaVersion: Number(status.schemaVersion) || 0,
      journalMode: String(status.journalMode || "unknown").toLocaleUpperCase(),
      bootId: status.bootId || "Unavailable",
      uptime: formatUptime(status.uptimeMillis),
      dataRoot,
      dataRootDisplay: middleTruncate(dataRoot, 54),
      keeper: status.keeper ? "Supervising" : "State unavailable",
    },
  };
}

export function createInitialDesktopState(): DesktopState {
  return {
    connection: "loading",
    isRefreshing: true,
    snapshot: null,
    model: null,
    failure: null,
  };
}

export function reduceDesktopState(
  state: DesktopState,
  event: DesktopStateEvent,
): DesktopState {
  if (event.type === "loadStarted") {
    return { ...state, isRefreshing: true, failure: null };
  }
  if (event.type === "loadSucceeded") {
    const model = buildViewModel(event.snapshot, event.now);
    return {
      connection: model.health.tone === "warning" ? "degraded" : "connected",
      isRefreshing: false,
      snapshot: event.snapshot,
      model,
      failure: null,
    };
  }
  return {
    ...state,
    connection: "offline",
    isRefreshing: false,
    failure: event.failure,
  };
}

export function normalizeFailure(error: unknown): DesktopFailure {
  if (error && typeof error === "object") {
    const candidate = error as Partial<DesktopFailure>;
    return {
      code: String(candidate.code || "LZ-DESKTOP-UNKNOWN"),
      message: String(
        candidate.message ||
          "The desktop could not load a control-plane snapshot.",
      ),
    };
  }
  return {
    code: "LZ-DESKTOP-UNKNOWN",
    message:
      typeof error === "string"
        ? error
        : "The desktop could not load a control-plane snapshot.",
  };
}

export function describeSelection(
  model: DesktopViewModel,
  selection: Selection,
) {
  if (selection.kind === "project") {
    const project = model.projects.find(
      (candidate) => candidate.id === selection.id,
    );
    return project
      ? {
          kindLabel: "Project",
          label: project.name,
          detail: project.path,
          metadata: project.checkoutLabel,
        }
      : null;
  }
  const event = model.events.find((candidate) => candidate.id === selection.id);
  return event
    ? {
        kindLabel: "Event",
        label: event.label,
        detail: event.summary,
        metadata: event.timestamp,
      }
    : null;
}

export function buildCommandGroups(
  commands: CommandDefinition[],
  query: string,
  context: CommandContext,
): CommandGroup[] {
  const normalizedQuery = query.trim().toLocaleLowerCase();
  const visible = commands.filter((command) => {
    if (
      command.selectionKind &&
      command.selectionKind !== context.selection?.kind
    )
      return false;
    if (!normalizedQuery) return true;
    return [command.label, command.detail, ...(command.keywords ?? [])]
      .join(" ")
      .toLocaleLowerCase()
      .includes(normalizedQuery);
  });
  const order: CommandGroupLabel[] = context.selection
    ? ["Inspect", "Act", "Navigate", "Create", "Recent"]
    : ["Act", "Navigate", "Create", "Recent", "Inspect"];
  return order
    .map((label) => ({
      label,
      commands: visible.filter((command) => command.group === label),
    }))
    .filter((group) => group.commands.length > 0);
}
