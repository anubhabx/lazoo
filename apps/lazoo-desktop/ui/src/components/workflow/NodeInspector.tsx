// The inspector: one form per node kind. Every edit goes straight to the workflow.

import { Trash2 } from "lucide-react";
import { useId, useState } from "react";

import type { AuthorityMode } from "../../lib/protocol";
import { authorityLabel, providerLabel } from "../../lib/util";
import { describeSchedule, nextFire } from "../../lib/workflows/schedule";
import {
  NODE_CATALOG,
  type NodeConfigs,
  type Schedule,
  type WorkflowNode,
} from "../../lib/workflows/types";
import { useAppState } from "../../lib/context";
import { Button, Field, Segmented, Switch } from "../primitives";
import { AUTHORITY_MEANING, CONDITION_OPS, plural } from "./model";
import { useMinute } from "./useMinute";
import { KindTile } from "./WorkflowNode";

const AUTHORITIES: AuthorityMode[] = ["ask", "self-decision", "full-autonomy"];
/** Monday first: [label, full name, day number with 0 = Sunday]. */
const WEEK: [string, string, number][] = [
  ["M", "Monday", 1],
  ["T", "Tuesday", 2],
  ["W", "Wednesday", 3],
  ["T", "Thursday", 4],
  ["F", "Friday", 5],
  ["S", "Saturday", 6],
  ["S", "Sunday", 0],
];

/** A number input that lets the text be empty or half typed while it is being edited. */
function NumberInput({
  id,
  value,
  min,
  max,
  onCommit,
  label,
}: {
  id?: string;
  value: number;
  min: number;
  max: number;
  onCommit: (value: number) => void;
  label?: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);
  return (
    <input
      id={id}
      className="input"
      type="number"
      inputMode="numeric"
      aria-label={label}
      min={min}
      max={max}
      value={draft ?? String(value)}
      onChange={(event) => {
        const text = event.target.value;
        setDraft(text);
        const parsed = Number(text);
        if (text.trim() !== "" && Number.isFinite(parsed)) {
          onCommit(Math.min(max, Math.max(min, Math.round(parsed))));
        }
      }}
      onBlur={() => setDraft(null)}
    />
  );
}

export function NodeInspector({
  node,
  onLabel,
  onConfig,
  onDelete,
}: {
  node: WorkflowNode;
  onLabel: (label: string) => void;
  onConfig: (config: object) => void;
  onDelete: () => void;
}) {
  const entry = NODE_CATALOG[node.kind];
  const state = useAppState();
  const employee =
    node.kind === "agent.prompt"
      ? state.employees?.find(
          (item) =>
            item.id === (node.config as NodeConfigs["agent.prompt"]).employeeId,
        )
      : undefined;
  return (
    <aside className="wf-panel" aria-label="Inspector">
      <div className="wf-panel__head">
        <KindTile
          kind={node.kind}
          size={32}
          agent={
            employee
              ? { name: employee.name, provider: employee.provider }
              : null
          }
        />
        <div className="wf-panel__titles">
          <span className="wf-panel__kind">{entry.title}</span>
          <input
            className="wf-panel__label"
            aria-label="Step name"
            value={node.label}
            onChange={(event) => onLabel(event.target.value)}
            onBlur={() => {
              if (!node.label.trim()) onLabel(entry.title);
            }}
            spellCheck={false}
          />
        </div>
      </div>
      <p className="wf-panel__desc">{entry.description}</p>
      <div className="wf-panel__body">
        {node.kind === "trigger.schedule" ? (
          <ScheduleForm
            config={node.config as NodeConfigs["trigger.schedule"]}
            onConfig={onConfig}
          />
        ) : null}
        {node.kind === "trigger.manual" ? (
          <p className="wf-note">
            This workflow starts when you press Run now. It never starts on its
            own.
          </p>
        ) : null}
        {node.kind === "agent.prompt" ? (
          <AgentForm
            config={node.config as NodeConfigs["agent.prompt"]}
            onConfig={onConfig}
          />
        ) : null}
        {node.kind === "logic.condition" ? (
          <ConditionForm
            config={node.config as NodeConfigs["logic.condition"]}
            onConfig={onConfig}
          />
        ) : null}
        {node.kind === "logic.wait" ? (
          <WaitForm
            config={node.config as NodeConfigs["logic.wait"]}
            onConfig={onConfig}
          />
        ) : null}
        {node.kind === "notify" ? (
          <NotifyForm
            config={node.config as NodeConfigs["notify"]}
            onConfig={onConfig}
          />
        ) : null}
      </div>
      <div className="wf-panel__foot">
        <Button variant="ghost" size="sm" onClick={onDelete}>
          <Trash2 size={13} aria-hidden="true" />
          Delete step
        </Button>
      </div>
    </aside>
  );
}

