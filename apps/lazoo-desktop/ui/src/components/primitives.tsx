// The base components. Every page builds from these; their look lives in
// styles/components.css and their values in design-system/tokens.css.

import {
  useEffect,
  useId,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type ReactNode,
} from "react";
import { AnimatePresence, motion } from "motion/react";

import { words } from "../lib/util";

export const EASE: [number, number, number, number] = [0.22, 1, 0.36, 1];
/** Settles quickly without overshoot: the feel of every moving surface. */
export const SPRING = {
  type: "spring",
  stiffness: 520,
  damping: 42,
  mass: 0.7,
} as const;

export const fade = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0 },
  transition: { duration: 0.16, ease: EASE },
};

export const fadeFast = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0 },
  transition: { duration: 0.12, ease: EASE },
};

export const rise = {
  initial: { opacity: 0, y: 4 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0 },
  transition: { duration: 0.16, ease: EASE },
};

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "ghost" | "danger" | "accent";
  size?: "md" | "sm" | "lg";
  icon?: boolean;
  loading?: boolean;
  /** Tooltip text; also the accessible name when none is given. */
  tip?: string;
  tipSide?: "bottom" | "top" | "right" | "left";
};

export function Button({
  variant = "default",
  size = "md",
  icon = false,
  loading = false,
  tip,
  tipSide,
  className = "",
  type = "button",
  disabled,
  children,
  ...rest
}: ButtonProps) {
  const classes = [
    "btn",
    variant !== "default" ? `btn--${variant}` : "",
    size !== "md" ? `btn--${size}` : "",
    icon ? "btn--icon" : "",
    loading ? "is-loading" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      type={type}
      className={classes}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      data-tip={tip}
      data-tip-side={tip ? tipSide : undefined}
      aria-label={rest["aria-label"] ?? (icon ? tip : undefined)}
      {...rest}
    >
      {loading ? <Spinner /> : null}
      {children}
    </button>
  );
}

export function Spinner({ size = 12 }: { size?: number }) {
  return (
    <span
      className="spinner"
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  );
}

export function Kbd({ keys }: { keys: string[] }) {
  return (
    <span className="kbd-group" aria-hidden="true">
      {keys.map((key) => (
        <kbd key={key} className="kbd">
          {key}
        </kbd>
      ))}
    </span>
  );
}

export function Dot({ tone }: { tone: string }) {
  return <span className="dot" data-tone={tone} aria-hidden="true" />;
}

/** A status dot is never alone: the word travels with it. */
export function StatusWord({
  tone,
  label,
  className = "",
}: {
  tone: string;
  label?: string;
  className?: string;
}) {
  return (
    <span className={`status-word ${className}`.trim()}>
      <Dot tone={tone} />
      <span>{label ?? words(tone)}</span>
    </span>
  );
}

export function Badge({
  tone = "neutral",
  children,
  title,
}: {
  tone?: "neutral" | "accent" | "success" | "warning" | "danger";
  children: ReactNode;
  title?: string;
}) {
  return (
    <span className="badge" data-tone={tone} title={title}>
      {children}
    </span>
  );
}

/** An agent's mark: see AgentAvatar (dithered glyph, animated by state). */
export { AgentAvatar as Avatar } from "./AgentAvatar";

const GLYPH_HUES = [
  "var(--lz-iris-400)",
  "var(--lz-teal-400)",
  "var(--lz-lilac-400)",
  "var(--lz-sky-400)",
  "var(--lz-mint-400)",
  "var(--lz-peach-400)",
  "var(--lz-rose-400)",
];

export function glyphHue(name: string): string {
  let hash = 0;
  for (const char of name) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  return GLYPH_HUES[Math.abs(hash) % GLYPH_HUES.length]!;
}

/** A project's mark: a tinted tile with its initial, stable per name. */
export function ProjectGlyph({
  name,
  size = 18,
}: {
  name: string;
  size?: number;
}) {
  const style = {
    "--glyph-hue": glyphHue(name),
    width: size,
    height: size,
    fontSize: Math.round(size * 0.55),
  } as CSSProperties;
  return (
    <span className="glyph" style={style} aria-hidden="true">
      {(name.trim()[0] ?? "?").toUpperCase()}
    </span>
  );
}

