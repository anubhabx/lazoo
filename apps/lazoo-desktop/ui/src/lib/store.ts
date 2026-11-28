// One store for the whole desktop: read models, their event-driven refresh,
// routing, and actions sent exactly as the daemon described them.

import type { Bridge } from "./bridge";
import { asFailure } from "./bridge";
import type { AgentDraft } from "./presets";
import { employeeParams } from "./presets";
import type {
  AuthorityMode,
  ConversationRow,
  EmployeeRecord,
  EmployeeSessionRecord,
  EventFrame,
  InboxAction,
  InboxCounts,
  InboxFilter,
  InboxListResult,
  NativeTrustRequestRecord,
  ProjectRecord,
  ProviderListEntry,
  RpcFailure,
  SystemStatus,
  Uuid,
  WorkspaceConversationResult,
  WorkspaceConversationsResult,
  WorkspaceLauncherResult,
} from "./protocol";
import { newKey, readStorage, writeStorage } from "./util";

export type SettingsSection =
  | "general"
  | "accounts"
  | "voice"
  | "skills"
  | "plugins"
  | "employees"
  | "harnesses"
  | "projects"
  | "memory"
  | "resources"
  | "daemon";

export type Route =
  | { kind: "launcher" }
  | { kind: "conversation"; id: Uuid; tab: Uuid | "lead" }
  | { kind: "inbox"; filter: InboxFilter }
  | { kind: "workflows"; id: string | null }
  | { kind: "settings"; section: SettingsSection };

export type Connection = "connecting" | "connected" | "uncertain" | "offline";

export interface Toast {
  id: string;
  tone: "info" | "error";
  title: string;
  detail?: string;
}

export interface AppState {
  connection: Connection;
  connectionError: RpcFailure | null;
  status: SystemStatus | null;
  projects: ProjectRecord[];
  spaceId: Uuid | null;
  route: Route;
  conversations: WorkspaceConversationsResult | null;
  /** Sidebar lists: Home (`"home"`) and every expanded project, newest first. */
  spaceConversations: Record<string, ConversationRow[]>;
  /** Projects open in the sidebar, remembered across launches. */
  expandedProjects: Record<Uuid, boolean>;
  sidebarCollapsed: boolean;
  launcher: WorkspaceLauncherResult | null;
  conversation: WorkspaceConversationResult | null;
  conversationError: RpcFailure | null;
  inboxCounts: InboxCounts | null;
  inbox: InboxListResult | null;
  trust: NativeTrustRequestRecord[];
  employees: EmployeeRecord[] | null;
  providers: ProviderListEntry[] | null;
  /** The quick-setup dialog that offers the default agents. */
  setupOpen: boolean;
  /** A lead the user just created; the launcher selects it. */
  preferredLeadId: Uuid | null;
  /** Every session the daemon knows; finished workers find their suspended session here. */
  sessions: EmployeeSessionRecord[];
  /** Local "seen" marks against `lastResultAt`, per conversation. */
  seen: Record<Uuid, string>;
  detailsOpen: boolean;
  paletteOpen: boolean;
  busy: Record<string, true>;
  composerError: RpcFailure | null;
  toasts: Toast[];
  /** Window-level title when the shell runs outside Tauri. */
  bridgeMissing: boolean;
  /** Bumped on every skill or plugin change event; the extension pages re-read. */
  extensionTick: number;
  /** An extension operation has started and not yet finished. */
  extensionBusy: boolean;
  /** A dialog the command palette asked a Settings page to open. */
  extensionIntent: ExtensionIntent | null;
}

export type ExtensionIntent = "add-skills" | "install-plugin";

export type Model =
  | "status"
  | "projects"
  | "conversations"
  | "launcher"
  | "conversation"
  | "inbox"
  | "inboxCounts"
  | "trust"
  | "sessions"
  | "employees"
  | "providers";

const ALL_MODELS: Model[] = [
  "status",
  "projects",
  "conversations",
  "launcher",
  "conversation",
  "inbox",
  "inboxCounts",
  "trust",
  "sessions",
  "employees",
  "providers",
];
const SEEN_KEY = "lazoo.seen";
const EXPANDED_KEY = "lazoo.expanded-projects";
const SIDEBAR_KEY = "lazoo.sidebar-collapsed";
/** `offered` once the setup dialog has been shown; it opens by itself only once. */
export const SETUP_KEY = "lazoo.setup";
const PENDING_KEY = "lazoo.pending-keys";
const DEBOUNCE_MS = 250;
const STATUS_POLL_MS = 30_000;

