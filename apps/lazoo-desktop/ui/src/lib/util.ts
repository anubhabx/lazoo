export function newKey(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `k-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function shortId(id: string | null | undefined): string {
  return id ? id.slice(0, 8) : "";
}

export function relativeTime(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "";
  const then = Date.parse(iso);
  if (Number.isNaN(then)) return "";
  const seconds = Math.max(0, Math.round((now - then) / 1000));
  if (seconds < 45) return "now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d`;
  const date = new Date(then);
  return date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

export function clockTime(iso: string | null | undefined): string {
  if (!iso) return "";
  const time = Date.parse(iso);
  if (Number.isNaN(time)) return "";
  return new Date(time).toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
}

/** A deadline relative to now: "in 4m", "in 2h", or "passed" once it is over. */
export function deadlineIn(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const seconds = Math.round((at - now) / 1000);
  if (seconds <= 0) return "passed";
  if (seconds < 60) return "in under a minute";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `in ${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `in ${hours}h`;
  return `in ${Math.round(hours / 24)}d`;
}

/** A future moment: clock time today, otherwise day and time. */
export function untilTime(iso: string | null | undefined, now = Date.now()) {
  if (!iso) return "";
  const at = Date.parse(iso);
  if (Number.isNaN(at)) return "";
  const date = new Date(at);
  const clock = date.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  if (at - now < 20 * 3600 * 1000) return clock;
  return `${date.toLocaleDateString(undefined, { weekday: "short" })} ${clock}`;
}

export type DayGroup = "Today" | "Previous 7 days" | "Earlier";

export function dayGroup(iso: string, now = Date.now()): DayGroup {
  const then = Date.parse(iso);
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  if (then >= start.getTime()) return "Today";
  if (now - then < 7 * 24 * 60 * 60 * 1000) return "Previous 7 days";
  return "Earlier";
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export function formatDuration(millis: number): string {
  const seconds = Math.floor(millis / 1000);
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/** Middle-truncate a path so its tail (the informative part) survives. */
export function middleTruncate(text: string, max = 48): string {
  if (text.length <= max) return text;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return `${text.slice(0, head)}…${text.slice(text.length - tail)}`;
}

export function providerLabel(provider: string | null | undefined): string {
  switch (provider) {
    case "codex":
      return "Codex";
    case "claude-code":
      return "Claude Code";
    case "opencode":
      return "OpenCode";
    case "antigravity":
      return "Antigravity";
    case "github-copilot":
      return "GitHub Copilot";
    case "grok-build":
      return "Grok Build";
    default:
      return provider ?? "";
  }
}

export function authorityLabel(mode: string | null | undefined): string {
  switch (mode) {
    case "ask":
      return "Ask";
    case "self-decision":
      return "Self-decision";
    case "full-autonomy":
      return "Full autonomy";
    default:
      return mode ?? "";
  }
}

export function words(value: string | null | undefined): string {
  return (value ?? "").replace(/-/g, " ");
}

export function readStorage<T>(key: string, fallback: T): T {
  try {
    const raw = globalThis.localStorage?.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

export function writeStorage(key: string, value: unknown) {
  try {
    globalThis.localStorage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage is a convenience, never a requirement.
  }
}

export function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}
