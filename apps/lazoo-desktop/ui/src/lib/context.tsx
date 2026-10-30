import { createContext, useContext, useSyncExternalStore } from "react";

import type { Bridge } from "./bridge";
import type { AppState, Store } from "./store";
import type { TerminalManager } from "./terminals";

const StoreContext = createContext<Store | null>(null);
const BridgeContext = createContext<Bridge | null>(null);
const TerminalsContext = createContext<TerminalManager | null>(null);

export const StoreProvider = StoreContext.Provider;
export const BridgeProvider = BridgeContext.Provider;
export const TerminalsProvider = TerminalsContext.Provider;

export function useStore(): Store {
  const store = useContext(StoreContext);
  if (!store) throw new Error("StoreProvider is missing");
  return store;
}

export function useBridge(): Bridge {
  const bridge = useContext(BridgeContext);
  if (!bridge) throw new Error("BridgeProvider is missing");
  return bridge;
}

export function useTerminals(): TerminalManager {
  const terminals = useContext(TerminalsContext);
  if (!terminals) throw new Error("TerminalsProvider is missing");
  return terminals;
}

export function useAppState(): AppState {
  const store = useStore();
  return useSyncExternalStore(
    store.subscribe,
    store.getSnapshot,
    store.getSnapshot,
  );
}
