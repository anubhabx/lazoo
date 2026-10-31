// Native terminal panes: one xterm per open agent, bytes from the Rust poller,
// lifecycle through the daemon's session.terminal.* and terminal.* RPCs.

import { FitAddon } from "@xterm/addon-fit";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";

import type { Bridge, TerminalStreamHandle } from "./bridge";
import { asFailure } from "./bridge";
import type {
  RpcFailure,
  SessionTerminalOpenResult,
  SessionTerminalStatusResult,
  TerminalFrame,
  TerminalRecord,
} from "./protocol";
import { decodeBase64, newKey, readStorage, writeStorage } from "./util";

export type TerminalKind = "view" | "attach";

export interface TerminalEntry {
  sessionId: string;
  kind: TerminalKind | null;
  terminal: Terminal;
  fit: FitAddon;
  webgl: WebglAddon | null;
  record: TerminalRecord | null;
  status: SessionTerminalStatusResult | null;
  statusError: RpcFailure | null;
  stream: TerminalStreamHandle | null;
  offset: number;
  live: boolean;
  exitCode: number | null;
  gap: boolean;
  opening: boolean;
  inputRefused: boolean;
  error: RpcFailure | null;
  bytes: number;
  resizeTimer: ReturnType<typeof setTimeout> | null;
}

const KEYS = "lazoo.terminal-keys";

// Mirrors design-system/tokens.css (slate, soft hues); xterm cannot read CSS variables.
export const TERMINAL_THEME = {
  background: "#171c24",
  foreground: "#dfe4ea",
  cursor: "#e8ecf1",
  cursorAccent: "#171c24",
  selectionBackground: "#3a4566",
  selectionInactiveBackground: "#2d3542",
  black: "#20262f",
  red: "#ec8b91",
  green: "#6dc9a1",
  yellow: "#d6b97d",
  blue: "#7db8f2",
  magenta: "#b7a2f0",
  cyan: "#66c3c8",
  white: "#d3d9e1",
  brightBlack: "#5a6576",
  brightRed: "#f2a5aa",
  brightGreen: "#8ed8b7",
  brightYellow: "#e3cc9c",
  brightBlue: "#a1cbf6",
  brightMagenta: "#cbbcf5",
  brightCyan: "#8dd3d7",
  brightWhite: "#f3f5f8",
};

export function createTerminal(): Terminal {
  return new Terminal({
    allowProposedApi: true,
    cursorBlink: false,
    cursorStyle: "bar",
    cursorInactiveStyle: "outline",
    fontFamily: '"IBM Plex Mono", "Cascadia Mono", Consolas, monospace',
    fontSize: 13,
    lineHeight: 1.2,
    letterSpacing: 0,
    scrollback: 5000,
    theme: TERMINAL_THEME,
    windowsPty: { backend: "conpty" },
    minimumContrastRatio: 1,
    rescaleOverlappingGlyphs: true,
    customGlyphs: true,
  });
}

/** Holds xterm instances and their streams outside React so tabs can swap freely. */
export class TerminalManager {
  private readonly entries = new Map<string, TerminalEntry>();
  private readonly listeners = new Set<() => void>();
  private version = 0;
  private keys = readStorage<Record<string, string>>(KEYS, {});
  private warm: { terminal: Terminal; host: HTMLElement } | null = null;

  constructor(
    private readonly bridge: Bridge,
    private readonly factory: () => Terminal = createTerminal,
  ) {}

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getVersion = () => this.version;

  private bump() {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }

  get(sessionId: string): TerminalEntry | undefined {
    return this.entries.get(sessionId);
  }

  ensure(sessionId: string): TerminalEntry {
    const existing = this.entries.get(sessionId);
    if (existing) return existing;
    const terminal = this.factory();
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    try {
      terminal.loadAddon(new Unicode11Addon());
      terminal.unicode.activeVersion = "11";
    } catch {
      // The default Unicode tables are acceptable.
    }
    const entry: TerminalEntry = {
      sessionId,
      kind: null,
      terminal,
      fit,
      webgl: null,
      record: null,
      status: null,
      statusError: null,
      stream: null,
      offset: 0,
      live: false,
      exitCode: null,
      gap: false,
      opening: false,
      inputRefused: false,
      error: null,
      bytes: 0,
      resizeTimer: null,
    };
    terminal.onData((data) => void this.input(entry, data));
    terminal.onBinary((data) => void this.input(entry, data));
    terminal.onResize(({ cols, rows }) =>
      this.scheduleResize(entry, cols, rows),
    );
    this.entries.set(sessionId, entry);
    this.bump();
    return entry;
  }

