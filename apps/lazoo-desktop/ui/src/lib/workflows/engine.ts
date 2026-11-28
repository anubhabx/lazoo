// Runs one workflow: from a trigger, depth first in edge order, each node at most once.
// Steps act through the daemon bridge (`deps.call`); everything else is injected so the
// whole walk is testable without timers, a daemon or a window.

import type {
  ConversationTurnRecord,
  TurnState,
  WorkspaceLauncherResult,
} from "../protocol";
import { blockFromRecord, stateWord } from "../transcript";
import { outgoing } from "./graph";
import type {
  Handle,
  NodeConfigs,
  RunStep,
  Workflow,
  WorkflowNode,
  WorkflowRun,
} from "./types";

export interface EngineDeps {
  call<T>(method: string, params: unknown): Promise<T>;
  now(): Date;
  /** Resolves after `ms`; rejects when `signal` aborts. */
  sleep(ms: number, signal: AbortSignal): Promise<void>;
  notify(title: string, body: string): void;
  signal: AbortSignal;
  newId(): string;
}

export const STEP_CAP = 100;
export const POLL_MS = 3000;
const TERMINAL: TurnState[] = [
  "completed",
  "terminal-failed",
  "cancelled",
  "uncertain",
];

/** `{{input}}` is the previous step's output. */
export function interpolate(template: string, input: string): string {
  return template.replace(/\{\{\s*input\s*\}\}/g, () => input);
}

export function evaluateCondition(
  config: NodeConfigs["logic.condition"],
  input: string,
): boolean {
  const subject = input.toLowerCase();
  const value = config.value.toLowerCase();
  switch (config.op) {
    case "contains":
      return subject.includes(value);
    case "not-contains":
      return !subject.includes(value);
    case "equals":
      return subject.trim() === value.trim();
    case "is-empty":
      return input.trim() === "";
  }
}

interface StepResult {
  /** Shown in the run history. */
  output: string | null;
  /** Handed to the next step as `input`. */
  next: string;
  handle: Handle;
}

class StepFailure extends Error {}

function messageOf(error: unknown): string {
  if (typeof error === "object" && error !== null && "message" in error) {
    const { message } = error as { message: unknown };
    if (typeof message === "string" && message) return message;
  }
  return String(error);
}