// ---- schedule ------------------------------------------------------------------

function ScheduleForm({
  config,
  onConfig,
}: {
  config: NodeConfigs["trigger.schedule"];
  onConfig: (config: object) => void;
}) {
  const schedule = config.schedule;
  const set = (next: Schedule) => onConfig({ schedule: next });
  const timeId = useId();
  const amountId = useId();
  const [hours, setHours] = useState(
    schedule.mode === "interval" &&
      schedule.everyMinutes >= 60 &&
      schedule.everyMinutes % 60 === 0,
  );
  useMinute();
  const next = nextFire(schedule, new Date());
  return (
    <>
      <Field label="Repeat">
        <Segmented
          label="Schedule mode"
          name="wf-schedule-mode"
          value={schedule.mode}
          options={[
            { value: "interval", label: "Every…" },
            { value: "daily", label: "At a time" },
          ]}
          onChange={(mode) => {
            if (mode === schedule.mode) return;
            set(
              mode === "interval"
                ? { mode: "interval", everyMinutes: 30 }
                : { mode: "daily", time: "09:00", days: [1, 2, 3, 4, 5] },
            );
            setHours(false);
          }}
        />
      </Field>
      {schedule.mode === "interval" ? (
        <Field label="Every" htmlFor={amountId}>
          <div className="wf-pair">
            <NumberInput
              id={amountId}
              value={hours ? schedule.everyMinutes / 60 : schedule.everyMinutes}
              min={1}
              max={hours ? 168 : 10080}
              onCommit={(value) =>
                set({
                  mode: "interval",
                  everyMinutes: Math.min(10080, value * (hours ? 60 : 1)),
                })
              }
            />
            <select
              className="select"
              aria-label="Unit"
              value={hours ? "hours" : "minutes"}
              onChange={(event) => {
                const toHours = event.target.value === "hours";
                setHours(toHours);
                // Keep the interval; round up to a whole hour when switching to hours.
                if (toHours) {
                  set({
                    mode: "interval",
                    everyMinutes: Math.min(
                      10080,
                      Math.max(1, Math.ceil(schedule.everyMinutes / 60)) * 60,
                    ),
                  });
                }
              }}
            >
              <option value="minutes">minutes</option>
              <option value="hours">hours</option>
            </select>
          </div>
        </Field>
      ) : (
        <>
          <Field label="Time" htmlFor={timeId}>
            <input
              id={timeId}
              className="input"
              type="time"
              value={schedule.time}
              onChange={(event) => {
                if (/^\d{2}:\d{2}$/.test(event.target.value)) {
                  set({ ...schedule, time: event.target.value });
                }
              }}
            />
          </Field>
          <Field label="Days">
            <div className="wf-days" role="group" aria-label="Days of the week">
              {WEEK.map(([short, name, day]) => {
                const on = schedule.days.includes(day);
                return (
                  <button
                    key={name}
                    type="button"
                    className="wf-day"
                    aria-pressed={on}
                    aria-label={name}
                    title={name}
                    onClick={() => {
                      const days = on
                        ? schedule.days.filter((entry) => entry !== day)
                        : [...schedule.days, day].sort();
                      if (days.length) set({ ...schedule, days });
                    }}
                  >
                    {short}
                  </button>
                );
              })}
            </div>
            <div className="wf-shortcuts">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => set({ ...schedule, days: [1, 2, 3, 4, 5] })}
              >
                Weekdays
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() =>
                  set({ ...schedule, days: [0, 1, 2, 3, 4, 5, 6] })
                }
              >
                Every day
              </Button>
            </div>
          </Field>
        </>
      )}
      <p className="wf-note" aria-live="polite">
        <strong>{describeSchedule(schedule)}</strong>
        <br />
        Next:{" "}
        {next.toLocaleString(undefined, {
          weekday: "short",
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })}
      </p>
    </>
  );
}

// ---- agent ---------------------------------------------------------------------

