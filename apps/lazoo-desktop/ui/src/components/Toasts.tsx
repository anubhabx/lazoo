import { AnimatePresence, motion } from "motion/react";
import { X } from "lucide-react";

import { useAppState, useStore } from "../lib/context";
import { Button, EASE } from "./primitives";

export function Toasts() {
  const store = useStore();
  const { toasts } = useAppState();
  return (
    <div
      className="toasts"
      role="region"
      aria-live="polite"
      aria-label="Notifications"
    >
      <AnimatePresence initial={false}>
        {toasts.map((toast) => (
          <motion.div
            key={toast.id}
            className={`toast ${toast.tone === "error" ? "toast--error" : ""}`.trim()}
            role="status"
            layout
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: 0.22, ease: EASE }}
          >
            <span className="toast__title">{toast.title}</span>
            {toast.detail ? (
              <span className="toast__detail">{toast.detail}</span>
            ) : null}
            <Button
              variant="ghost"
              size="sm"
              icon
              aria-label="Dismiss notification"
              onClick={() => store.dismissToast(toast.id)}
            >
              <X size={12} aria-hidden="true" />
            </Button>
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}