/** Contract index section 2: which models an event family touches. */
export function modelsForEvent(eventType: string): Model[] {
  const family = eventType.split(".")[0];
  const models = new Set<Model>();
  switch (family) {
    case "conversation":
      models.add("conversations").add("conversation");
      if (eventType === "conversation.created") models.add("launcher");
      break;
    case "turn":
      models.add("conversations").add("conversation");
      break;
    case "task":
    case "approval":
    case "incident":
      models
        .add("conversations")
        .add("conversation")
        .add("inbox")
        .add("inboxCounts");
      break;
    case "employee":
      models.add("launcher").add("conversation").add("employees");
      break;
    case "roster":
      models.add("launcher").add("conversation").add("providers");
      break;
    case "session":
      models.add("launcher").add("conversation").add("sessions");
      break;
    case "message":
      models.add("conversation");
      break;
    case "project":
      models.add("projects").add("launcher").add("conversations");
      break;
    case "native":
    case "native-trust":
      models.add("trust").add("conversation");
      break;
    default:
      break;
  }
  return [...models];
}

function initialState(): AppState {
  return {
    connection: "connecting",
    connectionError: null,
    status: null,
    projects: [],
    spaceId: null,
    route: { kind: "launcher" },
    conversations: null,
    spaceConversations: {},
    expandedProjects: readStorage<Record<Uuid, boolean>>(EXPANDED_KEY, {}),
    sidebarCollapsed: readStorage<boolean>(SIDEBAR_KEY, false),
    launcher: null,
    conversation: null,
    conversationError: null,
    inboxCounts: null,
    inbox: null,
    trust: [],
    employees: null,
    providers: null,
    setupOpen: false,
    preferredLeadId: null,
    sessions: [],
    seen: readStorage<Record<Uuid, string>>(SEEN_KEY, {}),
    detailsOpen: false,
    paletteOpen: false,
    busy: {},
    composerError: null,
    toasts: [],
    bridgeMissing: false,
    extensionTick: 0,
    extensionBusy: false,
    extensionIntent: null,
  };
}

interface StoreOptions {
  statusPollMs?: number;
  debounceMs?: number;
  now?: () => number;
}

export class Store {
  private state: AppState = initialState();
  private readonly listeners = new Set<() => void>();
  private readonly timers = new Map<Model, ReturnType<typeof setTimeout>>();
  private readonly inFlight = new Map<Model, Promise<void>>();
  private unsubscribe: (() => Promise<void>) | null = null;
  private lastSequence = 0;
  private reconnectDelay = 500;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private statusTimer: ReturnType<typeof setInterval> | null = null;
  private disposed = false;
  private everConnected = false;
  /** Lazoo's own navigation history (IA section 6.4): Alt+Left/Right traverse it. */
  private history: Route[] = [{ kind: "launcher" }];
  private historyIndex = 0;
  private pendingKeys = readStorage<Record<string, string>>(PENDING_KEY, {});

  constructor(
    private readonly bridge: Bridge,
    private readonly options: StoreOptions = {},
  ) {}

  // ---- subscription ---------------------------------------------------

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.state;

  private set(
    patch: Partial<AppState> | ((state: AppState) => Partial<AppState>),
  ) {
    const next = typeof patch === "function" ? patch(this.state) : patch;
    this.state = { ...this.state, ...next };
    for (const listener of this.listeners) listener();
  }

  // ---- lifecycle ------------------------------------------------------

  async init() {
    this.disposed = false;
    await Promise.all([
      this.refresh("status"),
      this.refresh("projects"),
      this.refresh("conversations"),
      this.refresh("launcher"),
      this.refresh("inboxCounts"),
      this.refresh("trust"),
      this.refresh("employees"),
      this.refresh("providers"),
    ]);
    this.offerSetup();
    this.startStatusPolling();
    await this.connectEvents();
  }

  dispose() {
    this.disposed = true;
    if (this.statusTimer) clearInterval(this.statusTimer);
    this.statusTimer = null;
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = null;
    for (const timer of this.timers.values()) clearTimeout(timer);
    this.timers.clear();
    const unsubscribe = this.unsubscribe;
    this.unsubscribe = null;
    void unsubscribe?.();
  }

