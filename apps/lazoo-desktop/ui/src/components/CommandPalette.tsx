import {
  Gauge,
  Home,
  Inbox,
  MessageSquare,
  PanelLeft,
  PanelRight,
  Plus,
  Search,
  Settings,
  Sparkles,
  SquareTerminal,
  Users,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { useAppState, useStore } from "../lib/context";
import { Kbd, ProjectGlyph } from "./primitives";

interface Command {
  id: string;
  group: "Go to" | "Conversations" | "Terminals" | "Actions";
  label: string;
  hint?: string;
  icon: ReactNode;
  keywords?: string;
  run: () => void;
}

export function CommandPalette() {
  const store = useStore();
  const state = useAppState();
  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);

  const dialog = useRef<HTMLDivElement>(null);

  useEffect(() => {
    input.current?.focus();
    function onDown(event: MouseEvent) {
      if (!dialog.current?.contains(event.target as Node)) {
        store.togglePalette(false);
      }
    }
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [store]);

  const commands = useMemo<Command[]>(() => {
    const list: Command[] = [
      {
        id: "go-home",
        group: "Go to",
        label: "Home",
        icon: <Home size={14} aria-hidden="true" />,
        run: () => {
          store.selectSpace(null);
          store.navigate({ kind: "launcher" });
        },
      },
      ...state.projects.map((project) => ({
        id: `go-project-${project.id}`,
        group: "Go to" as const,
        label: project.displayName,
        hint: "project",
        keywords: project.canonicalPath,
        icon: <ProjectGlyph name={project.displayName} size={15} />,
        run: () => store.selectSpace(project.id),
      })),
      {
        id: "go-inbox",
        group: "Go to",
        label: "Inbox",
        hint: state.inboxCounts?.unresolved
          ? `${state.inboxCounts.unresolved} need you`
          : undefined,
        icon: <Inbox size={14} aria-hidden="true" />,
        run: () => store.navigate({ kind: "inbox", filter: "all" }),
      },
      {
        id: "go-settings",
        group: "Go to",
        label: "Settings",
        icon: <Settings size={14} aria-hidden="true" />,
        run: () => store.navigate({ kind: "settings", section: "general" }),
      },
      {
        id: "go-employees",
        group: "Go to",
        label: "Employees",
        icon: <Users size={14} aria-hidden="true" />,
        run: () => store.navigate({ kind: "settings", section: "employees" }),
      },
      {
        id: "go-accounts",
        group: "Go to",
        label: "Accounts and usage",
        keywords: "routing quota limits reserve",
        icon: <Gauge size={14} aria-hidden="true" />,
        run: () => store.navigate({ kind: "settings", section: "accounts" }),
      },
      {
        id: "new-work",
        group: "Actions",
        label: "New work",
        hint: "Ctrl N",
        icon: <Plus size={14} aria-hidden="true" />,
        run: () => store.navigate({ kind: "launcher" }),
      },
      {
        id: "setup-agents",
        group: "Actions",
        label: "Set up agents",
        keywords: "team employees onboarding recommended defaults",
        icon: <Sparkles size={14} aria-hidden="true" />,
        run: () => store.openSetup(),
      },
      {
        id: "toggle-sidebar",
        group: "Actions",
        label: state.sidebarCollapsed ? "Show sidebar" : "Hide sidebar",
        hint: "Ctrl B",
        icon: <PanelLeft size={14} aria-hidden="true" />,
        run: () => store.toggleSidebar(),
      },
    ];
    for (const row of state.conversations?.conversations ?? []) {
      list.push({
        id: `conv-${row.id}`,
        group: "Conversations",
        label: row.title,
        hint: row.leadName ?? undefined,
        icon: <MessageSquare size={14} aria-hidden="true" />,
        run: () => store.openConversation(row.id),
      });
    }
    const conversation = state.conversation;
    if (conversation && state.route.kind === "conversation") {
      list.push({
        id: "focus-lead",
        group: "Terminals",
        label: `Focus ${conversation.lead.name} (main)`,
        icon: <SquareTerminal size={14} aria-hidden="true" />,
        run: () => store.focusTab("lead"),
      });
      for (const worker of conversation.workers) {
        list.push({
          id: `focus-${worker.taskId}`,
          group: "Terminals",
          label: `Focus ${worker.name ?? "worker"}`,
          hint: worker.state,
          keywords: worker.label,
          icon: <SquareTerminal size={14} aria-hidden="true" />,
          run: () => store.focusTab(worker.taskId),
        });
      }
      list.push({
        id: "toggle-details",
        group: "Actions",
        label: state.detailsOpen ? "Hide activity" : "Show activity",
        hint: "Ctrl I",
        icon: <PanelRight size={14} aria-hidden="true" />,
        run: () => store.toggleDetails(),
      });
    }
    return list;
  }, [state, store]);

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (!needle) return commands;
    return commands.filter((command) =>
      `${command.label} ${command.hint ?? ""} ${command.keywords ?? ""} ${command.group}`
        .toLowerCase()
        .includes(needle),
    );
  }, [commands, query]);

  useEffect(() => {
    setIndex(0);
  }, [query]);

  const grouped = useMemo(() => {
    const order: Command["group"][] = [
      "Go to",
      "Conversations",
      "Terminals",
      "Actions",
    ];
    return order
      .map((group) => ({
        group,
        items: filtered.filter((command) => command.group === group),
      }))
      .filter((entry) => entry.items.length);
  }, [filtered]);

  function run(command: Command) {
    store.togglePalette(false);
    command.run();
  }

  function onKey(event: React.KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setIndex((value) => Math.min(value + 1, filtered.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setIndex((value) => Math.max(value - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      const command = filtered[index];
      if (command) run(command);
    }
  }

  const activeId = filtered[index]?.id;

  return (
    <>
      {/* Opened from the keyboard many times a day: no open or close motion. */}
      <div className="scrim" aria-hidden="true" />
      <div
        ref={dialog}
        className="palette"
        role="dialog"
        aria-label="Command palette"
        onKeyDown={onKey}
      >
        <div className="palette__input">
          <Search size={15} aria-hidden="true" />
          <input
            ref={input}
            type="text"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-activedescendant={activeId ? `cmd-${activeId}` : undefined}
            aria-label="Search commands"
            placeholder="Search commands"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <Kbd keys={["Esc"]} />
        </div>
        <div className="palette__list" id="palette-list" role="listbox">
          {grouped.map(({ group, items }) => (
            <div key={group}>
              <div className="palette__group">{group}</div>
              {items.map((command) => (
                <button
                  key={command.id}
                  id={`cmd-${command.id}`}
                  type="button"
                  role="option"
                  className="palette__item"
                  aria-selected={command.id === activeId}
                  onMouseEnter={() => setIndex(filtered.indexOf(command))}
                  onClick={() => run(command)}
                >
                  {command.id === activeId ? (
                    <span className="palette__highlight" />
                  ) : null}
                  <span className="row__icon">{command.icon}</span>
                  <span className="truncate">{command.label}</span>
                  {command.hint ? (
                    <span className="meta">{command.hint}</span>
                  ) : null}
                </button>
              ))}
            </div>
          ))}
          {!filtered.length ? (
            <div className="palette__group">No matches</div>
          ) : null}
        </div>
        <div className="palette__foot">
          <span>
            <Kbd keys={["↑", "↓"]} /> select
          </span>
          <span>
            <Kbd keys={["Enter"]} /> run
          </span>
          <span>
            <Kbd keys={["Esc"]} /> close
          </span>
        </div>
      </div>
    </>
  );
}
