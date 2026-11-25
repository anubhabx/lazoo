// The add-step popover: every kind in the catalog, grouped, with a line of meaning.

import { useEffect, useMemo, useRef, type CSSProperties } from "react";

import {
  NODE_CATALOG,
  NODE_KINDS,
  type NodeGroup,
  type NodeKind,
} from "../../lib/workflows/types";
import { KindTile } from "./WorkflowNode";

const GROUPS: NodeGroup[] = ["Triggers", "Agents", "Logic", "Output"];

export function NodePicker({
  onPick,
  onClose,
  style,
  triggers = true,
  label = "Add a step",
}: {
  onPick: (kind: NodeKind) => void;
  onClose: () => void;
  style?: CSSProperties;
  /** False when the step follows another one: triggers take no input. */
  triggers?: boolean;
  label?: string;
}) {
  const root = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;

  const groups = useMemo(
    () =>
      GROUPS.map((group) => ({
        group,
        kinds: NODE_KINDS.filter(
          (kind) =>
            NODE_CATALOG[kind].group === group &&
            (triggers || NODE_CATALOG[kind].hasInput),
        ),
      })).filter((entry) => entry.kinds.length),
    [triggers],
  );

  useEffect(() => {
    root.current?.querySelector<HTMLElement>("button")?.focus();
    function onDown(event: MouseEvent) {
      const target = event.target as HTMLElement;
      if (target.closest?.("[data-picker-anchor]")) return;
      if (!root.current?.contains(target)) closeRef.current();
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        closeRef.current();
        return;
      }
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      const items = [
        ...(root.current?.querySelectorAll<HTMLElement>("button") ?? []),
      ];
      if (!items.length) return;
      event.preventDefault();
      const at = items.indexOf(document.activeElement as HTMLElement);
      const next =
        event.key === "ArrowDown"
          ? (at + 1) % items.length
          : (at - 1 + items.length) % items.length;
      items[next]?.focus();
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, []);

  return (
    <div
      ref={root}
      className="wf-picker"
      role="menu"
      aria-label={label}
      style={style}
    >
      {groups.map(({ group, kinds }) => (
        <div
          key={group}
          className="wf-picker__group"
          role="group"
          aria-label={group}
        >
          <span className="wf-picker__heading">{group}</span>
          {kinds.map((kind) => {
            const entry = NODE_CATALOG[kind];
            return (
              <button
                key={kind}
                type="button"
                role="menuitem"
                className="wf-picker__item"
                onClick={() => onPick(kind)}
              >
                <KindTile kind={kind} size={24} />
                <span className="wf-picker__text">
                  <span className="wf-picker__title">{entry.title}</span>
                  <span className="wf-picker__desc">{entry.description}</span>
                </span>
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}
