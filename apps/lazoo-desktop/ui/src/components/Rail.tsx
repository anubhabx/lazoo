import {
  ChevronRight,
  FolderPlus,
  Inbox,
  PanelLeft,
  Plus,
  Search,
  Settings,
} from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";

import { useAppState, useStore } from "../lib/context";
import type { ConversationRow, ProjectRecord } from "../lib/protocol";
import { relativeTime } from "../lib/util";
import {
  Avatar,
  Button,
  Dialog,
  Dot,
  Field,
  Kbd,
  ProjectGlyph,
} from "./primitives";
import { LogoMark } from "./Logo";

/** How many conversations an open project shows before "Show all". */
const PROJECT_PREVIEW = 5;

export function markerTone(
  row: ConversationRow,
  seen: Record<string, string>,
): { tone: string; label: string } | null {
  switch (row.marker) {
    case "needs-attention":
      return { tone: "attention", label: "Needs you" };
    case "active":
      return { tone: "running", label: "Active" };
    case "failed":
      return { tone: "failed", label: "Failed" };
    case "completed": {
      const mark = row.lastResultAt ?? row.updatedAt;
      return seen[row.id] === mark
        ? null
        : { tone: "complete", label: "Completed, unread" };
    }
    default:
      return null;
  }
}

/** What the lead's avatar shows for a conversation row. */
function rowStatus(row: ConversationRow): string {
  switch (row.marker) {
    case "active":
      return "running";
    case "needs-attention":
      return "attention";
    case "failed":
      return "failed";
    default:
      return "idle";
  }
}

export function Rail() {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const [adding, setAdding] = useState(false);
  const unresolved = state.inboxCounts?.unresolved ?? 0;
  const home = state.spaceConversations.home ?? [];
  const homeLoaded = "home" in state.spaceConversations;
  const onLauncher = route.kind === "launcher";
  // The mark's cursor blinks while any conversation the sidebar knows is active.
  const working = Object.values(state.spaceConversations).some((rows) =>
    rows.some((row) => row.marker === "active"),
  );

  return (
    <aside className="rail" aria-label="Spaces and conversations">
      <div className="rail__inner">
        <div className="rail__head">
          <span className="wordmark">
            <span className="wordmark__glyph">
              <LogoMark size={16} live={working} />
            </span>
            lazoo
          </span>
          <Button
            variant="ghost"
            size="sm"
            icon
            tip="Hide sidebar  Ctrl B"
            tipSide="bottom"
            aria-label="Hide sidebar"
            onClick={() => store.toggleSidebar(true)}
          >
            <PanelLeft size={15} aria-hidden="true" />
          </Button>
        </div>

        <nav className="rail__actions" aria-label="Primary">
          <RailButton
            icon={<Plus size={15} aria-hidden="true" />}
            label="New work"
            current={onLauncher && state.spaceId === null}
            trailing={<Kbd keys={["Ctrl", "N"]} />}
            onClick={() => store.selectSpace(null)}
          />
          <RailButton
            icon={<Search size={15} aria-hidden="true" />}
            label="Search"
            ariaLabel="Open command palette"
            trailing={<Kbd keys={["Ctrl", "K"]} />}
            onClick={() => store.togglePalette(true)}
          />
          <RailButton
            icon={<Inbox size={15} aria-hidden="true" />}
            label="Inbox"
            current={route.kind === "inbox"}
            onClick={() => store.navigate({ kind: "inbox", filter: "all" })}
            trailing={
              unresolved > 0 ? (
                <span
                  className="rail-badge"
                  aria-label={`${unresolved} items need you`}
                >
                  {unresolved}
                </span>
              ) : null
            }
          />
        </nav>

        <div className="rail__scroll">
          <section className="rail-section" aria-label="Projects">
            <div className="rail-section__head">
              <span>Projects</span>
              <Button
                variant="ghost"
                size="sm"
                icon
                tip="Add a project"
                tipSide="left"
                aria-label="Add a project"
                onClick={() => setAdding(true)}
              >
                <Plus size={14} aria-hidden="true" />
              </Button>
            </div>
            {state.projects.map((project) => (
              <ProjectEntry key={project.id} project={project} />
            ))}
            {state.projects.length === 0 ? (
              <button
                type="button"
                className="rail-row rail-row--hint"
                onClick={() => setAdding(true)}
              >
                <span className="rail-row__icon">
                  <FolderPlus size={15} aria-hidden="true" />
                </span>
                <span className="rail-row__label">Add your first project</span>
              </button>
            ) : null}
          </section>

          <section className="rail-section" aria-label="Recents">
            <div className="rail-section__head">
              <span>Recents</span>
            </div>
            <div role="list" aria-label="Conversation history">
              {home.map((row) => (
                <ConversationEntry key={row.id} row={row} />
              ))}
            </div>
            {homeLoaded && home.length === 0 ? (
              <p className="rail-section__empty">No conversations yet</p>
            ) : null}
          </section>
        </div>

        <div className="rail__foot">
          <RailButton
            icon={<Settings size={15} aria-hidden="true" />}
            label="Settings"
            current={route.kind === "settings"}
            onClick={() =>
              store.navigate({ kind: "settings", section: "general" })
            }
          />
          <Connection />
        </div>
      </div>
      <AddProjectDialog open={adding} onClose={() => setAdding(false)} />
    </aside>
  );
}

function RailButton({
  icon,
  label,
  ariaLabel,
  current = false,
  trailing,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  ariaLabel?: string;
  current?: boolean;
  trailing?: ReactNode;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className="rail-row"
      aria-current={current ? "page" : undefined}
      aria-label={ariaLabel}
      onClick={onClick}
    >
      <span className="rail-row__icon">{icon}</span>
      <span className="rail-row__label">{label}</span>
      {trailing ? <span className="rail-row__trail">{trailing}</span> : null}
    </button>
  );
}