export async function runWorkflow(
  workflow: Workflow,
  triggerNodeId: string,
  deps: EngineDeps,
  onUpdate: (run: WorkflowRun) => void,
): Promise<WorkflowRun> {
  const nodes = new Map(workflow.nodes.map((node) => [node.id, node]));
  const trigger = nodes.get(triggerNodeId);
  const run: WorkflowRun = {
    id: deps.newId(),
    workflowId: workflow.id,
    trigger: trigger?.kind === "trigger.schedule" ? "schedule" : "manual",
    triggerNodeId,
    status: "running",
    startedAt: deps.now().toISOString(),
    finishedAt: null,
    steps: [],
  };
  const snapshot = (): WorkflowRun => ({
    ...run,
    steps: run.steps.map((step) => ({ ...step })),
  });
  const emit = () => onUpdate(snapshot());
  const visited = new Set<string>();
  emit();

  const agentStep = async (
    node: WorkflowNode<"agent.prompt">,
    step: RunStep,
    input: string,
  ): Promise<StepResult> => {
    const config = node.config;
    if (!config.employeeId)
      throw new StepFailure("No agent is chosen for this step.");
    const prompt = interpolate(config.prompt, input).trim();
    if (!prompt) throw new StepFailure("The prompt is empty.");
    const launcher = await deps.call<WorkspaceLauncherResult>(
      "workspace.launcher",
      { projectId: config.spaceId },
    );
    const lead = launcher.leads.find(
      (candidate) => candidate.employeeId === config.employeeId,
    );
    if (!lead)
      throw new StepFailure("The chosen agent is no longer available.");
    if (!lead.startable) {
      throw new StepFailure(
        lead.blockingReason ?? `${lead.name} cannot start a conversation.`,
      );
    }
    const [create, submit] = launcher.start;
    if (!create || !submit) {
      throw new StepFailure("The daemon did not offer a start request.");
    }
    const key = `wf-${run.id}-${node.id}`;
    const created = await deps.call<{ conversation: { id: string } }>(
      create.method,
      {
        ...create.params,
        leadEmployeeId: config.employeeId,
        idempotencyKey: `${key}-create`,
      },
    );
    const conversationId = created.conversation.id;
    step.conversationId = conversationId;
    emit();
    const submitted = await deps.call<{ turn: { id: string } }>(submit.method, {
      ...submit.params,
      conversationId,
      authorityMode: config.authority,
      text: prompt,
      idempotencyKey: `${key}-submit`,
    });
    if (!config.waitForReply) {
      return { output: conversationId, next: conversationId, handle: "out" };
    }
    const deadline = deps.now().getTime() + config.timeoutMinutes * 60_000;
    for (;;) {
      const record = await deps.call<ConversationTurnRecord>("turn.show", {
        id: submitted.turn.id,
      });
      if (TERMINAL.includes(record.state)) {
        if (record.state !== "completed") {
          throw new StepFailure(
            record.failureMessage ?? `The turn ${stateWord(record.state)}.`,
          );
        }
        const reply =
          blockFromRecord(record).assistantText.trim() ||
          stateWord(record.state);
        return { output: reply, next: reply, handle: "out" };
      }
      if (deps.now().getTime() >= deadline) {
        throw new StepFailure(
          `No reply within ${config.timeoutMinutes} minutes.`,
        );
      }
      await deps.sleep(POLL_MS, deps.signal);
    }
  };

  const execute = async (
    node: WorkflowNode,
    step: RunStep,
    input: string,
  ): Promise<StepResult> => {
    switch (node.kind) {
      case "trigger.schedule":
      case "trigger.manual":
        return { output: null, next: "", handle: "out" };
      case "agent.prompt":
        return agentStep(node as WorkflowNode<"agent.prompt">, step, input);
      case "logic.condition": {
        const config = node.config as NodeConfigs["logic.condition"];
        const result = evaluateCondition(config, input);
        const handle: Handle = result ? "true" : "false";
        return { output: handle, next: input, handle };
      }
      case "logic.wait": {
        const { minutes } = node.config as NodeConfigs["logic.wait"];
        await deps.sleep(minutes * 60_000, deps.signal);
        return { output: `Waited ${minutes} min`, next: input, handle: "out" };
      }
      case "notify": {
        const config = node.config as NodeConfigs["notify"];
        const title = interpolate(config.title, input);
        const body = interpolate(config.body, input);
        deps.notify(title, body);
        return { output: body, next: body, handle: "out" };
      }
    }
  };

  /** False when the run must stop (failure or cancellation). */
  const visit = async (nodeId: string, input: string): Promise<boolean> => {
    const node = nodes.get(nodeId);
    if (!node || visited.has(nodeId)) return true;
    if (deps.signal.aborted) {
      run.status = "cancelled";
      return false;
    }
    const step: RunStep = {
      nodeId,
      status: "running",
      startedAt: deps.now().toISOString(),
      finishedAt: null,
      output: null,
      error: null,
      conversationId: null,
    };
    run.steps.push(step);
    if (visited.size >= STEP_CAP) {
      step.status = "failed";
      step.error = `A run is limited to ${STEP_CAP} steps.`;
      step.finishedAt = step.startedAt;
      run.status = "failed";
      return false;
    }
    visited.add(nodeId);
    emit();
    let result: StepResult;
    try {
      result = await execute(node, step, input);
    } catch (error) {
      step.status = "failed";
      step.finishedAt = deps.now().toISOString();
      if (deps.signal.aborted) {
        step.error = "Cancelled";
        run.status = "cancelled";
      } else {
        step.error = messageOf(error);
        run.status = "failed";
      }
      return false;
    }
    step.status = "succeeded";
    step.output = result.output;
    step.finishedAt = deps.now().toISOString();
    emit();
    for (const edge of outgoing(workflow, nodeId, result.handle)) {
      if (!(await visit(edge.target, result.next))) return false;
    }
    return true;
  };

  const completed = trigger ? await visit(triggerNodeId, "") : false;
  if (completed) run.status = "succeeded";
  else if (run.status === "running") run.status = "failed";
  run.finishedAt = deps.now().toISOString();
  emit();
  return snapshot();
}
