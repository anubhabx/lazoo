import type { ButtonHTMLAttributes, ReactNode } from "react";
import { motion } from "motion/react";

import { words } from "../lib/util";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "default" | "primary" | "ghost" | "danger";
  size?: "md" | "sm";
  icon?: boolean;
};

export function Button({
  variant = "default",
  size = "md",
  icon = false,
  className = "",
  type = "button",
  ...rest
}: ButtonProps) {
  const classes = [
    "btn",
    variant !== "default" ? `btn--${variant}` : "",
    size === "sm" ? "btn--sm" : "",
    icon ? "btn--icon" : "",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return <button type={type} className={classes} {...rest} />;
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
      <span className="dot" data-tone={tone} aria-hidden="true" />
      <span>{label ?? words(tone)}</span>
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
              transition={{ duration: 0.15, ease: [0.22, 1, 0.36, 1] }}
            />
          ) : null}
          {option.label}
        </button>
      ))}
    </div>
  );
}

export function Empty({
  title,
  detail,
  action,
}: {
  title: string;
  detail?: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="empty__title">{title}</span>
      {detail ? <span>{detail}</span> : null}
      {action}
    </div>
  );
}

export const EASE: [number, number, number, number] = [0.22, 1, 0.36, 1];

export const fade = {
  initial: { opacity: 0, y: 4 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -4 },
  transition: { duration: 0.22, ease: EASE },
};

export const fadeFast = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0 },
  transition: { duration: 0.15, ease: EASE },
};