function Connection() {
  const state = useAppState();
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
    <span
      className="rail-conn"
      title={state.connectionError?.message ?? label}
      data-state={connection}
    >
      <Dot tone={tone} />
      <span>{label}</span>
    </span>
  );
}

function ProjectEntry({ project }: { project: ProjectRecord }) {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const expanded = Boolean(state.expandedProjects[project.id]);
  const rows = state.spaceConversations[project.id] ?? [];
  const needsYou = rows.filter(
    (row) => row.marker === "needs-attention",
  ).length;
  const current = route.kind === "launcher" && state.spaceId === project.id;
  const preview = rows.slice(0, PROJECT_PREVIEW);

  return (
    <div className="rail-project" data-expanded={expanded}>
      <div className="rail-project__row">
        <button
          type="button"
          className="rail-row"
          aria-current={current ? "page" : undefined}
          title={project.canonicalPath}
          onClick={() => store.selectSpace(project.id)}
        >
          <span className="rail-row__icon">
            <ProjectGlyph name={project.displayName} size={18} />
          </span>
          <span className="rail-row__label">{project.displayName}</span>
          {needsYou > 0 ? (
            <span className="rail-row__trail">
              <span className="rail-badge" aria-label={`${needsYou} need you`}>
                {needsYou}
              </span>
            </span>
          ) : null}
        </button>
        <button
          type="button"
          className="rail-project__toggle"
          aria-expanded={expanded}
          aria-label={`${expanded ? "Collapse" : "Expand"} ${project.displayName}`}
          onClick={() => store.setProjectExpanded(project.id, !expanded)}
        >
          <ChevronRight size={13} aria-hidden="true" />
        </button>
      </div>
      {expanded ? (
        <div
          className="rail-project__children"
          role="list"
          aria-label={`${project.displayName} conversations`}
        >
          {preview.map((row) => (
            <ConversationEntry key={row.id} row={row} nested />
          ))}
          {rows.length === 0 && project.id in state.spaceConversations ? (
            <button
              type="button"
              className="rail-row rail-row--nested rail-row--hint"
              onClick={() => store.selectSpace(project.id)}
            >
              <span className="rail-row__label">
                Start the first conversation
              </span>
            </button>
          ) : null}
          {rows.length > PROJECT_PREVIEW ? (
            <button
              type="button"
              className="rail-row rail-row--nested rail-row--hint"
              onClick={() => store.selectSpace(project.id)}
            >
              <span className="rail-row__label">Show all {rows.length}</span>
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function ConversationEntry({
  row,
  nested = false,
}: {
  row: ConversationRow;
  nested?: boolean;
}) {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const marker = markerTone(row, state.seen);
  const selected = route.kind === "conversation" && route.id === row.id;
  return (
    <div role="listitem">
      <button
        type="button"
        className={`rail-row rail-row--conv ${nested ? "rail-row--nested" : ""} ${marker ? "rail-row--strong" : ""}`.trim()}
        aria-current={selected ? "page" : undefined}
        aria-label={`${row.title}${marker ? `, ${marker.label}` : ""}`}
        title={row.markerReason ?? row.title}
        onClick={() => store.openConversation(row.id)}
      >
        <span className="rail-row__icon">
          <Avatar
            name={row.leadName ?? row.title}
            provider={row.provider}
            size={20}
            status={rowStatus(row)}
          />
        </span>
        <span className="rail-row__label">{row.title}</span>
        <span className="rail-row__trail">
          {marker?.tone === "complete" ? <Dot tone="complete" /> : null}
          <span className="rail-row__time">{relativeTime(row.updatedAt)}</span>
        </span>
      </button>
    </div>
  );
}

export function AddProjectDialog({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const store = useStore();
  const [path, setPath] = useState("");
  const [name, setName] = useState("");
  const [isolated, setIsolated] = useState(true);
  const busy = store.isBusy(`project.register:${path.trim().toLowerCase()}`);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!path.trim()) return;
    const result = (await store.mutate(
      "project.register",
      {
        path: path.trim(),
        displayName: name.trim() || null,
        checkoutMode: isolated ? "isolated" : "lead-main",
      },
      `project.register:${path.trim().toLowerCase()}`,
    )) as { project?: { id?: string } } | null;
    if (!result) return;
    setPath("");
    setName("");
    onClose();
    store.toast({ tone: "info", title: "Project added" });
    await store.refresh("projects");
    if (result.project?.id) store.selectSpace(result.project.id);
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Add a project"
      description="A local Git repository. Its conversations, agents' worktrees, and history stay inside it."
      width={480}
    >
      <form className="stack" onSubmit={(event) => void submit(event)}>
        <Field label="Repository path" htmlFor="add-project-path">
          <input
            id="add-project-path"
            className="input mono-input"
            placeholder="C:\workspace\my-repo"
            value={path}
            data-autofocus
            onChange={(event) => setPath(event.target.value)}
          />
        </Field>
        <Field
          label="Name"
          htmlFor="add-project-name"
          hint="Defaults to the folder name."
        >
          <input
            id="add-project-name"
            className="input"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <label className="check-row">
          <input
            type="checkbox"
            checked={isolated}
            onChange={(event) => setIsolated(event.target.checked)}
          />
          <span>
            <strong>Isolate every agent in its own worktree</strong>
            <span className="meta">
              Recommended. Agents never write to your checkout or to each
              other&apos;s branches.
            </span>
          </span>
        </label>
        <div className="form-actions">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            type="submit"
            variant="primary"
            loading={busy}
            disabled={!path.trim()}
          >
            Add project
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
