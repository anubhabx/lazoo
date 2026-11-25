// The node canvas: React Flow kept controlled by the workflow in controller state.
// Positions move locally while dragging and are saved once, on drop.

import {
  Background,
  BackgroundVariant,
  ConnectionLineType,
  Controls,
  ReactFlow,
  applyEdgeChanges,
  applyNodeChanges,
  useReactFlow,
  type Connection,
  type Edge,
  type EdgeChange,
  type NodeChange,
  type OnConnectEnd,
} from "@xyflow/react";
import { Plus } from "lucide-react";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";

import { newKey } from "../../lib/util";
import {
  NODE_CATALOG,
  type Handle,
  type NodeKind,
  type Workflow,
  type WorkflowRun,
} from "../../lib/workflows/types";
import type { WorkflowIssue } from "../../lib/workflows/graph";
import { Button } from "../primitives";
import {
  NODE_GAP,
  NODE_WIDTH,
  addNode,
  canConnect,
  connect,
  freeHandle,
  nodeSummary,
  removeNodes,
  snap,
  type SummaryContext,
} from "./model";
import { NodePicker } from "./NodePicker";
import { WorkflowStepNode, type StepNode } from "./WorkflowNode";

const nodeTypes = { step: WorkflowStepNode };
const proOptions = { hideAttribution: true };
// Auto-fit never shrinks nodes below a readable size; a long chain is panned instead.
const fitViewOptions = { padding: 0.15, minZoom: 0.8, maxZoom: 1 };
const defaultEdgeOptions = { type: "default" } as const;
const snapGrid: [number, number] = [8, 8];

interface PickerState {
  /** Pixels inside the canvas. */
  at: { x: number; y: number } | null;
  from: { nodeId: string; handle: Handle } | null;
  /** Where the new node goes, in flow coordinates. */
  position: { x: number; y: number } | null;
}

