import {
  Activity,
  CornerDownLeft,
  FolderGit2,
  LayoutDashboard,
  RefreshCw,
  Search,
  ServerCog,
  SquareArrowOutUpRight,
} from "lucide-react";
import { useEffect, useMemo, useState, type KeyboardEvent } from "react";
import {
  Dialog,
  Header,
  Input,
  ListBox,
  ListBoxItem,
  ListBoxSection,
  Modal,
  ModalOverlay,
  TextField,
  type Selection as AriaSelection,
} from "react-aria-components";

import {
  buildCommandGroups,
  type CommandContext,
  type CommandDefinition,
} from "../model";
import { Button } from "./primitives";

export const desktopCommands: CommandDefinition[] = [
  {
    id: "refresh",
    label: "Refresh snapshot",
    detail: "Read the latest local daemon state",
    group: "Act",
    keywords: ["sync", "reload"],
  },
  {
    id: "navigate-control",
    label: "Open control room",
    detail: "Return to the operational overview",
    group: "Navigate",
    keywords: ["home", "overview"],
  },
  {
    id: "navigate-projects",
    label: "Open projects",
    detail: "Show the registered project ledger",
    group: "Navigate",
    keywords: ["repository", "repos"],
  },
  {
    id: "navigate-activity",
    label: "Open activity",
    detail: "Show durable control-plane events",
    group: "Navigate",
    keywords: ["events", "history", "ledger"],
  },
  {
    id: "navigate-system",
    label: "Open system",
    detail: "Inspect daemon and storage facts",
    group: "Navigate",
    keywords: ["status", "runtime", "health"],
  },
  {
    id: "inspect-project",
    label: "Inspect selected project",
    detail: "Return to the selected project's details",
    group: "Inspect",
    keywords: ["details", "repository"],
    selectionKind: "project",
  },
  {
    id: "inspect-event",
    label: "Inspect selected event",
    detail: "Return to the selected event's details",
    group: "Inspect",
    keywords: ["details", "ledger"],
    selectionKind: "event",
  },
];

const commandIcons = {
  refresh: RefreshCw,
  "navigate-control": LayoutDashboard,
  "navigate-projects": FolderGit2,
  "navigate-activity": Activity,
  "navigate-system": ServerCog,
  "inspect-project": SquareArrowOutUpRight,
  "inspect-event": SquareArrowOutUpRight,
};

interface CommandPaletteProps {
  isOpen: boolean;
  context: CommandContext;
  onOpenChange: (isOpen: boolean) => void;
  onExecute: (command: CommandDefinition) => void;
  onInspectSelection: () => void;
}

function selectedKey(keys: AriaSelection) {
  if (keys === "all") return null;
  const key = keys.values().next().value;
  return key == null ? null : String(key);
}