  private startStatusPolling() {
    const interval = this.options.statusPollMs ?? STATUS_POLL_MS;
    if (interval <= 0 || this.statusTimer) return;
    this.statusTimer = setInterval(() => void this.refresh("status"), interval);
  }

  /** Each model's own `eventHighWater`; the subscription starts from the lowest. */
  private readonly marks = new Map<Model, number>();

  private lowestMark(): number {
    const marks = [...this.marks.values()];
    return marks.length ? Math.min(...marks) : 0;
  }

  private async connectEvents() {
    if (this.disposed) return;
    const after = this.lastSequence || this.lowestMark();
    try {
      this.unsubscribe = await this.bridge.subscribeEvents(after, (frame) =>
        this.onEventFrame(frame),
      );
      this.reconnectDelay = 500;
      this.set({ connection: "connected", connectionError: null });
      if (this.everConnected) {
        // A reconnect: every model may have moved while the pipe was down.
        for (const model of ALL_MODELS) this.schedule(model);
        this.set((state) => ({
          extensionTick: state.extensionTick + 1,
          extensionBusy: false,
        }));
      }
      this.everConnected = true;
    } catch (error) {
      const failure = asFailure(error);
      this.set({
        connection:
          failure.code === "LZ-DESKTOP-BRIDGE-MISSING"
            ? "offline"
            : "uncertain",
        connectionError: failure,
        bridgeMissing: failure.code === "LZ-DESKTOP-BRIDGE-MISSING",
      });
      if (failure.code !== "LZ-DESKTOP-BRIDGE-MISSING")
        this.scheduleReconnect();
    }
  }