function AgentForm({
  config,
  onConfig,
}: {
  config: NodeConfigs["agent.prompt"];
  onConfig: (config: object) => void;
}) {
  const state = useAppState();
  const agentId = useId();
  const projectId = useId();
  const authorityId = useId();
  const promptId = useId();
  const timeoutId = useId();
  const employees = (state.employees ?? []).filter(
    (item) => item.enabled || item.id === config.employeeId,
  );
  return (
    <>
      <Field label="Agent" htmlFor={agentId}>
        <select
          id={agentId}
          className="select"
          value={config.employeeId ?? ""}
          onChange={(event) =>
            onConfig({ employeeId: event.target.value || null })
          }
        >
          <option value="">Choose an agent</option>
          {employees.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name} · {providerLabel(item.provider)}
            </option>
          ))}
        </select>
      </Field>
      <Field label="Project" htmlFor={projectId}>
        <select
          id={projectId}
          className="select"
          value={config.spaceId ?? ""}
          onChange={(event) =>
            onConfig({ spaceId: event.target.value || null })
          }
        >
          <option value="">Home</option>
          {state.projects.map((project) => (
            <option key={project.id} value={project.id}>
              {project.displayName}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Authority"
        htmlFor={authorityId}
        hint={AUTHORITY_MEANING[config.authority]}
      >
        <select
          id={authorityId}
          className="select"
          value={config.authority}
          onChange={(event) =>
            onConfig({ authority: event.target.value as AuthorityMode })
          }
        >
          {AUTHORITIES.map((mode) => (
            <option key={mode} value={mode}>
              {authorityLabel(mode)}
            </option>
          ))}
        </select>
      </Field>
      <Field
        label="Prompt"
        htmlFor={promptId}
        hint="Use {{input}} for the previous step's result."
      >
        <textarea
          id={promptId}
          className="textarea"
          rows={6}
          value={config.prompt}
          placeholder="What should the agent do?"
          onChange={(event) => onConfig({ prompt: event.target.value })}
        />
      </Field>
      <div className="wf-switch-row">
        <span>Wait for the reply</span>
        <Switch
          label="Wait for the reply"
          checked={config.waitForReply}
          onChange={(waitForReply) => onConfig({ waitForReply })}
        />
      </div>
      {config.waitForReply ? (
        <Field label="Timeout (minutes)" htmlFor={timeoutId}>
          <NumberInput
            id={timeoutId}
            value={config.timeoutMinutes}
            min={1}
            max={1440}
            onCommit={(timeoutMinutes) => onConfig({ timeoutMinutes })}
          />
        </Field>
      ) : null}
    </>
  );
}

// ---- logic and output ------------------------------------------------------------

function ConditionForm({
  config,
  onConfig,
}: {
  config: NodeConfigs["logic.condition"];
  onConfig: (config: object) => void;
}) {
  const opId = useId();
  const valueId = useId();
  return (
    <>
      <Field label="When the output" htmlFor={opId}>
        <select
          id={opId}
          className="select"
          value={config.op}
          onChange={(event) =>
            onConfig({ op: event.target.value as typeof config.op })
          }
        >
          {CONDITION_OPS.map((op) => (
            <option key={op.value} value={op.value}>
              {op.label}
            </option>
          ))}
        </select>
      </Field>
      {config.op !== "is-empty" ? (
        <Field label="Value" htmlFor={valueId}>
          <input
            id={valueId}
            className="input"
            value={config.value}
            onChange={(event) => onConfig({ value: event.target.value })}
          />
        </Field>
      ) : null}
      <p className="wf-note">
        When the check passes the run continues from True, otherwise from False.
        A branch with nothing connected simply ends the run.
      </p>
    </>
  );
}

function WaitForm({
  config,
  onConfig,
}: {
  config: NodeConfigs["logic.wait"];
  onConfig: (config: object) => void;
}) {
  const id = useId();
  return (
    <Field
      label="Minutes"
      htmlFor={id}
      hint={
        config.minutes === 0 ? "No delay." : plural(config.minutes, "minute")
      }
    >
      <NumberInput
        id={id}
        value={config.minutes}
        min={0}
        max={10080}
        onCommit={(minutes) => onConfig({ minutes })}
      />
    </Field>
  );
}

function NotifyForm({
  config,
  onConfig,
}: {
  config: NodeConfigs["notify"];
  onConfig: (config: object) => void;
}) {
  const titleId = useId();
  const bodyId = useId();
  return (
    <>
      <Field label="Title" htmlFor={titleId}>
        <input
          id={titleId}
          className="input"
          value={config.title}
          onChange={(event) => onConfig({ title: event.target.value })}
        />
      </Field>
      <Field
        label="Body"
        htmlFor={bodyId}
        hint="Use {{input}} for the previous step's result."
      >
        <textarea
          id={bodyId}
          className="textarea"
          rows={4}
          value={config.body}
          onChange={(event) => onConfig({ body: event.target.value })}
        />
      </Field>
    </>
  );
}
