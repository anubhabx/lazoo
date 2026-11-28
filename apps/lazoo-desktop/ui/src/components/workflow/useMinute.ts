// One shared 60 s tick, so times that read "next run" or "ago" stay fresh while a page is up.

import { useSyncExternalStore } from "react";

let tick = 0;
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!timer) {
    timer = setInterval(() => {
      tick += 1;
      for (const entry of listeners) entry();
    }, 60_000);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size && timer) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** Re-renders the caller once a minute. */
export function useMinute(): number {
  return useSyncExternalStore(
    subscribe,
    () => tick,
    () => tick,
  );
}