  /** Put the xterm into a container; the instance survives re-mounting. */
  mount(sessionId: string, container: HTMLElement) {
    const entry = this.ensure(sessionId);
    const element = entry.terminal.element;
    if (element) {
      if (element.parentElement !== container) container.appendChild(element);
    } else {
      entry.terminal.open(container);
      // The DOM renderer paints at once; WebGL attaches when the open settles,
      // so creating its context never blocks the page transition.
      whenIdle(() => {
        if (this.entries.get(sessionId) === entry) this.loadWebgl(entry);
      });
    }
    this.fitSoon(entry);
  }

  /**
   * Draw xterm's WebGL glyph atlas once while the app is idle. Every terminal
   * with the same options shares it, and it lives while any owner does, so a
   * hidden terminal keeps it warm and the first conversation opens without
   * the ~200 ms first-draw stall.
   */
  prewarm() {
    if (this.warm || typeof document === "undefined") return;
    const host = document.createElement("div");
    host.setAttribute("aria-hidden", "true");
    host.style.cssText =
      "position:fixed;left:-10000px;top:0;width:240px;height:64px;pointer-events:none;";
    document.body.appendChild(host);
    const terminal = this.factory();
    try {
      terminal.open(host);
      terminal.loadAddon(new WebglAddon());
      terminal.write("lazoo");
    } catch {
      // Without WebGL there is nothing to warm.
    }
    this.warm = { terminal, host };
  }

