import { AnimatePresence, MotionConfig, motion } from "motion/react";
import { useEffect, useMemo } from "react";

import { CommandPalette } from "./components/CommandPalette";
import { DetailsPanel } from "./components/DetailsPanel";
import { Rail } from "./components/Rail";
import { SetupDialog } from "./components/SetupDialog";
import { Titlebar } from "./components/Titlebar";
import { Toasts } from "./components/Toasts";
import { EASE } from "./components/primitives";
import type { Bridge } from "./lib/bridge";
import {
  BridgeProvider,
  StoreProvider,
  TerminalsProvider,
  useAppState,
  useStore,
} from "./lib/context";
import type { Store } from "./lib/store";
import { TerminalManager } from "./lib/terminals";
import { ConversationPage } from "./pages/Conversation";
import { InboxPage } from "./pages/Inbox";
import { LauncherPage } from "./pages/Launcher";
import { SettingsPage } from "./pages/Settings";

export function App({ store, bridge }: { store: Store; bridge: Bridge }) {
  const terminals = useMemo(() => new TerminalManager(bridge), [bridge]);

  useEffect(() => {
    void store.init();
    return () => {
      store.dispose();
      terminals.dispose();
    };
  }, [store, terminals]);

  return (
    <StoreProvider value={store}>
      <BridgeProvider value={bridge}>
        <TerminalsProvider value={terminals}>
          <MotionConfig
            reducedMotion="user"
            transition={{ duration: 0.22, ease: EASE }}
          >
            <Shell />
          </MotionConfig>
        </TerminalsProvider>
      </BridgeProvider>
    </StoreProvider>
  );
}

function Shell() {
  const store = useStore();
  const state = useAppState();
  const route = state.route;

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const key = event.key.toLowerCase();
      if (event.ctrlKey && !event.altKey && key === "k") {
        event.preventDefault();
        store.togglePalette();
        return;
      }
      if (event.ctrlKey && !event.altKey && key === "i") {
        if (store.getSnapshot().route.kind === "conversation") {
          event.preventDefault();
          store.toggleDetails();
        }
        return;
      }
      if (event.ctrlKey && !event.altKey && key === "n") {
        event.preventDefault();
        store.navigate({ kind: "launcher" });
        return;
      }
      if (event.ctrlKey && !event.altKey && key === "b") {
        const target = event.target as HTMLElement | null;
        if (target?.closest?.(".xterm")) return;
        event.preventDefault();
        store.toggleSidebar();
        return;
      }
      if (
        event.altKey &&
        (event.key === "ArrowLeft" || event.key === "ArrowRight")
      ) {
        // Lazoo's history, never a terminal's: a focused native pane keeps its keys.
        const target = event.target as HTMLElement | null;
        if (target?.closest?.(".xterm")) return;
        event.preventDefault();
        if (event.key === "ArrowLeft") store.back();
        else store.forward();
        return;
      }
      if (event.key === "Escape") {
        const snapshot = store.getSnapshot();
        if (snapshot.setupOpen) return;
        if (snapshot.paletteOpen) store.togglePalette(false);
        else if (snapshot.detailsOpen) store.toggleDetails(false);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [store]);

  const pageKey =
    route.kind === "conversation"
      ? `conversation:${route.id}`
      : route.kind === "settings"
        ? "settings"
        : route.kind === "launcher"
          ? `launcher:${state.spaceId ?? "home"}`
          : route.kind;

  return (
    <div
      className="shell"
      data-sidebar={state.sidebarCollapsed ? "collapsed" : "open"}
    >
      <Rail />
      <div className="main">
        <Titlebar />
        <main className="stage" aria-label="Workspace">
          <AnimatePresence initial={false}>
            <motion.div
              key={pageKey}
              className="page-layer"
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -3 }}
              transition={{ duration: 0.2, ease: EASE }}
            >
              {route.kind === "launcher" ? <LauncherPage /> : null}
              {route.kind === "conversation" ? <ConversationPage /> : null}
              {route.kind === "inbox" ? <InboxPage /> : null}
              {route.kind === "settings" ? <SettingsPage /> : null}
            </motion.div>
          </AnimatePresence>
        </main>
        <AnimatePresence initial={false}>
          {state.detailsOpen && route.kind === "conversation" ? (
            <DetailsPanel key="details" />
          ) : null}
        </AnimatePresence>
      </div>
      <Toasts />
      <AnimatePresence>
        {state.paletteOpen ? <CommandPalette key="palette" /> : null}
      </AnimatePresence>
      <SetupDialog />
    </div>
  );
}
