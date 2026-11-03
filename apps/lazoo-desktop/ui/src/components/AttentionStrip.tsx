import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";

import { useAppState, useStore } from "../lib/context";
import type {
  InboxItem,
  NativeTrustRequestRecord,
  WorkspaceConversationResult,
} from "../lib/protocol";
import {
  middleTruncate,
  providerLabel,
  deadlineIn,
  relativeTime,
} from "../lib/util";
import { ActionButton } from "../pages/Conversation";
import { Button, EASE } from "./primitives";

export function AttentionStrip({
  conversation,
}: {
  conversation: WorkspaceConversationResult;
}) {
  const { trust } = useAppState();
  const sessionIds = new Set<string>();
  if (conversation.lead.session) sessionIds.add(conversation.lead.session.id);
  for (const worker of conversation.workers) {
    if (worker.session) sessionIds.add(worker.session.id);
  }
  const trustHere = trust.filter((request) =>
    sessionIds.has(request.sessionId),
  );

  return (
    <div
      className="attention"
      role="region"
      aria-label="Needs you"
      aria-live="polite"
    >
      <AnimatePresence initial={false}>
        {trustHere.map((request) => (
          <TrustItem key={request.id} request={request} />
        ))}
        {conversation.attention.map((item) => (
          <AttentionItem key={item.id} item={item} />
        ))}
      </AnimatePresence>
    </div>
  );
}

const itemMotion = {
  layout: true,
  initial: { opacity: 0, y: 6 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -6 },
  transition: { duration: 0.22, ease: EASE },
};

function AttentionItem({ item }: { item: InboxItem }) {
  const watching = item.attention !== "action-required";
  return (
    <motion.div
      className={`attention__item ${watching ? "attention__item--watching" : ""}`.trim()}
      role="group"
      aria-label={item.title}
      {...itemMotion}
    >
      <span
        className="dot"
        data-tone={watching ? "waiting" : "attention"}
        aria-hidden="true"
      />
      <span className="attention__text">
        <strong>{item.scope.employeeName ?? item.subtype}</strong> {item.title}
        <span className="meta">
          {item.requiredAction}
          {item.deadline
            ? ` · ${item.deadline.kind} ${deadlineIn(item.deadline.at)}`
            : ""}
          {` · ${relativeTime(item.createdAt)}`}
        </span>
      </span>
      <span className="attention__actions">
        {item.actions.map((action, index) => (
          <ActionButton
            key={action.action}
            action={action}
            primary={index === 0 && !watching}
          />
        ))}
      </span>
    </motion.div>
  );
}

function TrustItem({ request }: { request: NativeTrustRequestRecord }) {
  const store = useStore();
  const [auto, setAuto] = useState(false);
  const busy = store.isBusy(`trust:${request.id}`);
  return (
    <motion.div
      className="attention__item"
      role="group"
      aria-label="Native workspace trust"
      {...itemMotion}
    >
      <span className="dot" data-tone="attention" aria-hidden="true" />
      <span className="attention__text">
        <strong>{providerLabel(request.provider)}</strong> asks to trust{" "}
        <span className="mono">
          {middleTruncate(
            request.canonicalWorkspacePath.replace(/^\\\\\?\\/, ""),
            56,
          )}
        </span>
        <span className="meta">
          {request.managedWorkspace
            ? "Lazoo-managed workspace"
            : "unmanaged path"}{" "}
          · {relativeTime(request.createdAt)}
        </span>
      </span>
      <span className="attention__actions">
        <label className="chip" style={{ cursor: "default" }}>
          <input
            type="checkbox"
            checked={auto}
            disabled={!request.managedWorkspace}
            onChange={(event) => setAuto(event.target.checked)}
          />
          Auto-accept managed next time
        </label>
        <Button
          size="sm"
          variant="primary"
          disabled={busy}
          onClick={() => void store.resolveTrust(request, "approve", auto)}
        >
          Approve
        </Button>
        <Button
          size="sm"
          variant="ghost"
          disabled={busy}
          onClick={() => void store.resolveTrust(request, "decline", false)}
        >
          Decline
        </Button>
      </span>
    </motion.div>
  );
}
