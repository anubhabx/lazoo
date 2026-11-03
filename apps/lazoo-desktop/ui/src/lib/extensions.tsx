// Small pieces the Skills and Plugins pages share. These RPCs carry no
// idempotency key (the daemon rejects unknown fields); the ledger revision
// fences every change, so the pages call the bridge directly.

import { useEffect, useRef } from "react";

import { useAppState, useStore } from "./context";
import type {
  ExtensionErrorBody,
  ExtensionPending,
  ExtensionTarget,
  RpcFailure,
} from "./protocol";
import type { ExtensionIntent } from "./store";
import { middleTruncate, providerLabel } from "./util";

/** The five harnesses, in the order every extension page lists them. */
export const EXTENSION_PROVIDERS = [
  "claude-code",
  "codex",
  "github-copilot",
  "opencode",
  "antigravity",
];

/** Re-read whenever a `skill.changed` or `plugin.changed` event has arrived. */
export function useExtensionSync(reload: () => void) {
  const { extensionTick } = useAppState();
  const seen = useRef(extensionTick);
  useEffect(() => {
    if (seen.current === extensionTick) return;
    seen.current = extensionTick;
    reload();
  }, [extensionTick, reload]);
}

/** Open a dialog the command palette asked for, once. */
export function useExtensionIntent(intent: ExtensionIntent, open: () => void) {
  const store = useStore();
  const { extensionIntent } = useAppState();
  const openRef = useRef(open);
  openRef.current = open;
  useEffect(() => {
    if (extensionIntent !== intent) return;
    store.clearExtensionIntent();
    openRef.current();
  }, [extensionIntent, intent, store]);
}

/** A failure in the composer error style: the code in mono, then the message. */
export function ErrorLine({
  error,
  quiet = false,
}: {
  error: RpcFailure | ExtensionErrorBody | null;
  /** Inside something that is already an alert. */
  quiet?: boolean;
}) {
  if (!error) return null;
  return (
    <p className="ext-error" role={quiet ? undefined : "alert"}>
      <code>{error.code}</code> {error.message}
    </p>
  );
}

export function isStale(error: { code: string }): boolean {
  return (
    error.code === "LZ-EXTENSION-STALE" || error.code === "LZ-EXTENSION-DRIFT"
  );
}

export const isExpired = (error: { code: string }) =>
  error.code === "LZ-EXTENSION-PREVIEW-EXPIRED";

export function targetKey(target: ExtensionTarget): string {
  return `${target.provider}:${target.accountId ?? "personal"}`;
}

export function sameTarget(a: ExtensionTarget, b: ExtensionTarget): boolean {
  return targetKey(a) === targetKey(b);
}

/** The wire form: a personal harness omits `accountId`. */
export function targetParam(target: ExtensionTarget): ExtensionTarget {
  return target.accountId
    ? { provider: target.provider, accountId: target.accountId }
    : { provider: target.provider };
}

export function targetLabel(
  target: ExtensionTarget,
  profiles: ReadonlyMap<string, string>,
): string {
  const harness = providerLabel(target.provider);
  if (!target.accountId) return `${harness} · Personal`;
  return `${harness} · ${profiles.get(target.accountId) ?? `Profile ${target.accountId.slice(0, 8)}`}`;
}

/** Windows' verbatim prefix is noise to a person and trips path parsing. */
export function cleanPath(path: string): string {
  return path.replace(/^\\\\\?\\/, "");
}

/** The user's home folder written as `~`. */
export function homePath(path: string): string {
  return cleanPath(path).replace(/^[A-Za-z]:\\Users\\[^\\]+/i, "~");
}

export function parentPath(path: string): string {
  const clean = cleanPath(path);
  const index = Math.max(clean.lastIndexOf("\\"), clean.lastIndexOf("/"));
  return index > 0 ? clean.slice(0, index) : clean;
}

export function displayPath(path: string, max = 56): string {
  return middleTruncate(homePath(path), max);
}

/** Every filesystem path an open or interrupted ledger entry names. */
export function pendingPaths(pending: ExtensionPending | null): string[] {
  if (!pending) return [];
  const found = new Set<string>();
  const visit = (value: unknown, key: string, depth: number) => {
    if (depth > 4 || value === null || value === undefined) return;
    if (typeof value === "string") {
      if (key === "journal" || key === "backupDirectory") found.add(value);
    } else if (Array.isArray(value)) {
      for (const item of value) visit(item, key, depth + 1);
    } else if (typeof value === "object") {
      for (const [name, child] of Object.entries(value))
        visit(child, name, depth + 1);
    }
  };
  visit(pending, "", 0);
  return [...found];
}

/** A command as one line, quoting arguments that need it. */
export function commandLine(args: string[]): string {
  return args
    .map((arg) => (/[\s"]/.test(arg) ? JSON.stringify(arg) : arg))
    .join(" ");
}
