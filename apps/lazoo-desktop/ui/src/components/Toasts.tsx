import { AnimatePresence, motion } from "motion/react";
import { CircleAlert, CircleCheck, X } from "lucide-react";

import { useAppState, useStore } from "../lib/context";
import { Button, SPRING } from "./primitives";

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
            initial={{ opacity: 0, y: 12, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, x: 24, transition: { duration: 0.16 } }}
            transition={SPRING}
          >
            <span className="toast__icon" aria-hidden="true">
              {toast.tone === "error" ? (
                <CircleAlert size={15} />
              ) : (
                <CircleCheck size={15} />
              )}
            </span>
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