export function WorkflowCanvas({
  workflow,
  issues,
  run,
  selectedIds,
  onSelect,
  commit,
  context,
}: {
  workflow: Workflow;
  issues: WorkflowIssue[];
  /** The active run, else the latest one: it tints the nodes. */
  run: WorkflowRun | null;
  selectedIds: string[];
  onSelect: (ids: string[]) => void;
  commit: (change: (current: Workflow) => Workflow) => void;
  context: SummaryContext;
}) {
  const flow = useReactFlow();
  const frame = useRef<HTMLDivElement>(null);
  const [picker, setPicker] = useState<PickerState | null>(null);

  const derived = useMemo<StepNode[]>(() => {
    const selected = new Set(selectedIds);
    return workflow.nodes.map((node) => {
      const summary = nodeSummary(node, context);
      const employee =
        node.kind === "agent.prompt"
          ? context.employees?.find(
              (entry) =>
                entry.id ===
                (node.config as { employeeId: string | null }).employeeId,
            )
          : undefined;
      return {
        id: node.id,
        type: "step" as const,
        position: node.position,
        selected: selected.has(node.id),
        ariaLabel: `${node.label}, ${NODE_CATALOG[node.kind].title}`,
        width: NODE_WIDTH,
        data: {
          kind: node.kind,
          label: node.label,
          headline: summary.headline,
          detail: summary.detail,
          muted: summary.muted,
          agent: employee
            ? { name: employee.name, provider: employee.provider }
            : null,
          issues: issues
            .filter((issue) => issue.nodeId === node.id)
            .map((issue) => issue.message),
          status:
            run?.steps.find((step) => step.nodeId === node.id)?.status ?? null,
        },
      };
    });
  }, [workflow.nodes, context, issues, run, selectedIds]);

  const [nodes, setNodes] = useState<StepNode[]>(derived);
  // Re-derive when the workflow changes, but keep what React Flow measured and
  // never move a node out from under the pointer.
  useEffect(() => {
    setNodes((previous) =>
      derived.map((node) => {
        const before = previous.find((entry) => entry.id === node.id);
        if (!before) return node;
        return {
          ...node,
          measured: before.measured,
          dragging: before.dragging,
          position: before.dragging ? before.position : node.position,
        };
      }),
    );
  }, [derived]);

  // Keep the graph in view. When a panel opens or closes the canvas resizes: refit if
  // any node fell outside, but once the user has panned or zoomed themselves only bring
  // the selected node back. Never while a node is being dragged.
  const userMoved = useRef(false);
  const only = selectedIds.length === 1 ? selectedIds[0] : null;
  const onlyRef = useRef(only);
  onlyRef.current = only;
  const ensureVisible = useCallback(() => {
    const box = frame.current?.getBoundingClientRect();
    if (!box || box.width === 0) return;
    if (flow.getNodes().some((node) => node.dragging)) return;
    const { x, y, zoom } = flow.getViewport();
    const outside = (node: {
      position: { x: number; y: number };
      measured?: { width?: number; height?: number };
    }) => {
      const width = (node.measured?.width ?? NODE_WIDTH) * zoom;
      const height = (node.measured?.height ?? 80) * zoom;
      const left = node.position.x * zoom + x;
      const top = node.position.y * zoom + y;
      return (
        left < 4 ||
        top < 4 ||
        left + width > box.width - 4 ||
        top + height > box.height - 4
      );
    };
    const nodes = flow.getNodes();
    if (!userMoved.current) {
      if (nodes.some(outside)) {
        void flow.fitView({ ...fitViewOptions, duration: 0 });
      }
      return;
    }
    const selected = nodes.find((node) => node.id === onlyRef.current);
    if (selected && outside(selected)) {
      void flow.setCenter(
        selected.position.x + NODE_WIDTH / 2,
        selected.position.y + 40,
        { zoom, duration: 0 },
      );
    }
  }, [flow]);
  useEffect(() => {
    const element = frame.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const observer = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(ensureVisible, 60);
    });
    observer.observe(element);
    return () => {
      clearTimeout(timer);
      observer.disconnect();
    };
  }, [ensureVisible]);
  useEffect(() => {
    if (!only) return;
    const timer = setTimeout(ensureVisible, 80);
    return () => clearTimeout(timer);
  }, [only, ensureVisible]);

  const derivedEdges = useMemo<Edge[]>(() => {
    const active = run?.status === "running" ? run : null;
    return workflow.edges.map((edge) => {
      const target = active?.steps.find((step) => step.nodeId === edge.target);
      return {
        id: edge.id,
        source: edge.source,
        sourceHandle: edge.sourceHandle,
        target: edge.target,
        targetHandle: "in",
        animated: target?.status === "running",
      };
    });
  }, [workflow.edges, run]);
  const [edges, setEdges] = useState<Edge[]>(derivedEdges);
  useEffect(() => {
    setEdges((previous) =>
      derivedEdges.map((edge) => ({
        ...edge,
        selected: previous.find((entry) => entry.id === edge.id)?.selected,
      })),
    );
  }, [derivedEdges]);

  const onNodesChange = useCallback(
    (changes: NodeChange<StepNode>[]) => {
      setNodes((current) =>
        applyNodeChanges(
          // Deleting goes through onNodesDelete, so the workflow stays the source of truth.
          changes.filter((change) => change.type !== "remove"),
          current,
        ),
      );
      const picks = changes.filter((change) => change.type === "select");
      if (picks.length) {
        const next = new Set(selectedIds);
        for (const change of picks) {
          if (change.selected) next.add(change.id);
          else next.delete(change.id);
        }
        onSelect([...next]);
      }
    },
    [onSelect, selectedIds],
  );

  const onEdgesChange = useCallback((changes: EdgeChange[]) => {
    setEdges((current) =>
      applyEdgeChanges(
        changes.filter((change) => change.type !== "remove"),
        current,
      ),
    );
  }, []);

  const workflowRef = useRef(workflow);
  workflowRef.current = workflow;
  const isValidConnection = useCallback(
    (connection: Connection | Edge) =>
      canConnect(
        workflowRef.current,
        connection.source,
        (connection.sourceHandle ?? "out") as Handle,
        connection.target,
      ),
    [],
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      commit((current) =>
        connect(
          current,
          connection.source,
          (connection.sourceHandle ?? "out") as Handle,
          connection.target,
        ),
      );
    },
    [commit],
  );

  const onConnectEnd: OnConnectEnd = useCallback(
    (event, state) => {
      if (state.isValid || state.toNode || !state.fromNode) return;
      if (state.fromHandle?.type !== "source") return;
      const point =
        "changedTouches" in event
          ? event.changedTouches[0]!
          : (event as MouseEvent);
      const bounds = frame.current?.getBoundingClientRect();
      if (!bounds) return;
      setPicker({
        at: { x: point.clientX - bounds.left, y: point.clientY - bounds.top },
        from: {
          nodeId: state.fromNode.id,
          handle: (state.fromHandle.id ?? "out") as Handle,
        },
        position: flow.screenToFlowPosition({
          x: point.clientX,
          y: point.clientY,
        }),
      });
    },
    [flow],
  );

  const openFromButton = useCallback(() => {
    const bounds = frame.current?.getBoundingClientRect();
    const selected =
      selectedIds.length === 1
        ? workflow.nodes.find((node) => node.id === selectedIds[0])
        : undefined;
    const handle = selected ? freeHandle(workflow, selected) : null;
    let position: { x: number; y: number };
    if (selected && handle) {
      position = {
        x: selected.position.x + NODE_GAP,
        y:
          selected.position.y +
          (handle === "false" ? 70 : handle === "true" ? -70 : 0),
      };
    } else if (bounds) {
      const centre = flow.screenToFlowPosition({
        x: bounds.left + bounds.width / 2,
        y: bounds.top + bounds.height / 2,
      });
      position = { x: centre.x - NODE_WIDTH / 2, y: centre.y - 40 };
    } else {
      position = { x: 0, y: 0 };
    }
    setPicker({
      at: null,
      from: selected && handle ? { nodeId: selected.id, handle } : null,
      position,
    });
  }, [flow, selectedIds, workflow]);

  function pick(kind: NodeKind) {
    const state = picker;
    setPicker(null);
    if (!state?.position) return;
    const id = newKey();
    commit((current) =>
      addNode(current, id, kind, state.position!, state.from ?? undefined),
    );
    onSelect([id]);
  }

  const pickerStyle = useMemo<CSSProperties>(() => {
    if (!picker?.at) return { left: 12, top: 48 };
    const bounds = frame.current?.getBoundingClientRect();
    const width = 296;
    const x = Math.max(
      8,
      Math.min(picker.at.x, (bounds?.width ?? 800) - width - 8),
    );
    const y = Math.max(8, Math.min(picker.at.y, (bounds?.height ?? 600) - 360));
    return { left: x, top: y };
  }, [picker]);

  return (
    <div className="wf-canvas" ref={frame}>
      <ReactFlow<StepNode>
        className="lz-flow"
        colorMode="dark"
        nodes={nodes}
        edges={edges}
        nodeTypes={nodeTypes}
        proOptions={proOptions}
        defaultEdgeOptions={defaultEdgeOptions}
        connectionLineType={ConnectionLineType.Bezier}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onConnectEnd={onConnectEnd}
        isValidConnection={isValidConnection}
        onNodeDragStop={(_event, _node, dragged) => {
          const positions = new Map(
            dragged.map((node) => [
              node.id,
              { x: snap(node.position.x), y: snap(node.position.y) },
            ]),
          );
          commit((current) => ({
            ...current,
            nodes: current.nodes.map((node) =>
              positions.has(node.id)
                ? { ...node, position: positions.get(node.id)! }
                : node,
            ),
          }));
        }}
        onNodesDelete={(deleted) => {
          const ids = new Set(deleted.map((node) => node.id));
          commit((current) => removeNodes(current, ids));
          onSelect(selectedIds.filter((id) => !ids.has(id)));
        }}
        onEdgesDelete={(deleted) => {
          const ids = new Set(deleted.map((edge) => edge.id));
          commit((current) => ({
            ...current,
            edges: current.edges.filter((edge) => !ids.has(edge.id)),
          }));
        }}
        onMoveStart={(event) => {
          if (event) userMoved.current = true;
        }}
        onPaneClick={() => setPicker(null)}
        snapToGrid
        snapGrid={snapGrid}
        fitView
        fitViewOptions={fitViewOptions}
        minZoom={0.3}
        maxZoom={1.6}
        panOnScroll
        zoomOnScroll={false}
        zoomOnDoubleClick={false}
        deleteKeyCode={["Backspace", "Delete"]}
        nodesConnectable
        aria-label="Workflow canvas"
      >
        <Background
          variant={BackgroundVariant.Dots}
          gap={20}
          size={1}
          color="var(--wf-grid-dot)"
        />
        <Controls
          showInteractive={false}
          position="bottom-left"
          fitViewOptions={fitViewOptions}
        />
      </ReactFlow>
      <div className="wf-canvas__tools" data-picker-anchor>
        <Button
          size="sm"
          aria-haspopup="menu"
          aria-expanded={picker !== null && picker.at === null}
          onClick={() => (picker ? setPicker(null) : openFromButton())}
        >
          <Plus size={13} aria-hidden="true" />
          Add step
        </Button>
      </div>
      {picker ? (
        <NodePicker
          style={pickerStyle}
          triggers={!picker.from}
          onPick={pick}
          onClose={() => setPicker(null)}
        />
      ) : null}
    </div>
  );
}