  private scheduleReconnect() {
    if (this.disposed || this.reconnectTimer) return;
    const delay = this.reconnectDelay;
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, 8000);
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      void this.connectEvents();
    }, delay);
  }

  private onEventFrame(frame: EventFrame) {
    if (frame.kind === "closed") {
      this.unsubscribe = null;
      this.set({
        connection: "uncertain",
        connectionError: {
          code: "LZ-DESKTOP-EVENTS-CLOSED",
          message: frame.reason,
        },
      });
      this.scheduleReconnect();
      return;
    }
    const event = frame.event;
    if (event.sequence <= this.lastSequence) return;
    this.lastSequence = event.sequence;
    for (const model of modelsForEvent(event.eventType)) this.schedule(model);
    this.onExtensionEvent(event.eventType);
  }

  /**
   * Skills and plugins have no read model here: the pages re-read when the tick
   * moves. An operation in progress blocks their controls until it finishes.
   */
  private onExtensionEvent(eventType: string) {
    if (eventType === "extension.operation.started") {
      this.set({ extensionBusy: true });
    } else if (eventType === "extension.operation.finished") {
      this.set({ extensionBusy: false });
    } else if (
      eventType === "skill.changed" ||
      eventType === "plugin.changed"
    ) {
      this.set((state) => ({ extensionTick: state.extensionTick + 1 }));
    }
  }

  /** The palette's "Add skills" and "Install plugin": open the page, then its dialog. */
  openExtensionDialog(intent: ExtensionIntent) {
    this.navigate({
      kind: "settings",
      section: intent === "add-skills" ? "skills" : "plugins",
    });
    this.set({ extensionIntent: intent });
  }

  clearExtensionIntent() {
    if (this.state.extensionIntent) this.set({ extensionIntent: null });
  }

  /** Debounced re-read: at most one read per model per 250 ms. */
  schedule(model: Model) {
    if (this.timers.has(model)) return;
    const timer = setTimeout(() => {
      this.timers.delete(model);
      void this.refresh(model);
    }, this.options.debounceMs ?? DEBOUNCE_MS);
    this.timers.set(model, timer);
  }

  // ---- reads ----------------------------------------------------------

  refresh(model: Model): Promise<void> {
    const running = this.inFlight.get(model);
    if (running) return running;
    const task = this.read(model).finally(() => this.inFlight.delete(model));
    this.inFlight.set(model, task);
    return task;
  }

  private async read(model: Model) {
    try {
      switch (model) {
        case "status": {
          const status = await this.bridge.call<SystemStatus>("system.status");
          this.marks.set("status", status.eventHighWater);
          this.set({ status });
          break;
        }
        case "projects": {
          const projects =
            await this.bridge.call<ProjectRecord[]>("project.list");
          this.set({ projects });
          break;
        }
        case "conversations": {
          const current = this.state.spaceId;
          const known = new Set(this.state.projects.map((p) => p.id));
          const others = [
            null,
            ...Object.keys(this.state.expandedProjects).filter(
              (id) => this.state.expandedProjects[id] && known.has(id),
            ),
          ].filter((id) => id !== current);
          // The current space is read last so its reply is the freshest.
          const results = await Promise.all(
            [...others, current].map((projectId) =>
              this.bridge
                .call<WorkspaceConversationsResult>("workspace.conversations", {
                  projectId,
                  limit: projectId === current ? 200 : 50,
                })
                .catch((error: unknown) => {
                  if (projectId === current) throw error;
                  return null;
                }),
            ),
          );
          const lists = { ...this.state.spaceConversations };
          for (const result of results) {
            if (result)
              lists[result.projectId ?? "home"] = result.conversations;
          }
          this.set({ spaceConversations: lists });
          const conversations = results.at(-1);
          if (conversations && conversations.projectId === this.state.spaceId) {
            this.marks.set("conversations", conversations.eventHighWater);
            this.set({ conversations });
            // The open conversation is being watched: a result that lands now is seen.
            const route = this.state.route;
            if (route.kind === "conversation") this.markSeen(route.id);
          }
          break;
        }
        case "launcher": {
          const launcher = await this.bridge.call<WorkspaceLauncherResult>(
            "workspace.launcher",
            { projectId: this.state.spaceId },
          );
          if ((launcher.project?.id ?? null) === this.state.spaceId) {
            this.marks.set("launcher", launcher.eventHighWater);
            this.set({ launcher });
          }
          break;
        }
        case "conversation": {
          const route = this.state.route;
          if (route.kind !== "conversation") return;
          const conversation =
            await this.bridge.call<WorkspaceConversationResult>(
              "workspace.conversation",
              { id: route.id, turnLimit: 50 },
            );
          if (
            this.state.route.kind === "conversation" &&
            this.state.route.id === route.id
          ) {
            this.set({ conversation, conversationError: null });
          }
          break;
        }
        case "inboxCounts": {
          const result = await this.bridge.call<{
            counts: InboxCounts;
            eventHighWater: number;
          }>("inbox.counts");
          this.marks.set("inboxCounts", result.eventHighWater);
          this.set({ inboxCounts: result.counts });
          break;
        }
        case "inbox": {
          const route = this.state.route;
          const filter: InboxFilter =
            route.kind === "inbox" ? route.filter : "all";
          const inbox = await this.bridge.call<InboxListResult>("inbox.list", {
            filter,
            limit: 200,
          });
          this.set({ inbox, inboxCounts: inbox.counts });
          break;
        }
        case "sessions": {
          const sessions =
            await this.bridge.call<EmployeeSessionRecord[]>("session.list");
          this.set({ sessions: Array.isArray(sessions) ? sessions : [] });
          break;
        }
        case "employees": {
          const employees =
            await this.bridge.call<EmployeeRecord[]>("employee.list");
          this.set({ employees: Array.isArray(employees) ? employees : [] });
          break;
        }
        case "providers": {
          const providers =
            await this.bridge.call<ProviderListEntry[]>("provider.list");
          this.set({ providers: Array.isArray(providers) ? providers : [] });
          break;
        }
        case "trust": {
          const trust = await this.bridge.call<NativeTrustRequestRecord[]>(
            "native.trust.list",
            { state: "pending", limit: 100 },
          );
          this.set({ trust: Array.isArray(trust) ? trust : [] });
          break;
        }
        default:
          break;
      }
    } catch (error) {
      const failure = asFailure(error);
      if (model === "conversation") {
        this.set({ conversationError: failure });
      }
      if (failure.code === "LZ-DESKTOP-BRIDGE-MISSING") {
        this.set({
          connection: "offline",
          connectionError: failure,
          bridgeMissing: true,
        });
      } else if (failure.code === "LZ-DESKTOP-DAEMON-OFFLINE") {
        this.set({ connection: "uncertain", connectionError: failure });
      }
    }
  }

  // ---- navigation -----------------------------------------------------

  navigate(route: Route, record = true) {
    const previous = this.state.route;
    if (record) {
      this.history = [
        ...this.history.slice(0, this.historyIndex + 1),
        route,
      ].slice(-50);
      this.historyIndex = this.history.length - 1;
    }
    this.set({ route, paletteOpen: false, composerError: null });
    if (route.kind === "conversation") {
      if (previous.kind !== "conversation" || previous.id !== route.id) {
        this.set({ conversation: null, conversationError: null });
      }
      void this.refresh("conversation");
      void this.refresh("sessions");
      this.markSeen(route.id);
    } else if (route.kind === "inbox") {
      void this.refresh("inbox");
    } else if (route.kind === "launcher") {
      void this.refresh("launcher");
    }
  }

  canGoBack() {
    return this.historyIndex > 0;
  }

  canGoForward() {
    return this.historyIndex < this.history.length - 1;
  }

  back() {
    if (!this.canGoBack()) return;
    this.historyIndex -= 1;
    this.navigate(this.history[this.historyIndex]!, false);
  }

  forward() {
    if (!this.canGoForward()) return;
    this.historyIndex += 1;
    this.navigate(this.history[this.historyIndex]!, false);
  }

  openConversation(id: Uuid, tab: Uuid | "lead" = "lead") {
    this.navigate({ kind: "conversation", id, tab });
  }

  focusTab(tab: Uuid | "lead") {
    const route = this.state.route;
    if (route.kind !== "conversation") return;
    this.set({ route: { ...route, tab } });
  }

  selectSpace(spaceId: Uuid | null) {
    if (spaceId === this.state.spaceId) {
      this.navigate({ kind: "launcher" });
      return;
    }
    this.set({
      spaceId,
      conversations: null,
      launcher: null,
      conversation: null,
    });
    // After the switch, so the read it starts is for the new space.
    if (spaceId) this.setProjectExpanded(spaceId, true);
    this.navigate({ kind: "launcher" });
    void this.refresh("conversations");
    void this.refresh("launcher");
  }

  setProjectExpanded(projectId: Uuid, expanded: boolean) {
    if (Boolean(this.state.expandedProjects[projectId]) === expanded) return;
    const expandedProjects = { ...this.state.expandedProjects };
    if (expanded) expandedProjects[projectId] = true;
    else delete expandedProjects[projectId];
    writeStorage(EXPANDED_KEY, expandedProjects);
    this.set({ expandedProjects });
    if (expanded) void this.refresh("conversations");
  }

  toggleSidebar(collapsed?: boolean) {
    const next = collapsed ?? !this.state.sidebarCollapsed;
    writeStorage(SIDEBAR_KEY, next);
    this.set({ sidebarCollapsed: next });
  }

  /** The setup dialog opens by itself once, when a harness is detected. */
  private offerSetup() {
    if (readStorage<string | null>(SETUP_KEY, null)) return;
    if (!this.state.providers?.some((entry) => entry.detected)) return;
    writeStorage(SETUP_KEY, "offered");
    this.set({ setupOpen: true });
  }

  openSetup(open = true) {
    if (open) void this.refresh("providers");
    this.set({ setupOpen: open, paletteOpen: false });
  }

  /**
   * Create the drafted agents one at a time, reporting each outcome. Names are
   * the idempotency scope, so a retried setup never creates a twin.
   */
  async createTeam(
    drafts: AgentDraft[],
    onProgress: (
      key: string,
      outcome: "creating" | "created" | "failed",
    ) => void,
  ): Promise<number> {
    let created = 0;
    for (const draft of drafts) {
      onProgress(draft.key, "creating");
      const params = employeeParams(draft);
      const result = await this.mutate(
        "employee.create",
        params,
        `employee.create:${params.name.toLowerCase()}:${params.provider}`,
      );
      if (result) {
        created += 1;
        onProgress(draft.key, "created");
        const id =
          (result as { employee?: { id?: Uuid }; id?: Uuid }).employee?.id ??
          (result as { id?: Uuid }).id;
        if (draft.key === "lead" && id) this.set({ preferredLeadId: id });
      } else {
        onProgress(draft.key, "failed");
      }
    }
    void this.refresh("employees");
    void this.refresh("launcher");
    return created;
  }

  togglePalette(open?: boolean) {
    this.set((state) => ({ paletteOpen: open ?? !state.paletteOpen }));
  }

  toggleDetails(open?: boolean) {
    this.set((state) => ({ detailsOpen: open ?? !state.detailsOpen }));
  }

  markSeen(conversationId: Uuid) {
    const row = this.state.conversations?.conversations.find(
      (c) => c.id === conversationId,
    );
    const mark = row?.lastResultAt ?? row?.updatedAt;
    if (!mark || this.state.seen[conversationId] === mark) return;
    const seen = { ...this.state.seen, [conversationId]: mark };
    writeStorage(SEEN_KEY, seen);
    this.set({ seen });
  }

  // ---- toasts ---------------------------------------------------------

  toast(toast: Omit<Toast, "id">, ttlMs = 6000) {
    const id = newKey();
    this.set((state) => ({ toasts: [...state.toasts, { ...toast, id }] }));
    if (ttlMs > 0) setTimeout(() => this.dismissToast(id), ttlMs);
    return id;
  }

  dismissToast(id: string) {
    this.set((state) => ({
      toasts: state.toasts.filter((toast) => toast.id !== id),
    }));
  }

  // ---- idempotency ----------------------------------------------------

  /** One key per logical operation, kept until its outcome is known. */
  private keyFor(operation: string): string {
    const existing = this.pendingKeys[operation];
    if (existing) return existing;
    const key = newKey();
    this.pendingKeys = { ...this.pendingKeys, [operation]: key };
    writeStorage(PENDING_KEY, this.pendingKeys);
    return key;
  }

  private settle(operation: string) {
    if (!(operation in this.pendingKeys)) return;
    const { [operation]: _settled, ...rest } = this.pendingKeys;
    this.pendingKeys = rest;
    writeStorage(PENDING_KEY, rest);
  }

  private setBusy(id: string, busy: boolean) {
    this.set((state) => {
      const next = { ...state.busy };
      if (busy) next[id] = true;
      else delete next[id];
      return { busy: next };
    });
  }

  isBusy(id: string) {
    return Boolean(this.state.busy[id]);
  }

  // ---- actions --------------------------------------------------------

  /**
   * Send an action exactly as the daemon described it. Only the idempotency
   * key (and the rename's title) are added; naming an RPC grants nothing.
   */
  async act(
    action: InboxAction,
    extra: Record<string, unknown> = {},
  ): Promise<boolean> {
    const target = String(
      action.params.id ?? action.params.conversationId ?? "",
    );
    const revision = String(action.params.expectedRevision ?? "");
    const operation = `${action.method}:${action.action}:${target}:${revision}`;
    const busyId = `${action.action}:${target}`;
    if (this.isBusy(busyId)) return false;
    this.setBusy(busyId, true);
    try {
      await this.bridge.call(action.method, {
        ...action.params,
        ...extra,
        idempotencyKey: this.keyFor(operation),
      });
      this.settle(operation);
      return true;
    } catch (error) {
      const failure = asFailure(error);
      if (!isTransient(failure)) this.settle(operation);
      this.toast({
        tone: "error",
        title: failure.code,
        detail: failure.message,
      });
      return false;
    } finally {
      this.setBusy(busyId, false);
      this.schedule("conversation");
      this.schedule("conversations");
      this.schedule("inbox");
      this.schedule("inboxCounts");
    }
  }

  /** Launcher: `conversation.create`, then `turn.submit { activateIfNeeded }`. */
  async start(input: { leadId: Uuid; authority: AuthorityMode; text: string }) {
    const launcher = this.state.launcher;
    const text = input.text.trim();
    if (!launcher || !text || this.isBusy("start")) return;
    const [create, submit] = launcher.start;
    if (!create || !submit) return;
    this.setBusy("start", true);
    this.set({ composerError: null });
    const operation = `start:${this.state.spaceId ?? "home"}:${input.leadId}:${hashText(text)}`;
    try {
      const created = await this.bridge.call<{ conversation: { id: Uuid } }>(
        create.method,
        {
          ...create.params,
          leadEmployeeId: input.leadId,
          idempotencyKey: this.keyFor(`${operation}:create`),
        },
      );
      const conversationId = created.conversation.id;
      this.openConversation(conversationId);
      try {
        await this.bridge.call(submit.method, {
          ...submit.params,
          conversationId,
          authorityMode: input.authority,
          text,
          idempotencyKey: this.keyFor(`${operation}:submit`),
        });
        this.settle(`${operation}:create`);
        this.settle(`${operation}:submit`);
      } catch (error) {
        const failure = asFailure(error);
        if (!isTransient(failure)) {
          this.settle(`${operation}:create`);
          this.settle(`${operation}:submit`);
        }
        this.set({ composerError: failure });
      }
    } catch (error) {
      const failure = asFailure(error);
      if (!isTransient(failure)) this.settle(`${operation}:create`);
      this.set({ composerError: failure });
    } finally {
      this.setBusy("start", false);
      this.schedule("conversations");
      this.schedule("conversation");
      this.schedule("launcher");
    }
  }

  /** Composer: one more user turn to the open conversation's lead. */
  async submitTurn(text: string, authority: AuthorityMode) {
    const conversation = this.state.conversation;
    const trimmed = text.trim();
    if (!conversation || !trimmed || this.isBusy("turn")) return false;
    this.setBusy("turn", true);
    this.set({ composerError: null });
    const operation = `turn:${conversation.conversation.id}:${hashText(trimmed)}:${conversation.turns.length}`;
    try {
      const result = await this.bridge.call<{
        message?: { delivery?: { state?: string } };
      }>("turn.submit", {
        conversationId: conversation.conversation.id,
        author: { kind: "user" },
        recipient: null,
        taskId: null,
        authorityMode: authority,
        text: trimmed,
        activateIfNeeded: true,
        maxAttempts: 3,
        idempotencyKey: this.keyFor(operation),
      });
      this.settle(operation);
      // A busy recipient holds the turn and takes it when it is idle.
      if (result?.message?.delivery?.state === "queued")
        this.toast({
          tone: "info",
          title: `Queued for ${conversation.lead.name}`,
          detail: "It is delivered when the current turn finishes.",
        });
      return true;
    } catch (error) {
      const failure = asFailure(error);
      if (!isTransient(failure)) this.settle(operation);
      this.set({ composerError: failure });
      return false;
    } finally {
      this.setBusy("turn", false);
      this.schedule("conversation");
      this.schedule("conversations");
    }
  }

  async rename(title: string | null) {
    const action = this.state.conversation?.lead.actions.find(
      (a) => a.action === "rename",
    );
    if (!action) return false;
    return this.act(action, { title });
  }

  async resolveTrust(
    request: NativeTrustRequestRecord,
    decision: "approve" | "decline",
    autoAcceptManagedNextTime: boolean,
  ) {
    const operation = `trust:${request.id}:${request.revision}:${decision}`;
    const busyId = `trust:${request.id}`;
    if (this.isBusy(busyId)) return;
    this.setBusy(busyId, true);
    try {
      await this.bridge.call("native.trust.resolve", {
        id: request.id,
        expectedRevision: request.revision,
        decision,
        autoAcceptManagedNextTime,
        idempotencyKey: this.keyFor(operation),
      });
      this.settle(operation);
    } catch (error) {
      const failure = asFailure(error);
      if (!isTransient(failure)) this.settle(operation);
      this.toast({
        tone: "error",
        title: failure.code,
        detail: failure.message,
      });
    } finally {
      this.setBusy(busyId, false);
      this.schedule("trust");
      this.schedule("conversation");
    }
  }

  /** Generic mutation for Settings forms; the key is per logical operation. */
  async mutate(
    method: string,
    params: Record<string, unknown>,
    operation: string,
  ) {
    if (this.isBusy(operation)) return null;
    this.setBusy(operation, true);
    try {
      const result = await this.bridge.call<unknown>(method, {
        ...params,
        idempotencyKey: this.keyFor(operation),
      });
      this.settle(operation);
      return result;
    } catch (error) {
      const failure = asFailure(error);
      if (!isTransient(failure)) this.settle(operation);
      this.toast({
        tone: "error",
        title: failure.code,
        detail: failure.message,
      });
      return null;
    } finally {
      this.setBusy(operation, false);
      this.schedule("projects");
      this.schedule("launcher");
    }
  }
}

/** A lost reply keeps its key; a refusal does not. */
export function isTransient(failure: RpcFailure): boolean {
  return (
    failure.code === "LZ-DESKTOP-DAEMON-OFFLINE" ||
    failure.code === "LZ-DESKTOP-IPC" ||
    failure.code === "LZ-DESKTOP-INVALID-RESPONSE" ||
    failure.code === "LZ-IPC"
  );
}

function hashText(text: string): string {
  let hash = 0;
  for (let index = 0; index < text.length; index += 1) {
    hash = (hash * 31 + text.charCodeAt(index)) | 0;
  }
  return (hash >>> 0).toString(36);
}
