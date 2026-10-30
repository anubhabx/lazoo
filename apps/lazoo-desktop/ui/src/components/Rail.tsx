import { AnimatePresence, motion } from "motion/react";
import { Inbox, Plus, Settings } from "lucide-react";
import { Fragment, useMemo } from "react";

import { useAppState, useStore } from "../lib/context";
import type { ConversationRow } from "../lib/protocol";
import { dayGroup, relativeTime, type DayGroup } from "../lib/util";
import { EASE } from "./primitives";

const GROUPS: DayGroup[] = ["Today", "Previous 7 days", "Earlier"];

export function markerTone(
  row: ConversationRow,
  seen: Record<string, string>,
): { tone: string; label: string } | null {
  switch (row.marker) {
    case "needs-attention":
      return { tone: "attention", label: "Needs you" };
    case "active":
      return { tone: "running", label: "Active" };
    case "failed":
      return { tone: "failed", label: "Failed" };
    case "completed": {
      const mark = row.lastResultAt ?? row.updatedAt;
      return seen[row.id] === mark
        ? null
        : { tone: "complete", label: "Completed, unread" };
    }
    default:
      return null;
  }
}

export function Rail() {
  const store = useStore();
  const state = useAppState();
  const route = state.route;
  const rows = state.conversations?.conversations ?? [];

  const grouped = useMemo(() => {
    const map = new Map<DayGroup, ConversationRow[]>();
    for (const row of rows) {
      const group = dayGroup(row.updatedAt);
      map.set(group, [...(map.get(group) ?? []), row]);
    }
    return map;
  }, [rows]);

  const unresolved = state.inboxCounts?.unresolved ?? 0;

  return (
    <nav className="rail" aria-label="Spaces and conversations">
      <div className="rail__top">
        <button
          type="button"
          className="row row--strong"
          aria-current={route.kind === "launcher" ? "page" : undefined}
          onClick={() => store.navigate({ kind: "launcher" })}
        >
          <span className="row__icon">
            <Plus size={14} aria-hidden="true" />
          </span>
          <span className="row__label">New work</span>
        </button>
      </div>
      <div
        className="rail__history"
        role="list"
        aria-label="Conversation history"
      >
        <AnimatePresence initial={false}>
          {GROUPS.filter((group) => grouped.has(group)).map((group) => (
            <Fragment key={group}>
              <motion.div
                className="rail__group"
                layout
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.22, ease: EASE }}
              >
                {group}
              </motion.div>
              {grouped.get(group)!.map((row) => {
                const marker = markerTone(row, state.seen);
                const selected =
                  route.kind === "conversation" && route.id === row.id;
                return (
                  <motion.div
                    key={row.id}
                    role="listitem"
                    layout
                    initial={{ opacity: 0, y: 4 }}
                    animate={{ opacity: 1, y: 0 }}
                    exit={{ opacity: 0, y: -4 }}
                    transition={{ duration: 0.22, ease: EASE }}
                  >
                    <button
                      type="button"
                      className={`row ${marker ? "row--strong" : ""}`.trim()}
                      aria-current={selected ? "page" : undefined}
                      aria-label={`${row.title}${marker ? `, ${marker.label}` : ""}`}
                      title={row.markerReason ?? row.title}
                      onClick={() => store.openConversation(row.id)}
                    >
                      <span className="marker">
                        {marker ? (
                          <span
                            className="dot"
                            data-tone={marker.tone}
                            aria-hidden="true"
                          />
                        ) : null}
                      </span>
                      <span className="row__label">{row.title}</span>
                      <span className="row__meta">
                        {relativeTime(row.updatedAt)}
                      </span>
                    </button>
                  </motion.div>
                );
              })}
            </Fragment>
          ))}
        </AnimatePresence>
        {state.conversations && rows.length === 0 ? (
          <div className="rail__group">No conversations yet</div>
        ) : null}
      </div>
      <div className="rail__foot">
        <button
          type="button"
          className="row"
          aria-current={route.kind === "inbox" ? "page" : undefined}
          onClick={() => store.navigate({ kind: "inbox", filter: "all" })}
        >
          <span className="row__icon">
            <Inbox size={14} aria-hidden="true" />
          </span>
          <span className="row__label">Inbox</span>
          <AnimatePresence>
            {unresolved > 0 ? (
              <motion.span
                className="row__badge"
                aria-label={`${unresolved} items need you`}
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.9 }}
                transition={{ duration: 0.15, ease: EASE }}
              >
                {unresolved}
              </motion.span>
            ) : null}
          </AnimatePresence>
        </button>
        <button
          type="button"
          className="row"
          aria-current={route.kind === "settings" ? "page" : undefined}
          onClick={() =>
            store.navigate({ kind: "settings", section: "general" })
          }
        >
          <span className="row__icon">
            <Settings size={14} aria-hidden="true" />
          </span>
          <span className="row__label">Settings</span>
        </button>
      </div>
    </nav>
  );
}
