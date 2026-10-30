import { AnimatePresence, motion } from "motion/react";
import { ChevronDown, Minus, Search, Square, X } from "lucide-react";
import { useEffect, useRef, useState, type MouseEvent } from "react";

import { useAppState, useBridge, useStore } from "../lib/context";
import { Kbd, StatusWord, fade } from "./primitives";

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

  const connection = state.connection;
  const tone =
    connection === "connected"
      ? "healthy"
      : connection === "connecting"
        ? "running"
        : connection === "uncertain"
          ? "blocked"
          : "failed";
  const label =
    connection === "connected"
      ? "Connected"
      : connection === "connecting"
        ? "Connecting"
        : connection === "uncertain"
          ? "Reconnecting"
          : "Daemon offline";

  return (
    <header className="titlebar" onMouseDown={onDrag}>
      <div className="titlebar__brand">
        <span className="wordmark">
          <span className="wordmark__mark" aria-hidden="true" />
          lazoo
        </span>
        <SpaceSwitcher />
      </div>
      <div className="titlebar__center">
        <button
          type="button"
          className="search-trigger"
          aria-label="Open command palette"
          onClick={() => store.togglePalette(true)}
        >
          <Search size={14} aria-hidden="true" />
          <span className="search-trigger__label">Search or run a command</span>
          <Kbd keys={["Ctrl", "K"]} />
        </button>
      </div>
      <div className="titlebar__right">
        <span
          className="conn"
          title={state.connectionError?.message ?? undefined}
        >
          <StatusWord tone={tone} label={label} />
        </span>
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
      </div>
    </header>
  );
}

function SpaceSwitcher() {
  const store = useStore();
  const state = useAppState();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    function onDown(event: globalThis.MouseEvent) {
      if (!anchor.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") setOpen(false);
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const current = state.projects.find(
    (project) => project.id === state.spaceId,
  );
  const name = current?.displayName ?? "Home";

  return (
    <div className="popover-anchor" ref={anchor} data-no-drag>
      <button
        type="button"
        className="btn btn--ghost btn--sm"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={`Switch space: ${name}`}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="truncate" style={{ maxWidth: 110 }}>
          {name}
        </span>
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      <AnimatePresence>
        {open ? (
          <motion.div
            className="menu"
            role="listbox"
            aria-label="Spaces"
            {...fade}
          >
            <button
              type="button"
              role="option"
              className="row"
              aria-selected={state.spaceId === null}
              onClick={() => {
                store.selectSpace(null);
                setOpen(false);
              }}
            >
              <span className="row__label">Home</span>
              <span className="row__meta">projectless</span>
            </button>
            {state.projects.map((project) => (
              <button
                key={project.id}
                type="button"
                role="option"
                className="row"
                aria-selected={state.spaceId === project.id}
                title={project.canonicalPath}
                onClick={() => {
                  store.selectSpace(project.id);
                  setOpen(false);
                }}
              >
                <span className="row__label">{project.displayName}</span>
              </button>
            ))}
            <button
              type="button"
              className="row"
              onClick={() => {
                store.navigate({ kind: "settings", section: "projects" });
                setOpen(false);
              }}
            >
              <span className="row__label">Register a project…</span>
            </button>
          </motion.div>
        ) : null}
      </AnimatePresence>
    </div>
  );
}
