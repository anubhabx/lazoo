import { CalendarClock, FolderGit2, Hash, History, X } from "lucide-react";
import type { ReactNode } from "react";

import type { DesktopViewModel, Selection } from "../model";
import { TooltipButton } from "./primitives";

interface InspectorProps {
  model: DesktopViewModel;
  selection: Selection;
  onClose: () => void;
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="inspector-fact">
      <dt>{label}</dt>
      <dd>{children}</dd>
    </div>
  );
}

export function Inspector({ model, selection, onClose }: InspectorProps) {
  const project =
    selection.kind === "project"
      ? model.projects.find((candidate) => candidate.id === selection.id)
      : null;
  const event =
    selection.kind === "event"
      ? model.events.find((candidate) => candidate.id === selection.id)
      : null;

  if (!project && !event) return null;

  return (
    <aside className="inspector" aria-label="Inspector">
      <header className="inspector-header">
        <div>
          <span className="section-kicker">
            {project ? "Project" : "Event"}
          </span>
          <h2>{project?.name ?? event?.label}</h2>
        </div>
        <TooltipButton
          className="inspector-close"
          aria-label="Close inspector"
          tooltip="Close inspector"
          onPress={onClose}
        >
          <X aria-hidden size={16} strokeWidth={1.8} />
        </TooltipButton>
      </header>

      {project ? (
        <div className="inspector-content">
          <div className="inspector-lead">
            <FolderGit2 aria-hidden size={18} strokeWidth={1.7} />
            <p className="technical" title={project.path}>
              {project.pathDisplay}
            </p>
          </div>
          <dl className="inspector-facts">
            <Fact label="Checkout">{project.checkoutLabel}</Fact>
            <Fact label="Revision">
              <span className="technical">{project.revision}</span>
            </Fact>
            <Fact label="Registered">{project.createdAt}</Fact>
          </dl>
          <div className="inspector-note">
            <Hash aria-hidden size={14} strokeWidth={1.7} />
            <p>
              This is a read-only registry snapshot. Project changes remain
              CLI-only.
            </p>
          </div>
        </div>
      ) : null}

      {event ? (
        <div className="inspector-content">
          <div className="inspector-lead">
            <History aria-hidden size={18} strokeWidth={1.7} />
            <p>{event.summary}</p>
          </div>
          <dl className="inspector-facts">
            <Fact label="Sequence">
              <span className="technical">#{event.sequence}</span>
            </Fact>
            <Fact label="Occurred">{event.timestamp}</Fact>
            <Fact label="Relative time">{event.occurredAt}</Fact>
          </dl>
          <div className="inspector-note">
            <CalendarClock aria-hidden size={14} strokeWidth={1.7} />
            <p>
              Events are shown newest first from the durable control-plane
              ledger.
            </p>
          </div>
        </div>
      ) : null}
    </aside>
  );
}