export function Field({
  label,
  hint,
  children,
  htmlFor,
}: {
  label: string;
  hint?: string;
  children: ReactNode;
  htmlFor?: string;
}) {
  return (
    <div className="field">
      <label className="field__label" htmlFor={htmlFor}>
        {label}
      </label>
      {children}
      {hint ? <span className="field__hint">{hint}</span> : null}
    </div>
  );
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  name,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
  label: string;
  name: string;
}) {
  return (
    <div className="segmented" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          aria-pressed={option.value === value}
          onClick={() => onChange(option.value)}
        >
          {option.value === value ? (
            <motion.span
              layoutId={`segmented-${name}`}
              className="segmented__thumb"
              transition={SPRING}
            />
          ) : null}
          <span className="segmented__label">{option.label}</span>
        </button>
      ))}
    </div>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
    >
      <span className="switch__thumb" />
    </button>
  );
}

export function Skeleton({
  width = "100%",
  height = 12,
  radius,
}: {
  width?: number | string;
  height?: number;
  radius?: number;
}) {
  return (
    <span
      className="skeleton"
      style={{ width, height, borderRadius: radius }}
      aria-hidden="true"
    />
  );
}

export function Empty({
  title,
  detail,
  action,
  icon,
}: {
  title: string;
  detail?: string;
  action?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <div className="empty">
      {icon ? <span className="empty__icon">{icon}</span> : null}
      <span className="empty__title">{title}</span>
      {detail ? <span className="empty__detail">{detail}</span> : null}
      {action}
    </div>
  );
}

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/**
 * A modal surface: scrim, focus kept inside, Escape and the scrim close it,
 * focus returns to where it came from.
 */
export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  width = 560,
  label,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
  /** Accessible name when the title is not plain text. */
  label?: string;
}) {
  const panel = useRef<HTMLDivElement>(null);
  // The latest close handler, without re-running the focus effect each render.
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    const frame = requestAnimationFrame(() => {
      const target =
        panel.current?.querySelector<HTMLElement>("[data-autofocus]") ??
        panel.current?.querySelector<HTMLElement>(FOCUSABLE);
      (target ?? panel.current)?.focus();
    });
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        closeRef.current();
        return;
      }
      if (event.key !== "Tab" || !panel.current) return;
      const items = [
        ...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE),
      ].filter(
        (item) => item.offsetParent !== null || item === document.activeElement,
      );
      if (!items.length) return;
      const first = items[0]!;
      const last = items[items.length - 1]!;
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
    window.addEventListener("keydown", onKey, true);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKey, true);
      previous?.focus?.();
    };
  }, [open]);

  return (
    <AnimatePresence>
      {open ? (
        <motion.div
          key="dialog"
          className="dialog-layer"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18, ease: EASE }}
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) onClose();
          }}
        >
          <motion.div
            ref={panel}
            className="dialog"
            role="dialog"
            aria-modal="true"
            aria-label={label}
            aria-labelledby={label ? undefined : titleId}
            aria-describedby={description ? descriptionId : undefined}
            tabIndex={-1}
            style={{ width: `min(${width}px, calc(100vw - 32px))` }}
            initial={{ opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.99 }}
            transition={SPRING}
          >
            <div className="dialog__head">
              <h2 id={titleId} className="dialog__title">
                {title}
              </h2>
              {description ? (
                <p id={descriptionId} className="dialog__description">
                  {description}
                </p>
              ) : null}
            </div>
            <div className="dialog__body">{children}</div>
            {footer ? <div className="dialog__foot">{footer}</div> : null}
          </motion.div>
        </motion.div>
      ) : null}
    </AnimatePresence>
  );
}

export interface MenuItem {
  label: string;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
}

/**
 * A small popover menu: secondary actions live here so a surface keeps one
 * primary action. Escape, outside click and selection close it.
 */
export function Menu({
  label,
  items,
  children,
  align = "end",
}: {
  label: string;
  items: MenuItem[];
  children: ReactNode;
  align?: "start" | "end";
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    if (!open) return;
    function onDown(event: MouseEvent) {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    }
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        setOpen(false);
      }
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);
  if (!items.length) return null;
  return (
    <span className="menu-anchor" ref={root}>
      <Button
        variant="ghost"
        size="sm"
        icon
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {children}
      </Button>
      {open ? (
        <div className="menu" role="menu" aria-label={label} data-align={align}>
          {items.map((item) => (
            <button
              key={item.label}
              type="button"
              role="menuitem"
              className="menu__item"
              data-danger={item.danger || undefined}
              disabled={item.disabled}
              onClick={() => {
                setOpen(false);
                item.onSelect();
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      ) : null}
    </span>
  );
}
