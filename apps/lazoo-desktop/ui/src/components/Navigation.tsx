import { Activity, FolderGit2, Gauge, Server } from "lucide-react";
import type { ComponentType } from "react";

import type { ViewId } from "../model";
import { TooltipButton } from "./primitives";

interface NavigationItem {
  id: ViewId;
  label: string;
  icon: ComponentType<{
    "aria-hidden"?: boolean;
    size?: number;
    strokeWidth?: number;
  }>;
}

const navigationItems: NavigationItem[] = [
  { id: "control", label: "Control room", icon: Gauge },
  { id: "projects", label: "Projects", icon: FolderGit2 },
  { id: "activity", label: "Activity", icon: Activity },
  { id: "system", label: "System", icon: Server },
];

interface NavigationProps {
  activeView: ViewId;
  onNavigate: (view: ViewId) => void;
}

export function Navigation({ activeView, onNavigate }: NavigationProps) {
  return (
    <nav className="navigation" aria-label="Primary">
      <div className="navigation-section-label">Workspace</div>
      <div className="navigation-items">
        {navigationItems.map((item) => {
          const Icon = item.icon;
          return (
            <TooltipButton
              key={item.id}
              className="navigation-item"
              aria-label={item.label}
              aria-current={activeView === item.id ? "page" : undefined}
              tooltip={item.label}
              onPress={() => onNavigate(item.id)}
            >
              <Icon aria-hidden size={16} strokeWidth={1.8} />
              <span className="navigation-label">{item.label}</span>
            </TooltipButton>
          );
        })}
      </div>

      <div className="navigation-footnote">
        <span className="navigation-footnote-mark" aria-hidden />
        <span className="navigation-label">Read-only control plane</span>
      </div>
    </nav>
  );
}

export const viewLabels: Record<ViewId, string> = {
  control: "Control room",
  projects: "Projects",
  activity: "Activity",
  system: "System",
};
