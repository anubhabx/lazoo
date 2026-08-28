import { ChevronRight, FolderGit2, History } from "lucide-react";
import {
  ListBox,
  ListBoxItem,
  type Selection as AriaSelection,
} from "react-aria-components";

import type { EventViewModel, ProjectViewModel, Selection } from "../model";

interface SelectionHandler {
  (selection: Selection, origin?: HTMLElement): void;
}

function selectedKeys(selection: Selection | null, kind: Selection["kind"]) {
  return selection?.kind === kind ? new Set([selection.id]) : new Set<string>();
}

function firstKey(keys: AriaSelection) {
  if (keys === "all") return null;
  const key = keys.values().next().value;
  return typeof key === "string" ? key : key == null ? null : String(key);
}

function htmlElement(element: Element) {
  return element instanceof HTMLElement ? element : undefined;
}

interface ProjectRowsProps {
  projects: ProjectViewModel[];
  selection: Selection | null;
  onSelect: SelectionHandler;
}

export function ProjectRows({
  projects,
  selection,
  onSelect,
}: ProjectRowsProps) {
  if (projects.length === 0) {
    return (
      <div className="empty-state">
        <FolderGit2 aria-hidden size={20} strokeWidth={1.6} />
        <div>
          <strong>No registered projects</strong>
          <p>
            Register a Git repository from the Lazoo CLI to populate this view.
          </p>
          <code>lazooctl project register C:\path\to\repository</code>
        </div>
      </div>
    );
  }

  return (
    <ListBox
      aria-label="Registered projects"
      className="operational-list"
      selectionMode="single"
      selectionBehavior="replace"
      selectedKeys={selectedKeys(selection, "project")}
      onSelectionChange={(keys) => {
        const id = firstKey(keys);
        if (id)
          onSelect(
            { kind: "project", id },
            document.activeElement as HTMLElement,
          );
      }}
    >
      {projects.map((project) => (
        <ListBoxItem
          id={project.id}
          key={project.id}
          aria-label={`${project.name} — ${project.path}`}
          textValue={`${project.name} ${project.path} ${project.checkoutLabel}`}
          className="operational-row project-row"
          onPress={(event) =>
            onSelect(
              { kind: "project", id: project.id },
              htmlElement(event.target),
            )
          }
        >
          <FolderGit2
            className="row-icon"
            aria-hidden
            size={15}
            strokeWidth={1.7}
          />
          <span className="row-primary">{project.name}</span>
          <span className="row-secondary technical" title={project.path}>
            {project.pathDisplay}
          </span>
          <span className="row-state">{project.checkoutLabel}</span>
          <span className="row-meta">r{project.revision}</span>
          <ChevronRight
            className="row-chevron"
            aria-hidden
            size={14}
            strokeWidth={1.8}
          />
        </ListBoxItem>
      ))}
    </ListBox>
  );
}

interface EventRowsProps {
  events: EventViewModel[];
  selection: Selection | null;
  onSelect: SelectionHandler;
}

export function EventRows({ events, selection, onSelect }: EventRowsProps) {
  if (events.length === 0) {
    return (
      <div className="empty-state">
        <History aria-hidden size={20} strokeWidth={1.6} />
        <div>
          <strong>No recent activity</strong>
          <p>
            Durable control-plane events will appear here as Lazoo records them.
          </p>
        </div>
      </div>
    );
  }

  return (
    <ListBox
      aria-label="Recent activity"
      className="operational-list"
      selectionMode="single"
      selectionBehavior="replace"
      selectedKeys={selectedKeys(selection, "event")}
      onSelectionChange={(keys) => {
        const id = firstKey(keys);
        if (id)
          onSelect(
            { kind: "event", id },
            document.activeElement as HTMLElement,
          );
      }}
    >
      {events.map((event) => (
        <ListBoxItem
          id={event.id}
          key={event.id}
          aria-label={`${event.label} — ${event.summary}`}
          textValue={`${event.label} ${event.summary} ${event.occurredAt}`}
          className="operational-row event-row"
          onPress={(press) =>
            onSelect({ kind: "event", id: event.id }, htmlElement(press.target))
          }
        >
          <span className="event-sequence">#{event.sequence}</span>
          <span className="row-primary">{event.label}</span>
          <span className="row-secondary">{event.summary}</span>
          <time className="row-meta" title={event.timestamp}>
            {event.occurredAt}
          </time>
          <ChevronRight
            className="row-chevron"
            aria-hidden
            size={14}
            strokeWidth={1.8}
          />
        </ListBoxItem>
      ))}
    </ListBox>
  );
}
