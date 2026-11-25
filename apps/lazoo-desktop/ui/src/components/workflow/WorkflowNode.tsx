// The one node component every step kind renders with.

import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import {
  Bell,
  Bot,
  CalendarClock,
  GitBranch,
  Hourglass,
  MousePointerClick,
  TriangleAlert,
  type LucideIcon,
} from "lucide-react";

import {
  NODE_CATALOG,
  type NodeGroup,
  type NodeKind,
  type StepStatus,
} from "../../lib/workflows/types";
import { Avatar, Dot, Spinner } from "../primitives";
import { HANDLE_LABEL, stepTone, stepWord } from "./model";

export const NODE_ICONS: Record<NodeKind, LucideIcon> = {
  "trigger.schedule": CalendarClock,
  "trigger.manual": MousePointerClick,
  "agent.prompt": Bot,
  "logic.condition": GitBranch,
  "logic.wait": Hourglass,
  notify: Bell,
};

export const GROUP_TINT: Record<NodeGroup, string> = {
  Triggers: "var(--lz-status-configured)",
  Agents: "var(--lz-color-icon-primary)",
  Logic: "var(--lz-color-icon-muted)",
  Output: "var(--lz-status-running)",
};

export interface StepNodeData extends Record<string, unknown> {
  kind: NodeKind;
  label: string;
  headline: string;
  detail: string | null;
  muted: boolean;
  agent: { name: string; provider: string } | null;
  issues: string[];
  status: StepStatus | null;
}

export type StepNode = Node<StepNodeData, "step">;

/** The tile that marks a kind: its icon tinted by group, or the chosen agent's avatar. */
export function KindTile({
  kind,
  agent,
  size = 24,
}: {
  kind: NodeKind;
  agent?: { name: string; provider: string } | null;
  size?: number;
}) {
  const Icon = NODE_ICONS[kind];
  return (
    <span
      className="wf-tile"
      style={
        {
          "--tint": GROUP_TINT[NODE_CATALOG[kind].group],
          width: size,
          height: size,
        } as React.CSSProperties
      }
      aria-hidden="true"
    >
      {agent ? (
        <Avatar name={agent.name} provider={agent.provider} size={size - 8} />
      ) : (
        <Icon size={16} strokeWidth={1.75} />
      )}
    </span>
  );
}

export function WorkflowStepNode({ data }: NodeProps<StepNode>) {
  const entry = NODE_CATALOG[data.kind];
  const outputs = entry.outputs;
  const branching = outputs.length > 1;
  const warning = data.issues.length ? data.issues.join(" ") : null;
  return (
    <div className="wf-node" data-status={data.status ?? undefined}>
      {entry.hasInput ? (
        <Handle
          type="target"
          position={Position.Left}
          id="in"
          className="wf-handle"
          aria-label={`${data.label} input`}
        />
      ) : null}
      <div className="wf-node__head">
        <KindTile kind={data.kind} agent={data.agent} />
        <span className="wf-node__titles">
          <span className="wf-node__kind">{entry.title}</span>
          <span className="wf-node__label" title={data.label}>
            {data.label}
          </span>
        </span>
        <span className="wf-node__marks">
          {warning ? (
            <span
              className="wf-node__warn"
              title={warning}
              role="img"
              aria-label={warning}
            >
              <TriangleAlert size={12} aria-hidden="true" />
            </span>
          ) : null}
          {data.status ? (
            <span
              className="wf-node__status"
              data-status={data.status}
              title={stepWord(data.status)}
              role="img"
              aria-label={stepWord(data.status)}
            >
              {data.status === "running" ? (
                <Spinner size={10} />
              ) : (
                <Dot tone={stepTone(data.status)} />
              )}
            </span>
          ) : null}
        </span>
        {!branching ? (
          <Handle
            type="source"
            position={Position.Right}
            id={outputs[0]}
            className="wf-handle"
            aria-label={`${data.label} output`}
          />
        ) : null}
      </div>
      <div className="wf-node__body">
        <span
          className="wf-node__headline"
          data-muted={data.muted || undefined}
          title={data.headline}
        >
          {data.headline}
        </span>
        {data.detail ? (
          <span className="wf-node__detail">{data.detail}</span>
        ) : null}
      </div>
      {branching ? (
        <div className="wf-node__branches">
          {outputs.map((handle) => (
            <div key={handle} className="wf-node__branch">
              <span>{HANDLE_LABEL[handle]}</span>
              <Handle
                type="source"
                position={Position.Right}
                id={handle}
                className="wf-handle"
                aria-label={`${data.label} ${HANDLE_LABEL[handle]} output`}
              />
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
