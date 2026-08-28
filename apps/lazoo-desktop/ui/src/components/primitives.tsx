import { forwardRef, type ReactNode } from "react";
import {
  Button as AriaButton,
  Tooltip as AriaTooltip,
  TooltipTrigger,
  type ButtonProps as AriaButtonProps,
} from "react-aria-components";

interface ButtonProps extends Omit<AriaButtonProps, "className"> {
  className?: string;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(
  function Button({ className = "", ...props }, ref) {
    return (
      <AriaButton
        {...props}
        ref={ref}
        className={`lz-button ${className}`.trim()}
      />
    );
  },
);

interface TooltipButtonProps extends ButtonProps {
  tooltip: ReactNode;
}

export function TooltipButton({ tooltip, ...props }: TooltipButtonProps) {
  return (
    <TooltipTrigger delay={450} closeDelay={0}>
      <Button {...props} />
      <AriaTooltip className="lz-tooltip">{tooltip}</AriaTooltip>
    </TooltipTrigger>
  );
}