export function CommandPalette({
  isOpen,
  context,
  onOpenChange,
  onExecute,
  onInspectSelection,
}: CommandPaletteProps) {
  const [query, setQuery] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const groups = useMemo(
    () => buildCommandGroups(desktopCommands, query, context),
    [context, query],
  );
  const commands = groups.flatMap((group) => group.commands);
  const activeId = commands.some((command) => command.id === selectedId)
    ? selectedId
    : (commands[0]?.id ?? null);

  useEffect(() => {
    if (!isOpen) return;
    setQuery("");
    setSelectedId(null);
  }, [isOpen]);

  const execute = (command: CommandDefinition | undefined) => {
    if (!command) return;
    onExecute(command);
    onOpenChange(false);
  };

  const moveSelection = (direction: 1 | -1) => {
    if (commands.length === 0) return;
    const currentIndex = commands.findIndex(
      (command) => command.id === activeId,
    );
    const nextIndex =
      currentIndex < 0
        ? 0
        : (currentIndex + direction + commands.length) % commands.length;
    setSelectedId(commands[nextIndex]?.id ?? null);
  };

  const onSearchKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      moveSelection(event.key === "ArrowDown" ? 1 : -1);
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    if (event.ctrlKey && context.selection) {
      onInspectSelection();
      onOpenChange(false);
      return;
    }
    execute(commands.find((command) => command.id === activeId));
  };

  return (
    <ModalOverlay
      className="command-overlay"
      isOpen={isOpen}
      onOpenChange={onOpenChange}
      isDismissable
    >
      <Modal className="command-modal">
        <Dialog className="command-dialog" aria-label="Command palette">
          <TextField className="command-search" aria-label="Search commands">
            <Search aria-hidden size={18} strokeWidth={1.8} />
            <Input
              autoFocus
              type="search"
              value={query}
              onChange={(event) => setQuery(event.currentTarget.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Search commands"
            />
            <kbd>Ctrl K</kbd>
          </TextField>

          <div className="command-results">
            {commands.length > 0 ? (
              <ListBox
                aria-label="Command results"
                selectionMode="single"
                selectionBehavior="replace"
                selectedKeys={activeId ? new Set([activeId]) : new Set()}
                onSelectionChange={(keys) => setSelectedId(selectedKey(keys))}
                onAction={(key) =>
                  execute(
                    commands.find((command) => command.id === String(key)),
                  )
                }
              >
                {groups.map((group) => (
                  <ListBoxSection key={group.label} className="command-group">
                    <Header className="command-group-label">
                      {group.label}
                    </Header>
                    {group.commands.map((command) => {
                      const Icon =
                        commandIcons[command.id as keyof typeof commandIcons];
                      return (
                        <ListBoxItem
                          id={command.id}
                          key={command.id}
                          className="command-row"
                          textValue={`${command.label} ${command.detail}`}
                          aria-label={`${command.label} — ${command.detail}`}
                        >
                          <span className="command-row-icon" aria-hidden>
                            {Icon ? <Icon size={16} strokeWidth={1.7} /> : null}
                          </span>
                          <span className="command-row-copy">
                            <strong>{command.label}</strong>
                            <span>{command.detail}</span>
                          </span>
                          <CornerDownLeft
                            className="command-enter-icon"
                            aria-hidden
                            size={14}
                            strokeWidth={1.8}
                          />
                        </ListBoxItem>
                      );
                    })}
                  </ListBoxSection>
                ))}
              </ListBox>
            ) : (
              <div className="command-empty" role="status">
                <strong>No matching commands</strong>
                <span>Try a view name, refresh, or inspect.</span>
              </div>
            )}
          </div>

          <footer className="command-footer">
            <span>
              <kbd>↑↓</kbd> Navigate
            </span>
            <span>
              <kbd>Enter</kbd> Run
            </span>
            {context.selection ? (
              <span>
                <kbd>Ctrl Enter</kbd> Inspect
              </span>
            ) : null}
            <span>
              <kbd>Esc</kbd> Close
            </span>
          </footer>
        </Dialog>
      </Modal>
    </ModalOverlay>
  );
}

interface ActionBarProps {
  viewLabel: string;
  selectionLabel?: string;
  selectionKind?: string;
  onOpenCommand: () => void;
  onCloseInspector: () => void;
}

export function ActionBar({
  viewLabel,
  selectionLabel,
  selectionKind,
  onOpenCommand,
  onCloseInspector,
}: ActionBarProps) {
  return (
    <div className="actionbar" role="toolbar" aria-label="Contextual actions">
      <div className="actionbar-location">
        <span>{selectionKind ?? "View"}</span>
        <strong>{selectionLabel ?? viewLabel}</strong>
      </div>
      <div className="actionbar-actions">
        {selectionLabel ? (
          <Button aria-label="Dismiss inspector" onPress={onCloseInspector}>
            <span>Close inspector</span>
            <kbd>Esc</kbd>
          </Button>
        ) : null}
        <Button aria-label="Open commands" onPress={onOpenCommand}>
          <span>Commands</span>
          <kbd>Ctrl K</kbd>
        </Button>
      </div>
    </div>
  );
}