  private loadWebgl(entry: TerminalEntry) {
    if (entry.webgl) return;
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => {
        webgl.dispose();
        entry.webgl = null;
      });
      entry.terminal.loadAddon(webgl);
      entry.webgl = webgl;
    } catch {
      entry.webgl = null;
    }
  }

  fitSoon(entry: TerminalEntry) {
    if (typeof requestAnimationFrame !== "function") return;
    requestAnimationFrame(() => {
      try {
        entry.fit.fit();
      } catch {
        // A hidden pane has no dimensions yet; the observer fits it when shown.
      }
    });
  }

  fit(sessionId: string) {
    const entry = this.entries.get(sessionId);
    if (!entry) return;
    try {
      entry.fit.fit();
    } catch {
      // Ignore zero-size containers.
    }
  }

  proposeDimensions(sessionId: string): { columns: number; rows: number } {
    const entry = this.entries.get(sessionId);
    const proposed = entry?.fit.proposeDimensions();
    const columns = proposed?.cols && proposed.cols > 10 ? proposed.cols : 120;
    const rows = proposed?.rows && proposed.rows > 3 ? proposed.rows : 36;
    return { columns: Math.min(columns, 500), rows: Math.min(rows, 200) };
  }

  async refreshStatus(
    sessionId: string,
  ): Promise<SessionTerminalStatusResult | null> {
    const entry = this.ensure(sessionId);
    try {
      const status = await this.bridge.call<SessionTerminalStatusResult>(
        "session.terminal.status",
        { id: sessionId },
      );
      entry.status = status;
      entry.statusError = null;
      const live = status.viewTerminal?.live
        ? { record: status.viewTerminal, kind: "view" as const }
        : status.terminal?.live
          ? { record: status.terminal, kind: "attach" as const }
          : null;
      if (live && (!entry.record || entry.record.id !== live.record.id)) {
        await this.adopt(entry, live.record, live.kind);
      } else if (live && entry.record && entry.record.id === live.record.id) {
        entry.record = live.record;
      }
      this.bump();
      return status;
    } catch (error) {
      entry.statusError = asFailure(error);
      this.bump();
      return null;
    }
  }

  /** Attach to a terminal the daemon already has live (reconnect). */
  private async adopt(
    entry: TerminalEntry,
    record: TerminalRecord,
    kind: TerminalKind,
  ) {
    await this.stopStream(entry);
    entry.terminal.reset();
    entry.record = record;
    entry.kind = kind;
    entry.offset = 0;
    entry.live = record.live;
    entry.exitCode = record.exitCode;
    entry.gap = false;
    entry.inputRefused = false;
    entry.error = null;
    entry.bytes = 0;
    await this.startStream(entry);
  }

  private keyFor(operation: string): string {
    const existing = this.keys[operation];
    if (existing) return existing;
    const key = newKey();
    this.keys = { ...this.keys, [operation]: key };
    writeStorage(KEYS, this.keys);
    return key;
  }

  private forgetKey(operation: string) {
    if (!(operation in this.keys)) return;
    const { [operation]: _gone, ...rest } = this.keys;
    this.keys = rest;
    writeStorage(KEYS, rest);
  }

  async open(
    sessionId: string,
    kind: TerminalKind,
    expectedRevision: number,
    retried = false,
  ): Promise<void> {
    const entry = this.ensure(sessionId);
    if (entry.opening) return;
    entry.opening = true;
    entry.error = null;
    this.bump();
    const method =
      kind === "view" ? "session.terminal.view" : "session.terminal.attach";
    const operation = `${method}:${sessionId}`;
    const { columns, rows } = this.proposeDimensions(sessionId);
    try {
      const result = await this.bridge.call<SessionTerminalOpenResult>(method, {
        id: sessionId,
        expectedRevision,
        columns,
        rows,
        idempotencyKey: this.keyFor(operation),
      });
      if (!result.terminal.live) {
        // An exact replay reconciled an old, finished open: a new view needs a new key.
        this.forgetKey(operation);
        if (!retried) {
          entry.opening = false;
          return this.open(sessionId, kind, expectedRevision, true);
        }
      }
      await this.adopt(entry, result.terminal, kind);
    } catch (error) {
      const failure = asFailure(error);
      entry.error = failure;
      if (failure.code !== "LZ-DESKTOP-DAEMON-OFFLINE")
        this.forgetKey(operation);
    } finally {
      entry.opening = false;
      this.bump();
      void this.refreshStatus(sessionId);
    }
  }

  private async startStream(entry: TerminalEntry) {
    const record = entry.record;
    if (!record) return;
    try {
      entry.stream = await this.bridge.startTerminalStream(
        { terminalId: record.id, bootId: record.bootId, after: entry.offset },
        (frame) => this.onFrame(entry, record.id, frame),
      );
    } catch (error) {
      entry.error = asFailure(error);
      console.error("terminal stream start failed", entry.error);
    }
    this.bump();
  }

  private onFrame(
    entry: TerminalEntry,
    terminalId: string,
    frame: TerminalFrame,
  ) {
    if (entry.record?.id !== terminalId) return;
    if (frame.kind === "closed") {
      entry.stream = null;
      if (frame.reason !== "exited") {
        entry.error = {
          code: "LZ-DESKTOP-TERMINAL-STREAM",
          message: frame.reason,
        };
        console.error("terminal stream closed", frame.reason);
        // The ring may already be released; the record still knows how it ended.
        void this.bridge
          .call<TerminalRecord>("terminal.show", { id: terminalId })
          .then((record) => {
            if (entry.record?.id !== terminalId) return;
            entry.record = record;
            entry.exitCode = record.exitCode;
            if (!record.live && record.exitCode !== null) entry.error = null;
            this.bump();
          })
          .catch(() => undefined);
      }
      entry.live = false;
      // A closed view is a finished operation: the next open needs a fresh key.
      this.forgetKey(
        `session.terminal.${entry.kind ?? "view"}:${entry.sessionId}`,
      );
      this.bump();
      return;
    }
    if (frame.gap && !entry.gap) {
      entry.gap = true;
      this.bump();
    }
    if (frame.data) {
      const bytes = decodeBase64(frame.data);
      entry.bytes += bytes.length;
      entry.terminal.write(bytes);
    }
    entry.offset = frame.end;
    const liveChanged =
      entry.live !== frame.live || entry.exitCode !== frame.exitCode;
    entry.live = frame.live;
    entry.exitCode = frame.exitCode;
    if (liveChanged) {
      if (!frame.live) {
        this.forgetKey(
          `session.terminal.${entry.kind ?? "view"}:${entry.sessionId}`,
        );
      }
      this.bump();
    }
  }

  private async stopStream(entry: TerminalEntry) {
    const stream = entry.stream;
    entry.stream = null;
    if (stream) {
      try {
        await stream.stop();
      } catch {
        // The task may already be gone.
      }
    }
  }

  async setFocused(sessionId: string, focused: boolean) {
    const entry = this.entries.get(sessionId);
    if (!entry?.stream) return;
    try {
      await entry.stream.focus(focused);
    } catch {
      // Best effort: the poller falls back to its unfocused cadence.
    }
  }

  private async input(entry: TerminalEntry, data: string) {
    const record = entry.record;
    if (!record || !entry.live || entry.inputRefused) return;
    try {
      await this.bridge.writeTerminal(record.id, record.bootId, data);
    } catch (error) {
      const failure = asFailure(error);
      if (failure.code === "LZ-TERMINAL-WRITE") {
        entry.inputRefused = true;
        this.bump();
      }
    }
  }

  private scheduleResize(entry: TerminalEntry, columns: number, rows: number) {
    if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
    entry.resizeTimer = setTimeout(() => {
      entry.resizeTimer = null;
      void this.resize(entry, columns, rows);
    }, 150);
  }

  private async resize(entry: TerminalEntry, columns: number, rows: number) {
    const record = entry.record;
    if (!record || !entry.live) return;
    if (record.columns === columns && record.rows === rows) return;
    try {
      const updated = await this.bridge.call<TerminalRecord>(
        "terminal.resize",
        {
          id: record.id,
          columns,
          rows,
          expectedRevision: record.revision,
          expectedBootId: record.bootId,
        },
      );
      if (updated && typeof updated === "object" && "revision" in updated) {
        entry.record = updated;
      }
    } catch {
      void this.refreshStatus(entry.sessionId);
    }
  }

  /** Close the daemon-side view or attachment; the rendered screen stays. */
  async close(sessionId: string) {
    const entry = this.entries.get(sessionId);
    const record = entry?.record;
    if (!entry || !record) return;
    await this.stopStream(entry);
    try {
      await this.bridge.call("terminal.stop", {
        id: record.id,
        expectedRevision: record.revision,
        expectedBootId: record.bootId,
      });
    } catch (error) {
      entry.error = asFailure(error);
    }
    entry.live = false;
    this.forgetKey(`session.terminal.${entry.kind ?? "view"}:${sessionId}`);
    this.bump();
    void this.refreshStatus(sessionId);
  }

  /**
   * Drop the xterm instance (tab or conversation gone) and close the daemon-side
   * view or attachment it opened, so leaving a conversation frees its terminal slot.
   */
  async release(sessionId: string) {
    const entry = this.entries.get(sessionId);
    if (!entry) return;
    await this.stopStream(entry);
    const record = entry.record;
    if (record && entry.live) {
      try {
        await this.bridge.call("terminal.stop", {
          id: record.id,
          expectedRevision: record.revision,
          expectedBootId: record.bootId,
        });
      } catch {
        // The daemon may already have closed it; status reads tell the truth later.
      }
      this.forgetKey(`session.terminal.${entry.kind ?? "view"}:${sessionId}`);
    }
    if (entry.resizeTimer) clearTimeout(entry.resizeTimer);
    entry.terminal.dispose();
    this.entries.delete(sessionId);
    this.bump();
  }

  async releaseAllExcept(keep: Set<string>) {
    for (const sessionId of [...this.entries.keys()]) {
      if (!keep.has(sessionId)) await this.release(sessionId);
    }
  }

  dispose() {
    for (const sessionId of [...this.entries.keys()])
      void this.release(sessionId);
    this.warm?.terminal.dispose();
    this.warm?.host.remove();
    this.warm = null;
  }
}

/** Run once the main thread is idle (or after 500 ms at the latest). */
export function whenIdle(task: () => void) {
  if (typeof requestIdleCallback === "function") {
    requestIdleCallback(() => task(), { timeout: 500 });
  } else {
    setTimeout(task, 0);
  }
}
