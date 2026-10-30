import { AnimatePresence, motion } from "motion/react";

import { Button, EASE, Empty } from "../components/primitives";
import { useAppState, useStore } from "../lib/context";
import type { InboxFilter, InboxItem } from "../lib/protocol";
import { providerLabel, relativeTime, words } from "../lib/util";
import { ActionButton } from "./Conversation";

const FILTERS: { value: InboxFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "approvals", label: "Approvals" },
  { value: "permissions", label: "Permissions" },
  { value: "incidents", label: "Incidents" },
  { value: "resolved", label: "Resolved" },
];

export function InboxPage() {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const filter: InboxFilter = route.kind === "inbox" ? route.filter : "all";
  const inbox = state.inbox;
  const counts = state.inboxCounts;

  function countFor(value: InboxFilter): number | null {
    if (!counts) return null;
    switch (value) {
      case "all":
        return counts.unresolved;
      case "approvals":
        return counts.approvals;
      case "permissions":
        return counts.permissions;
      case "incidents":
        return counts.incidents;
      default:
        return null;
    }
  }

  return (
    <section className="page" aria-label="Inbox">
      <header className="page__header">
        <h1 className="page__title">Inbox</h1>
        <div className="filters" role="tablist" aria-label="Inbox filters">
          {FILTERS.map((entry) => {
            const count = countFor(entry.value);
            return (
              <button
                key={entry.value}
                type="button"
                role="tab"
                aria-selected={filter === entry.value}
                onClick={() =>
                  store.navigate({ kind: "inbox", filter: entry.value })
                }
              >
                {entry.label}
                {count ? ` ${count}` : ""}
              </button>
            );
          })}
        </div>
        <span className="spacer" />
        <Button
          size="sm"
          variant="ghost"
          onClick={() => void store.refresh("inbox")}
        >
          Refresh
        </Button>
      </header>
      <div className="page__body">
        {!inbox ? (
          <span className="meta">Reading the Inbox…</span>
        ) : inbox.items.length === 0 ? (
          <Empty
            title={
              filter === "resolved"
                ? "Nothing resolved yet"
                : "Nothing needs you"
            }
            detail={
              filter === "permissions"
                ? "Provider-native permission prompts stay inside the harness terminal until passthrough exists."
                : "Approvals, proposals, uncertain tasks, and escalated incidents appear here."
            }
          />
        ) : (
          <div className="list" role="list">
            <AnimatePresence initial={false}>
              {inbox.items.map((item) => (
                <InboxRow key={`${item.kind}:${item.id}`} item={item} />
              ))}
            </AnimatePresence>
            {inbox.truncated ? (
              <span className="meta">Older items are not shown.</span>
            ) : null}
          </div>
        )}
      </div>
    </section>
  );
}

function toneFor(item: InboxItem): string {
  if (item.attention === "resolved") return "complete";
  if (item.attention === "acknowledged") return "idle";
  if (item.attention === "watching") return "waiting";
  if (item.kind === "incident")
    return item.severity === "priority" ? "failed" : "blocked";
  return "attention";
}

function InboxRow({ item }: { item: InboxItem }) {
  const store = useStore();
  const conversationId = item.scope.conversationId;
  const open = () => {
    if (!conversationId) return;
    if (item.scope.projectId !== undefined)
      store.selectSpace(item.scope.projectId);
    store.openConversation(conversationId, item.scope.taskId ?? "lead");
  };
  return (
    <motion.div
      className="item"
      role="listitem"
      layout
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -6 }}
      transition={{ duration: 0.22, ease: EASE }}
    >
      <span className="dot" data-tone={toneFor(item)} aria-hidden="true" />
      <div className="item__main">
        <span className="item__title">
          {conversationId ? (
            <button
              type="button"
              onClick={open}
              aria-label={`Open ${item.title}`}
            >
              {item.title}
            </button>
          ) : (
            item.title
          )}
        </span>
        <span className="meta">
          {words(item.kind)}
          {item.scope.employeeName ? ` · ${item.scope.employeeName}` : ""}
          {item.scope.provider
            ? ` · ${providerLabel(item.scope.provider)}`
            : ""}
          {` · ${item.requiredAction}`}
          {item.deadline
            ? ` · ${item.deadline.kind} ${relativeTime(item.deadline.at)}`
            : ""}
          {` · ${relativeTime(item.updatedAt)}`}
          {item.attention !== "action-required"
            ? ` · ${words(item.attention)}`
            : ""}
        </span>
      </div>
      <span className="item__actions">
        {item.actions.map((action, index) => (
          <ActionButton
            key={action.action}
            action={action}
            primary={index === 0 && item.attention === "action-required"}
          />
        ))}
      </span>
    </motion.div>
  );
}
