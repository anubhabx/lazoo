import { useAppState } from "../lib/context";

export function StatusBar() {
  const state = useAppState();
  const space =
    state.projects.find((project) => project.id === state.spaceId)
      ?.displayName ?? "Home";
  const conversation = state.conversation;
  const workers = conversation?.workers ?? [];
  const running = workers.filter((worker) => worker.state === "running").length;
  const needsYou =
    conversation?.attention.filter(
      (item) => item.attention === "action-required",
    ).length ?? 0;
  const connection =
    state.connection === "connected"
      ? "connected"
      : state.connection === "connecting"
        ? "connecting"
        : state.connection === "uncertain"
          ? "reconnecting"
          : "daemon offline";

  return (
    <footer className="statusbar" aria-label="Status">
      <span>
        <strong>{space}</strong>
      </span>
      {conversation ? (
        <span>
          {workers.length} {workers.length === 1 ? "worker" : "workers"}
          {running ? ` · ${running} running` : ""}
          {needsYou ? ` · ${needsYou} needs you` : ""}
        </span>
      ) : state.inboxCounts ? (
        <span>
          {state.inboxCounts.unresolved
            ? `${state.inboxCounts.unresolved} in inbox`
            : "inbox clear"}
        </span>
      ) : null}
      <span className="statusbar__right">
        <span>{connection}</span>
        {state.status ? (
          <>
            <span>{`schema ${state.status.schemaVersion}`}</span>
            <span>{`lazood ${state.status.daemonVersion}`}</span>
          </>
        ) : null}
      </span>
    </footer>
  );
}
