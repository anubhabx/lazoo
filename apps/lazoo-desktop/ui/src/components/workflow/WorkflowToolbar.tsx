// The editor's 40 px toolbar: where you are, whether it is saved, and what you can do.

import { ChevronLeft, Play, TriangleAlert } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { WorkflowIssue } from "../../lib/workflows/graph";
import type { Workflow } from "../../lib/workflows/types";
import { Button, Switch } from "../primitives";
import { plural } from "./model";

function NameField({
  name,
  onCommit,
}: {
  name: string;
  onCommit: (name: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(name);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (editing) input.current?.select();
  }, [editing]);

  function commit() {
    setEditing(false);
    const next = draft.trim();
    if (next && next !== name) onCommit(next);
  }

  if (editing) {
    return (
      <input
        ref={input}
        className="wf-name wf-name--input"
        aria-label="Workflow name"
        value={draft}
        autoFocus
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit();
          if (event.key === "Escape") setEditing(false);
        }}
      />
    );
  }
  return (
    <button
      type="button"
      className="wf-name"
      title="Rename workflow"
      aria-label={`Rename workflow: ${name}`}
      onClick={() => {
        setDraft(name);
        setEditing(true);
      }}
    >
      <span className="truncate">{name}</span>
    </button>
  );
}

function IssuesMenu({
  issues,
  onPick,
}: {
  issues: WorkflowIssue[];
  onPick: (issue: WorkflowIssue) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    function onDown(event: MouseEvent) {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
      }
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);
  if (!issues.length) return null;
  return (
    <span className="menu-anchor" ref={root}>
      <button
        type="button"
        className="chip wf-issues"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <TriangleAlert size={12} aria-hidden="true" />
        {plural(issues.length, "issue")}
      </button>
      {open ? (
        <div
          className="menu wf-issues__menu"
          role="menu"
          aria-label="Issues"
          data-align="end"
        >
          {issues.map((issue, index) => (
            <button
              key={`${issue.nodeId ?? "workflow"}-${index}`}
              type="button"
              role="menuitem"
              className="menu__item wf-issues__item"
              onClick={() => {
                setOpen(false);
                onPick(issue);
              }}
            >
              {issue.message}
            </button>
          ))}
        </div>
      ) : null}
    </span>
  );
}

export function WorkflowToolbar({
  workflow,
  issues,
  saving,
  running,
  runsOpen,
  onBack,
  onRename,
  onIssue,
  onToggleRuns,
  onEnabled,
  onRun,
  onCancel,
}: {
  workflow: Workflow;
  issues: WorkflowIssue[];
  saving: boolean;
  running: boolean;
  runsOpen: boolean;
  onBack: () => void;
  onRename: (name: string) => void;
  onIssue: (issue: WorkflowIssue) => void;
  onToggleRuns: () => void;
  onEnabled: (enabled: boolean) => void;
  onRun: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="wf-toolbar" role="toolbar" aria-label="Workflow">
      <Button
        variant="ghost"
        size="sm"
        icon
        tip="Back to workflows"
        tipSide="bottom"
        aria-label="Back to workflows"
        onClick={onBack}
      >
        <ChevronLeft size={15} aria-hidden="true" />
      </Button>
      <NameField name={workflow.name} onCommit={onRename} />
      <span className="wf-save" role="status" aria-live="polite">
        {saving ? "Saving…" : "Saved"}
      </span>
      <span className="spacer" />
      <IssuesMenu issues={issues} onPick={onIssue} />
      <Button
        variant="ghost"
        size="sm"
        aria-pressed={runsOpen}
        onClick={onToggleRuns}
      >
        Runs
      </Button>
      <span className="wf-enable">
        <Switch
          label="Enable workflow"
          checked={workflow.enabled}
          onChange={onEnabled}
        />
        <span className="wf-enable__text" aria-hidden="true">
          {workflow.enabled ? "On" : "Off"}
        </span>
      </span>
      {running ? (
        <Button size="sm" onClick={onCancel}>
          Cancel run
        </Button>
      ) : (
        <Button variant="primary" size="sm" onClick={onRun}>
          <Play size={12} aria-hidden="true" />
          Run now
        </Button>
      )}
    </div>
  );
}
