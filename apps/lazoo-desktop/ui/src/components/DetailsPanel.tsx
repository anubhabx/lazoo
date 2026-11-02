import { motion } from "motion/react";
import { X } from "lucide-react";
import { useEffect, useState } from "react";

import { useAppState, useBridge, useStore } from "../lib/context";
import type { SessionResources, WorkerCard } from "../lib/protocol";
import {
  authorityLabel,
  formatBytes,
  formatDuration,
  providerLabel,
  words,
} from "../lib/util";
import { Button, EASE } from "./primitives";

export function DetailsPanel() {
  const store = useStore();
  const state = useAppState();
  const conversation = state.conversation;
  const route = state.route;
  const tab = route.kind === "conversation" ? route.tab : "lead";
  const worker =
    conversation?.workers.find((card) => card.taskId === tab) ?? null;

  return (
    <motion.aside
      className="details"
      aria-label="Details"
      initial={{ width: 0, opacity: 0 }}
      animate={{ width: "var(--details)", opacity: 1 }}
      exit={{ width: 0, opacity: 0 }}
      transition={{ duration: 0.22, ease: EASE }}
    >
      <div className="details__inner">
        <div className="details__head">
          <h2>Details</h2>
          <Button
            variant="ghost"
            size="sm"
            icon
            aria-label="Close details"
            onClick={() => store.toggleDetails(false)}
          >
            <X size={14} aria-hidden="true" />
          </Button>
        </div>
        {!conversation ? (
          <span className="meta">Loading…</span>
        ) : worker ? (
          <WorkerFacts worker={worker} />
        ) : (
          <LeadFacts />
        )}
      </div>
    </motion.aside>
  );
}

function Fact({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: string | number | null | undefined;
  mono?: boolean;
}) {
  if (value === null || value === undefined || value === "") return null;
  return (
    <>
      <dt>{label}</dt>
      <dd className={mono ? "mono" : undefined}>{value}</dd>
    </>
  );
}

function LeadFacts() {
  const { conversation } = useAppState();
  if (!conversation) return null;
  const { lead, scope, rootTask } = conversation;
  return (
    <>
      <p className="section-title">Lead</p>
      <dl className="facts">
        <Fact label="Employee" value={lead.name} />
        <Fact label="Role" value={lead.role} />
        <Fact label="Harness" value={providerLabel(lead.provider)} />
        <Fact label="Model" value={lead.model} />
        <Fact label="Effort" value={lead.reasoningEffort} />
        <Fact label="Availability" value={words(lead.availability)} />
        <Fact label="Reason" value={lead.availabilityReason} />
        <Fact
          label="Authority"
          value={lead.authority ? authorityLabel(lead.authority) : "not yet"}
        />
        <Fact label="Lifecycle" value={words(lead.lifecycle)} />
      </dl>
      <p className="section-title">Session</p>
      <dl className="facts">
        <Fact label="Session" value={lead.session?.id} mono />
        <Fact
          label="State"
          value={lead.session ? words(lead.session.state) : "none"}
        />
        <Fact label="Host PID" value={lead.session?.nativeHostPid} mono />
        <Fact label="Directory" value={lead.session?.workingDirectory} mono />
        <Fact label="Active turn" value={lead.activeTurnId} mono />
      </dl>
      {lead.session ? <ResourceFacts sessionId={lead.session.id} /> : null}
      <p className="section-title">Conversation</p>
      <dl className="facts">
        <Fact label="Title" value={conversation.title} />
        <Fact label="Space" value={scope.projectName ?? "Home"} />
        <Fact label="Path" value={scope.canonicalPath} mono />
        <Fact
          label="Checkout"
          value={scope.checkoutMode ? words(scope.checkoutMode) : null}
        />
        <Fact label="Root task" value={rootTask?.objective} />
        <Fact
          label="Task state"
          value={rootTask ? words(rootTask.state) : null}
        />
        <Fact label="Id" value={conversation.conversation.id} mono />
        <Fact
          label="Revision"
          value={conversation.conversation.revision}
          mono
        />
        <Fact
          label="Created"
          value={new Date(conversation.conversation.createdAt).toLocaleString()}
        />
      </dl>
    </>
  );
}

function WorkerFacts({ worker }: { worker: WorkerCard }) {
  return (
    <>
      <p className="section-title">Worker</p>
      <dl className="facts">
        <Fact label="Employee" value={worker.name ?? worker.employeeId} />
        <Fact label="Harness" value={providerLabel(worker.provider)} />
        <Fact label="Model" value={worker.model} />
        <Fact label="State" value={words(worker.state)} />
        <Fact label="Why" value={worker.stateReason} />
        <Fact label="Task state" value={words(worker.taskState)} />
        <Fact
          label="Authority"
          value={authorityLabel(worker.effectiveAuthority)}
        />
        <Fact label="Depth" value={worker.depth} />
      </dl>
      <p className="section-title">Task</p>
      <dl className="facts">
        <Fact label="Objective" value={worker.label} />
        <Fact label="Task" value={worker.taskId} mono />
        <Fact label="Parent" value={worker.parentTaskId} mono />
        <Fact
          label="Workspace"
          value={worker.workspace?.kind ? words(worker.workspace.kind) : null}
        />
        <Fact label="Path" value={worker.workspace?.path} mono />
        <Fact label="Branch" value={worker.workspace?.branchName} mono />
        <Fact
          label="Base"
          value={worker.workspace?.baseCommit?.slice(0, 12)}
          mono
        />
        <Fact label="Failure" value={worker.failureCode} mono />
        <Fact label="Message" value={worker.failureMessage} />
        <Fact
          label="Created"
          value={new Date(worker.createdAt).toLocaleString()}
        />
        <Fact
          label="Updated"
          value={new Date(worker.updatedAt).toLocaleString()}
        />
        <Fact
          label="Finished"
          value={
            worker.terminalAt
              ? new Date(worker.terminalAt).toLocaleString()
              : null
          }
        />
      </dl>
      <p className="section-title">Session</p>
      <dl className="facts">
        <Fact label="Session" value={worker.session?.id} mono />
        <Fact
          label="State"
          value={worker.session ? words(worker.session.state) : "none"}
        />
        <Fact label="Active turn" value={worker.activeTurnId} mono />
      </dl>
      {worker.session ? <ResourceFacts sessionId={worker.session.id} /> : null}
    </>
  );
}

function ResourceFacts({ sessionId }: { sessionId: string }) {
  const bridge = useBridge();
  const [resources, setResources] = useState<SessionResources | null>(null);
  const [note, setNote] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setResources(null);
    setNote(null);
    bridge
      .call<SessionResources>("session.resources", { id: sessionId })
      .then((value) => {
        if (!cancelled) setResources(value);
      })
      .catch((error: { message?: string }) => {
        if (!cancelled) setNote(error.message ?? "Not live in this daemon.");
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId, bridge]);

  return (
    <>
      <p className="section-title">Resources</p>
      {resources ? (
        <dl className="facts">
          <Fact
            label="Processes"
            value={`${resources.activeProcesses} active · ${resources.totalProcesses} total`}
          />
          <Fact label="CPU time" value={formatDuration(resources.cpuMillis)} />
          <Fact
            label="Peak memory"
            value={formatBytes(resources.peakJobMemoryBytes)}
          />
          <Fact
            label="Sampled"
            value={new Date(resources.sampledAt).toLocaleTimeString()}
          />
        </dl>
      ) : (
        <span className="meta">{note ?? "Sampling…"}</span>
      )}
    </>
  );
}
