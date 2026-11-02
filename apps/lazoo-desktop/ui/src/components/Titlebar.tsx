import { AnimatePresence, motion } from "motion/react";
import {
  ArrowLeft,
  ArrowRight,
  ChevronRight,
  Minus,
  PanelLeft,
  Square,
  X,
} from "lucide-react";
import { useState, type MouseEvent } from "react";

import { useAppState, useBridge, useStore } from "../lib/context";
import { Button, EASE, ProjectGlyph } from "./primitives";

export function Titlebar() {
  const bridge = useBridge();
  const store = useStore();
  const state = useAppState();

  function onDrag(event: MouseEvent) {
    if (event.button !== 0) return;
    const target = event.target as HTMLElement;
    if (target.closest("button, input, [data-no-drag]")) return;
    if (event.detail === 2) {
      void bridge.window.toggleMaximize();
      return;
    }
    void bridge.window.startDragging();
  }

  return (
    <header className="titlebar" onMouseDown={onDrag}>
      <div className="titlebar__nav">
        <AnimatePresence initial={false}>
          {state.sidebarCollapsed ? (
            <motion.span
              key="expand"
              className="titlebar__expand"
              initial={{ opacity: 0, x: -6 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -6 }}
              transition={{ duration: 0.18, ease: EASE }}
            >
              <Button
                variant="ghost"
                size="sm"
                icon
                tip="Show sidebar  Ctrl B"
                aria-label="Show sidebar"
                onClick={() => store.toggleSidebar(false)}
              >
                <PanelLeft size={15} aria-hidden="true" />
              </Button>
            </motion.span>
          ) : null}
        </AnimatePresence>
        <Button
          variant="ghost"
          size="sm"
          icon
          tip="Back  Alt ←"
          aria-label="Back"
          disabled={!store.canGoBack()}
          onClick={() => store.back()}
        >
          <ArrowLeft size={15} aria-hidden="true" />
        </Button>
        <Button
          variant="ghost"
          size="sm"
          icon
          tip="Forward  Alt →"
          aria-label="Forward"
          disabled={!store.canGoForward()}
          onClick={() => store.forward()}
        >
          <ArrowRight size={15} aria-hidden="true" />
        </Button>
      </div>
      <Breadcrumb />
      <div className="window-controls" data-no-drag>
        <button
          type="button"
          aria-label="Minimize window"
          onClick={() => void bridge.window.minimize()}
        >
          <Minus size={14} aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Maximize or restore window"
          onClick={() => void bridge.window.toggleMaximize()}
        >
          <Square size={11} aria-hidden="true" />
        </button>
        <button
          type="button"
          aria-label="Close window"
          data-close
          onClick={() => void bridge.window.close()}
        >
          <X size={15} aria-hidden="true" />
        </button>
      </div>
    </header>
  );
}

/** Where you are: the space, then the page or conversation inside it. */
function Breadcrumb() {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const project = state.projects.find((entry) => entry.id === state.spaceId);
  const conversation =
    route.kind === "conversation" ? state.conversation : null;
  const conversationProject = conversation?.scope.projectName ?? null;

  if (route.kind === "inbox" || route.kind === "settings") {
    return (
      <nav className="crumbs" aria-label="Location">
        <span className="crumbs__current">
          {route.kind === "inbox" ? "Inbox" : "Settings"}
        </span>
      </nav>
    );
  }

  const spaceName =
    route.kind === "conversation"
      ? (conversationProject ??
        (conversation ? "Home" : (project?.displayName ?? "Home")))
      : (project?.displayName ?? "Home");
  const spaceId =
    route.kind === "conversation"
      ? (conversation?.scope.projectId ?? (conversation ? null : state.spaceId))
      : state.spaceId;

  return (
    <nav className="crumbs" aria-label="Location">
      <button
        type="button"
        className="crumbs__space"
        title={spaceId ? "Open project home" : "Open Home"}
        onClick={() => store.selectSpace(spaceId ?? null)}
      >
        {spaceId ? <ProjectGlyph name={spaceName} size={16} /> : null}
        <span className="truncate">{spaceName}</span>
      </button>
      {conversation ? (
        <>
          <ChevronRight size={13} className="crumbs__sep" aria-hidden="true" />
          <ConversationTitle />
        </>
      ) : null}
    </nav>
  );
}

function ConversationTitle() {
  const store = useStore();
  const { conversation } = useAppState();
  const [renaming, setRenaming] = useState(false);
  const [draft, setDraft] = useState("");
  if (!conversation) return null;
  const canRename = conversation.lead.actions.some(
    (action) => action.action === "rename",
  );

  async function commit() {
    setRenaming(false);
    const next = draft.trim();
    if (!next || next === conversation!.title) return;
    await store.rename(next);
  }

  if (renaming) {
    return (
      <input
        className="input input--sm crumbs__input"
        aria-label="Conversation title"
        value={draft}
        autoFocus
        data-no-drag
        onChange={(event) => setDraft(event.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(event) => {
          if (event.key === "Enter") void commit();
          if (event.key === "Escape") setRenaming(false);
        }}
      />
    );
  }
  return (
    <button
      type="button"
      className="crumbs__current crumbs__title"
      title={canRename ? "Rename conversation" : conversation.title}
      aria-label={`Rename conversation: ${conversation.title}`}
      disabled={!canRename}
      onClick={() => {
        setDraft(conversation.title);
        setRenaming(true);
      }}
    >
      <span className="truncate">{conversation.title}</span>
    </button>
  );
}
