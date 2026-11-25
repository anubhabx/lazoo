import { motion } from "motion/react";

import {
  Avatar,
  Button,
  Empty,
  Page,
  PageHeader,
  SPRING,
  Skeleton,
} from "../components/primitives";
import { useAppState, useStore } from "../lib/context";
import type { InboxFilter, InboxItem } from "../lib/protocol";
import { providerLabel, deadlineIn, relativeTime, words } from "../lib/util";
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
    <Page label="Inbox" wide>
      <PageHeader
        title="Inbox"
        description="Approvals, proposals and incidents that wait for you."
        actions={
          <Button variant="ghost" onClick={() => void store.refresh("inbox")}>
            Refresh
          </Button>
        }
        meta={
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
                  {filter === entry.value ? (
                    <motion.span
                      layoutId="inbox-filter"
                      className="filters__thumb"
                      transition={SPRING}
                    />
                  ) : null}
                  {entry.label}
                  {count ? (
                    <span className="filters__count">{count}</span>
                  ) : null}
                </button>
              );
            })}
          </div>
        }
      />
      {!inbox ? (
        <div className="list" aria-busy="true">
          {[0, 1, 2].map((index) => (
            <div key={index} className="item">
              <Skeleton width={8} height={8} radius={4} />
              <div className="item__main">
                <Skeleton width="46%" height={12} />
                <Skeleton width="70%" height={10} />
              </div>
            </div>
          ))}
        </div>
      ) : inbox.items.length === 0 ? (
        <Empty
          title={
            filter === "resolved" ? "Nothing resolved yet" : "Nothing needs you"
          }
          detail={
            filter === "permissions"
              ? "Provider-native permission prompts stay inside the harness terminal until passthrough exists."
              : filter === "resolved"
                ? "Items you approve, decline or dismiss are kept here."
                : "Approvals, proposals, uncertain tasks, and escalated incidents appear here."
          }
        />
      ) : (
        <>
          <div className="list" role="list">
            {inbox.items.map((item, index) => (
              <InboxRow
                key={`${item.kind}:${item.id}`}
                item={item}
                leadRow={index === 0}
              />
            ))}
          </div>
          {inbox.truncated ? (
            <p className="meta list-note">Older items are not shown.</p>
          ) : null}
        </>
      )}
    </Page>
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

function InboxRow({ item, leadRow }: { item: InboxItem; leadRow: boolean }) {
  const store = useStore();
  const conversationId = item.scope.conversationId;
  const open = () => {
    if (!conversationId) return;
    if (item.scope.projectId !== undefined)
      store.selectSpace(item.scope.projectId);
    store.openConversation(conversationId, item.scope.taskId ?? "lead");
  };
  return (
    <div className="item" role="listitem">
      {item.scope.employeeName ? (
        <Avatar
          name={item.scope.employeeName}
          provider={item.scope.provider}
          size={24}
          status={toneFor(item)}
        />
      ) : (
        <span className="dot" data-tone={toneFor(item)} aria-hidden="true" />
      )}
      <div className="item__main">
        <span className="item__title" title={item.title}>
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
            ? ` · ${item.deadline.kind} ${deadlineIn(item.deadline.at)}`
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
            primary={
              leadRow && index === 0 && item.attention === "action-required"
            }
          />
        ))}
      </span>
    </div>
  );
}
