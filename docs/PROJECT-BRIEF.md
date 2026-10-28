# Lazoo Project Brief

- **Status:** Owner-reviewed product and engineering contract
- **Research snapshot:** 2026-08-28
- **Owner review:** 2026-08-28
- **Provider scope update:** 2026-10-16: Grok Build is excluded from core v1 acceptance. Claude's restored native subscription is an accepted live-verification path; a configured external endpoint remains an alternative. Earlier provider research and results remain historical evidence.
- **Product boundary:** Windows-first, single-user, local desktop software
- **Repository stage:** Control-plane foundation, redesigned read-only Tauri desktop shell, backend native-provider adapters, durable employees, native-session Job Object ownership, collaboration-message and conversation/turn ledgers, schema-7 automatic Codex task dispatch with retained isolated workspaces, and the schema-8 approval gate for Codex command/file-change requests implemented with deterministic acceptance; ConPTY, per-capability policy, other-provider task delivery and approval bridging, and resource-governor slices remain

## 1. Executive summary

Lazoo is a personal desktop control plane for a team of named AI coding employees. Each employee runs inside its own native provider harness—Codex, Claude Code, OpenCode, Grok Build, or Antigravity—using that harness's own models, tools, plugins, authentication, terminal behavior, and session semantics.

The user chooses a lead employee and starts a normal conversation. The lead can reason about the request, inspect the live roster and employee capabilities, propose or perform delegation according to configured authority, and continue its own work while delegated employees run. Employees may message peers directly for questions and clarifications. A durable Lazoo message bus makes that collaboration work even when the providers do not share a native cross-session protocol.

Lazoo is not another coding agent and should not impersonate one. It is a **harness for native harnesses**:

- It starts an employee only when work or a message requires that employee.
- It gives each active employee an independent native process, terminal, session, and project context.
- It creates an isolated Git worktree by default for every assignment that can edit files.
- It maintains a provider-neutral task, message, approval, incident, and memory ledger.
- It keeps provider-specific behavior behind capability-aware adapters rather than reducing every provider to a lowest common denominator.
- It provides a native Windows desktop application, a persistent local daemon, and a companion CLI.
- It enforces deterministic safety and reliability outside the models, including resource limits, delivery retries, incident thresholds, and bounded remediation.

The product is intentionally personal. It does not need multi-user tenancy, cloud hosting, collaboration accounts, billing, marketplace packaging, or an extensibility story designed for strangers.

## 2. Product decision

The recommended design is:

- A **greenfield Lazoo implementation**. Relaydeck remains the closest benchmark and must be reassessed at stage boundaries, but Lazoo is not implemented as a Relaydeck extension.
- **Rust** for the daemon, process supervision, Windows integration, provider adapters, storage, and CLI.
- **Tauri 2** for the desktop shell, backed by the system WebView2 runtime.
- **SQLite in WAL mode** for durable local state, with an append-only audit/event model and transactional outbox.
- **Windows named pipes** for local IPC, restricted to the current user's security identifier.
- **ConPTY** for native interactive terminal sessions.
- **Windows Job Objects** for process-tree ownership, accounting, termination, and resource governance.
- **Git worktrees** as the default editing isolation boundary.
- A **canonical Lazoo message bus** for all cross-provider communication. Provider-native messaging may be used as an optimization, never as the only source of truth.

This choice keeps the always-on control plane small and native while allowing a rich desktop interface. Tauri's security model also supports keeping privileged capabilities in Rust rather than exposing them to the webview. Its Windows distribution supports WebView2-based installers, sidecars, capabilities, tray behavior, and signed updates through documented mechanisms.

## 3. Product principles

### 3.1 Preserve native capability

Lazoo must expose employees to their provider's native session, tools, MCP servers, plugins, skills, permission prompts, and terminal behavior wherever the provider supports them. A feature that only one provider supports remains available to employees on that provider and appears as a capability in the roster.

### 3.2 Capability truth over optimistic routing

The roster is live operational data, not a static prompt. A delegating employee sees whether another employee is enabled, installed, authenticated, compatible, busy, rate-limited, failed, or offline. Lazoo must not advertise a capability that has not been discovered or configured.

### 3.3 Lazy activation

Dormant employees consume no provider process. Lazoo activates an employee only for a user conversation, delegated task, direct peer message, explicit warm-up, or configured scheduled action. Idle employees suspend after a configurable timeout.

### 3.4 Deterministic control around probabilistic workers

Models may decide how to solve work, but they do not own delivery guarantees, authority enforcement, retries, resource limits, incident thresholds, audit retention, or credentials. These are daemon responsibilities.

### 3.5 Local and private by default

The control plane has no network listener by default, no product telemetry by default, and no Lazoo account. Provider traffic follows the provider's own behavior. Any external memory summarization or voice provider is explicit, visible, and independently configurable.

### 3.6 Recoverable operations

Lazoo favors reversible configuration changes, retained worktrees, bounded retries, backups before provider-config writes, and explicit reconciliation. It does not automatically erase dirty work, rewrite provider authentication, or hide a failed delivery.

### 3.7 The user remains the owner

Authority is configurable, but models cannot grant themselves additional authority. The user can pause the zoo, disable any provider or employee, inspect why a decision was made, interrupt sessions, and take over a native terminal.

## 4. Goals and non-goals

### 4.1 Core goals

1. Coordinate named employees across the five requested provider families.
2. Allow the user to select any configured employee as the lead.
3. Support lead-to-employee delegation and direct employee-to-employee messaging.
4. Keep employees dormant until called.
5. Provide every employee with an independent native environment and visible terminal/session.
6. Isolate editing assignments using Git worktrees by default.
7. Support live add, remove, enable, disable, model, role, tool, and policy changes.
8. Prevent delegation to an unavailable employee using live capability and health data.
9. Provide clean incident reporting, persistent-failure detection, bounded lead remediation, and priority user notification.
10. Provide project memory without injecting full transcripts into every session.
11. Federate MCP, plugin, skill, and connector configuration without pretending unsupported features exist.
12. Stay efficient on the owner's current laptop and cap Lazoo's own aggregate CPU use at 20 percent of the machine.
13. Add an integrated and standalone voice experience after the core platform is stable.

### 4.2 Explicit non-goals

- Reimplementing Codex, Claude Code, OpenCode, Grok Build, or Antigravity.
- Building a universal prompt format that discards provider-native features.
- Supporting multiple users, remote teams, hosted execution, billing, or public distribution in the initial product.
- Making macOS or Linux first-class during the Windows phase.
- Treating a model as the reliability supervisor.
- Automatically merging unverified code into the main checkout.
- Sharing provider credentials between harnesses.
- Requiring a local embedding model.
- Using full transcripts as default prompt context.
- Shipping voice before the orchestration core is dependable.

## 5. Baseline environment

The owner's laptop is the acceptance baseline:

| Area | Observed baseline |
|---|---|
| Operating system | Windows 11 Home Single Language, build 26200 |
| CPU | Intel Core i5-11260H, 6 cores / 12 logical processors |
| Memory | Approximately 15.7 GiB |
| GPUs | Intel UHD plus NVIDIA RTX 3050 Laptop GPU with 4 GiB VRAM |
| Primary disk | Approximately 473.8 GiB total, 30.4 GiB free at inventory time |

The low available disk space makes retention controls, compressed transcripts, worktree visibility, and safe cleanup especially important. Optional embedding models and transcript archives are not included in the core storage target and must show their own projected and actual disk use before download or retention changes.

## 6. Provider research and adapter strategy

The following is a local inventory combined with current official interface research. Versions and availability are a snapshot, not hard-coded product assumptions.

| Provider | Local state | Useful native integration surface | Native peer messaging | Lazoo strategy | Verification state |
|---|---|---|---|---|---|
| Codex | Codex CLI 0.144.1; desktop/native binaries and local config detected | App Server JSON-RPC over stdio, thread and turn lifecycle, streaming notifications, dynamic tools/MCP; non-interactive JSON execution as fallback | Codex has multi-agent child workflows, but a generic durable top-level cross-session bus remains an open product request | Prefer App Server per employee; generate protocol schemas from the installed version; expose Lazoo collaboration tools through the supported tool surface | Locally inspected and command help verified |
| Claude Code | Native executable 2.1.241 detected; local settings detected | Native Claude Code sessions and documented cross-session messaging; agent teams offer independent sessions and shared coordination | Yes, through documented cross-session messaging and experimental agent teams on supported versions | Preserve Claude-native messaging where useful, mirror every envelope into Lazoo's bus, and fall back to the canonical bus | Installation inspected only; live acceptance blocked by expired subscription |
| OpenCode | Version 1.18.23 and configuration detected | Local server and SDK, Server-Sent Events, sessions, asynchronous prompts, abort/status APIs, and ACP | No stable provider-neutral peer bus is documented in the researched server/SDK contract | Run a controlled native server/session boundary per employee configuration; consume SSE; use Lazoo tools for peer messaging | Locally inspected and command help verified |
| Grok Build CLI | Version 1.0.5 detected with local config | ACP-compatible agent stdio, headless and server modes, streaming JSON, resumable sessions | No durable cross-provider peer bus in ACP | Prefer ACP stdio for lifecycle, prompts, streaming, permission requests, and terminal updates; retain provider-native memory as an optional employee capability | Locally inspected and command help verified |
| Antigravity / agy | Antigravity desktop and IDE detected; agy CLI 1.1.22 installed under the user's local application data | Local help verifies streaming JSON, long-lived stream-JSON input, conversation continuation, model/effort/agent/mode/permission/sandbox controls, MCP, and plugins | No provider-neutral durable peer bus is documented | Use a long-lived stream-JSON process, persist conversation IDs, and probe the installed CLI rather than relying only on the parent process's PATH snapshot | Version and command surface locally verified; model-backed session not exercised |

### 6.1 Central finding: Lazoo must own collaboration

The providers expose different notions of sessions and collaboration. Claude Code has the strongest documented native cross-session messaging. Codex exposes a rich application server and multi-agent capabilities but does not currently provide a stable generic bus for independent top-level sessions. OpenCode, Grok Build, Antigravity, and ACP expose session control and streams, not a complete durable team communication contract.

Therefore Lazoo must provide the canonical task and message bus. Provider-native messaging is an optional fast path only when:

1. the message is first durably recorded by Lazoo;
2. the native delivery carries a Lazoo correlation and idempotency key;
3. acknowledgements and replies return to the Lazoo ledger; and
4. native failure falls back to ordinary Lazoo delivery.

This prevents split-brain history and makes cross-provider communication behave consistently.

### 6.2 Adapter contract

Every provider adapter implements a versioned contract with capability negotiation:

| Contract area | Required behavior |
|---|---|
| Discovery | Locate supported executables and configuration without assuming PATH is complete |
| Probe | Return installed version, authentication readiness when safely detectable, protocol compatibility, and degradation reason |
| Capabilities | Publish session, stream, cancel, resume, terminal, permission, MCP, plugin, skill, connector, native-message, and model-selection capabilities |
| Lifecycle | Start, attach, resume, interrupt, stop, and report exit with a stable employee/session identity |
| Turn transport | Send user, delegated, peer, and system context through the provider's supported interface |
| Event normalization | Convert native stream events into Lazoo turn, tool, terminal, permission, usage, completion, and error events while retaining an optional raw payload reference |
| Tool bridge | Register Lazoo collaboration tools only through mechanisms the provider supports |
| Configuration | Import, render, validate, back up, apply, and roll back only the provider-specific areas Lazoo is allowed to manage |
| Health | Emit heartbeats, progress evidence, rate-limit state, authentication state, and last successful interaction |
| Shutdown | Request graceful termination, preserve resumable state where available, then terminate the owned process tree after a bounded timeout |

Unknown or unsupported capabilities remain false. Adapters must not infer support from a provider name alone.

### 6.3 Provider-specific notes

**Codex.** App Server is the preferred surface because it exposes versioned JSON-RPC threads and turns, streaming notifications, interrupts, and tool integration. Lazoo should generate or vendor schemas from each supported installed Codex version and explicitly handle documented overloaded queues. The non-interactive JSON CLI remains a fallback and diagnostic path, not the primary interactive employee experience.

**Claude Code.** The installed binary version is new enough for the documented Windows cross-session facility by version comparison, but this has not been exercised because the subscription is expired. The adapter remains formally unverified until authentication, native session startup, permission prompts, cross-session delivery, interruption, and recovery are tested live. Native messages still cannot grant user approval, matching Lazoo's own invariant.

**OpenCode.** The server/SDK/SSE model is suitable for a native adapter, but startup and asynchronous prompt failures require defensive reconciliation. A reported failure mode can return a nominal asynchronous response after an underlying startup error and leave a frozen child session. Lazoo must require progress evidence and health deadlines rather than equating HTTP acceptance with a healthy task.

**Grok Build.** ACP stdio is the cleanest contract for session creation, prompt streaming, tool updates, permission requests, terminal events, and cancellation. Grok's native memory can remain enabled as an employee-specific feature, but it is separate from shared Lazoo project memory.

**Antigravity.** agy 1.1.22 is installed and its local command surface confirms the documented long-lived stream-JSON interface. Lazoo must not automate the desktop GUI as its normal adapter. Conversation identifiers should be persisted for resume. The installer added agy's directory to the persisted user PATH, but the already-running Codex desktop process retained its older environment; adapter discovery must inspect persisted Windows environment data and known installation locations rather than trusting its inherited PATH alone.

## 7. Existing landscape and greenfield rationale

Lazoo overlaps with existing local orchestration products and should be honest about that.

| Project | Relevant overlap | Why it does not fully define Lazoo |
|---|---|---|
| Relaydeck | Local daemon/dashboard, agent CLI terminals, peer messaging, skills, worktrees, multiple harnesses | Very close conceptually and the most important benchmark. Lazoo's differentiators must be native protocol adapters, Windows-first deterministic supervision, capability-aware live configuration, provider-config federation, configurable authority, and the owner's memory/voice requirements |
| Muster desktop | Native desktop cockpit for many agent CLIs, daemon, terminals, and worktrees | Useful UX and provider-coverage benchmark; Lazoo remains a personal, policy-driven team runtime rather than a general commercial cockpit |
| Asterism | Local-first multi-agent runtime with per-agent memory, secrets, and autonomy | Useful architecture comparison; Lazoo emphasizes native external harnesses and Windows process/session fidelity |
| Muster mission control | Shared task board and MCP-oriented coordination | Useful task-state comparison; it does not remove the need for Lazoo's native employee process and failure model |
| Claude Code agent teams | Independent Claude sessions, shared tasks, peer communication | Strong same-provider precedent, but experimental and limited to Claude Code |

Building greenfield is justified only if Lazoo preserves these specific priorities:

- native protocols instead of screen scraping or terminal-output heuristics where a protocol exists;
- a canonical cross-provider employee bus;
- Windows-native process, terminal, ACL, watchdog, and resource behavior;
- worktree-by-default editing isolation;
- authority policy independent of model prompts;
- live roster and capability reconciliation;
- project memory with transcript pointers rather than transcript injection;
- provider-native config, plugin, MCP, and connector federation;
- a personal voice workflow.

If Lazoo stops pursuing those differences, adopting or extending Relaydeck or a commercial cockpit would be cheaper and should be reconsidered before substantial implementation.

## 8. System architecture

### 8.1 Components

**Lazoo Desktop (Lazoo.exe)**

A Tauri 2 application that provides conversations, team roster, task graph, employee terminals, approvals, incidents, memory, integrations, resource use, and settings. Closing the window does not implicitly terminate active work. The tray shows current activity and priority incidents.

**Lazoo daemon (lazood.exe)**

The authoritative local control plane. It owns storage, policy, employee lifecycle, provider adapters, worktrees, the message bus, incidents, memory, configuration reconciliation, and resource governance.

**Lazoo CLI (lazooctl.exe, invoked as lazooctl)**

A first-class companion for status, conversations, tasks, employees, approvals, incidents, terminal attachment, configuration validation, diagnostics, and daemon control. It uses the same daemon contract as the desktop app.

**Lazoo keeper (lazoo-keeper.exe)**

A deliberately tiny per-user supervisor for the daemon. It has no model or provider logic. It detects daemon exit, applies a restart budget, writes an emergency local incident record, and raises a Windows notification when the primary incident path is unavailable. This closes the failsafe gap where the component responsible for reporting failures is itself down.

**Provider employee processes**

Independent native harness processes or provider-supported session servers, one isolated employee instance per active identity as required by the adapter. Interactive processes use ConPTY. Each employee has its own environment block, working directory, session identifiers, logs, and process job.

### 8.2 Logical flow

    User
      |
      +---- Lazoo Desktop
      +---- Lazoo CLI
                 |
          current-user named pipe
                 |
             lazood.exe
      +----------+-----------+-----------+-----------+
      |          |           |           |           |
    Policy     Task and     Employee    Incident    Memory and
    engine     message bus  supervisor  watchdog    config stores
      |          |           |           |           |
      +----------+----- Provider adapter registry ---+
                         |       |       |       |
                       Codex   Claude  OpenCode  Grok  agy
                         |       |       |       |     |
                    native session/process + ConPTY + worktree

### 8.3 IPC

Desktop and CLI control requests use a versioned JSON-RPC contract over a Windows named pipe. Terminal traffic uses a separate framed binary stream so high-volume PTY output cannot starve approvals, cancellation, heartbeats, or incidents.

The pipe is created with an explicit discretionary access control list for the current user's SID. Lazoo must not rely on Windows' default named-pipe ACL because Microsoft documents broader default read access. There is no TCP listener by default.

The client contract supports:

- command request/response;
- event subscriptions with monotonic sequence numbers;
- reconnect from last observed sequence;
- backpressure and bounded client queues;
- explicit protocol version negotiation;
- cancellation and timeouts;
- snapshot plus delta recovery after disconnection.

### 8.4 Storage

SQLite WAL is the primary store under the user's local application data, not inside the project repository. Each mutating operation writes domain state, an audit event, and any outgoing bus event transactionally. A single daemon writer avoids multi-process database contention.

Core records include:

- projects and project policies;
- providers, installations, capabilities, and health;
- employees, roles, configurations, and availability;
- native sessions and process identities;
- conversations, turns, tasks, dependencies, and results;
- messages, delivery attempts, acknowledgements, and replies;
- approvals and immutable decisions;
- incidents, occurrence counters, remediation attempts, and notifications;
- worktrees, branches, ownership, validation, and integration state;
- configuration sources, desired state, actual state, revisions, and backups;
- MCP servers, plugins, skills, and connector bindings;
- typed memories, source pointers, summarization jobs, and transcript archives;
- resource samples and performance-budget events;
- transactional outbox and append-only audit events.

Large terminal streams, compressed transcript archives, and raw provider payloads live in bounded content-addressed files with references in SQLite.

## 9. Employee model and lifecycle

An employee is a stable Lazoo identity layered over a provider configuration. “Ron” can remain Ron even if the selected Codex model or reasoning effort changes.

Each employee record contains:

- name and optional avatar;
- role and free-form operating instructions;
- provider, model, reasoning effort, and provider-native settings;
- enabled/disabled state;
- project access and default checkout policy;
- authority overrides;
- advertised skills, tools, MCP, plugins, and connector capabilities;
- concurrency limit, activation policy, idle timeout, and cost/rate constraints;
- memory read/write policy and injection budget;
- live health, availability, active assignments, and last capability refresh.

### 9.1 Lifecycle

| State | Meaning |
|---|---|
| Disabled | Explicitly unavailable; excluded from delegation |
| Offline | Enabled but missing, unauthenticated, incompatible, or manually marked unavailable |
| Dormant | Available and configured; no provider process exists |
| Starting | Native environment, worktree, and provider session are being prepared |
| Running | Actively processing a turn or tool action |
| Idle | Session remains attachable but is not doing work |
| Suspended | Process stopped after idle policy; resumable identifiers retained where supported |
| Failed | Session cannot safely continue without recovery or intervention |

Transitions are daemon-owned. A model can request activation, suspension, or cancellation but cannot directly rewrite state.

### 9.2 Activation triggers

- user starts or resumes a conversation with the employee;
- a task is delegated and passes policy;
- a direct peer message requires a response;
- the user explicitly starts or warms the employee;
- a future scheduled action is authorized.

Queueing a message does not require an already-running employee. The bus records the message, starts the recipient according to policy, injects only the relevant envelope and context references, and tracks acknowledgement.

### 9.3 Live roster propagation

Every capability or availability change increments a roster revision. Active leads receive a compact roster-change event. New delegations must cite the revision on which the choice was based; the daemon revalidates immediately before dispatch. If the target became unavailable, the task returns to the delegator with a structured routing failure instead of silently choosing another employee.

### 9.4 Live configuration changes

Employee and provider changes are validated and revisioned before becoming active. Disabling an employee removes it from new routing immediately; the user chooses whether existing work may finish, should suspend, or should cancel. Removing an employee retains its task, message, incident, audit, and memory history under a tombstoned identity.

Model, reasoning, tool, or integration changes apply at the earliest native-safe boundary. Lazoo never mutates a provider underneath an active turn. If a provider can update a setting in-session, the adapter applies and verifies it at the next turn; otherwise it suspends and resumes or starts a replacement session after warning about context consequences. Delegators receive the new roster revision only after actual state matches desired state or the employee is marked degraded.

## 10. Conversation, task, and peer flows

### 10.1 User-to-lead flow

1. The user selects a configured employee as lead and starts a conversation.
2. Lazoo activates or resumes that employee in the selected project.
3. The lead receives the request, project policy, its role, a compact live roster, relevant typed memories, and Lazoo collaboration tools.
4. The lead reasons about decomposition and selects employees based on capability, availability, role, current load, and policy.
5. In default ask mode, proposed delegations or governed actions become approval cards.
6. Approved tasks enter the durable task graph. Only assigned employees activate.
7. The lead continues independent work, waits on selected dependencies, or yields to the user.
8. Results return as structured task outcomes with links to session, worktree, verification, messages, and memories.

### 10.2 Peer clarification flow

1. A frontend employee encounters a design-system ambiguity.
2. It calls **message.send** to the UI/product employee, including the question, task reference, urgency, and the minimum relevant context.
3. The bus records the message and activates the recipient if necessary.
4. The recipient replies directly. The reply is linked to both tasks and becomes available to the sender.
5. The lead receives no interruption unless the question becomes blocked, exceeds its deadline, changes scope, affects integration, or matches a configured subscription.

QA can use the same path to report a minor API bug directly to the API employee. Direct messaging does not change task ownership, grant approval, or expand either employee's file or connector permissions.

### 10.3 Collaboration tools

Adapters expose the following tools using the provider's native tool mechanism or MCP where supported:

- **employees.list** — live roster, capability, availability, load, and revision;
- **task.delegate** — create a scoped task for a named employee;
- **task.wait** — wait for one or more task state changes without polling prompts;
- **task.report** — return structured outcome and verification evidence;
- **message.send** — send a correlated peer or lead message;
- **message.reply** — reply to a message thread;
- **incident.report** — record a human-readable operational problem;
- **incident.list** — read permitted active incidents and notification-stream updates;
- **memory.search** — retrieve compact indexed memories and source pointers;
- **memory.record** — propose a typed memory subject to policy.

These tools are versioned and capability-reported. Unsupported provider tool surfaces use a session-safe adapter injection path, not terminal keystroke simulation as a default.

## 11. Durable messaging semantics

### 11.1 Message envelope

Every message has:

- message and conversation identifiers;
- sender, recipient, project, task, and session identifiers;
- message type, priority, creation time, expiry, and correlation identifier;
- immutable text or structured payload;
- references to memories, files, worktrees, incidents, or approvals;
- roster and policy revisions;
- idempotency key;
- delivery state and attempt history.

Large context is referenced, not copied. The recipient explicitly reads a referenced transcript slice, file, or memory only when needed.

### 11.2 Delivery model

Delivery is **at least once**. Adapter delivery commands and recipient processing must be idempotent by message key. Lazoo does not claim exactly-once delivery across provider process crashes.

States are:

1. Queued
2. Activating
3. Delivered
4. Acknowledged
5. Replied
6. Expired
7. Dead-lettered

A task completion message is not considered delivered merely because bytes were written to stdin or an HTTP endpoint returned success. The adapter must observe a provider acknowledgement or session progress evidence. Missing evidence becomes a delivery incident.

### 11.3 Backpressure and fairness

Each employee and project has bounded queues. Interactive user messages and cancellation outrank background memory work. Peer questions outrank low-priority summaries but cannot starve user turns. Queue saturation produces a clean incident and a retry time instead of unbounded memory growth.

## 12. Authority and approval model

Authority is policy, not prompt text. The default is **ask**.

### 12.1 Modes

| Mode | Behavior |
|---|---|
| Ask | Employees may reason, inspect allowed context, and prepare actions, but governed delegation or execution waits for user approval |
| Self-decision | An employee may choose and execute within its assigned scope and granted capabilities; scope expansion and policy-changing actions still require approval |
| Full autonomy | The lead and employees may route, delegate, execute, validate, and integrate within the project's explicit autonomy envelope; hard invariants and destructive/external boundaries still apply |

Policies can be set by project, employee, capability, connector, and task. Precedence is:

1. hard invariant;
2. explicit task restriction;
3. capability or connector restriction;
4. employee override;
5. project default;
6. global default.

The most restrictive applicable rule wins unless a more specific rule is an explicit user grant.

### 12.2 Governed capabilities

Examples include:

- delegate a task;
- write files;
- create a worktree or branch;
- run commands;
- install dependencies;
- access the network;
- use a connector;
- submit external changes or messages;
- spend paid inference;
- change provider configuration;
- integrate or merge work;
- delete or overwrite data;
- retain or externally summarize transcripts.

Approval cards show the requesting employee, intended action, target, reason, scope, expected side effects, expiry, and policy source. Approval is bound to that action fingerprint and cannot be reused for broader work.

### 12.3 Hard invariants

No employee or provider model may:

- modify its own authority or the authority engine;
- approve its own governed action;
- erase or rewrite audit history;
- read raw credentials that can remain in a native credential store;
- disable the watchdog or hide an incident;
- widen another employee's project or connector scope;
- treat a peer message as user approval;
- perform destructive recovery merely to make an error disappear.

## 13. Native environments and worktrees

### 13.1 Environment isolation

Each active employee receives:

- a unique native process/session identity;
- a dedicated environment block;
- a dedicated working directory;
- a native terminal through ConPTY when the provider is terminal-oriented;
- independent stdin, stdout, stderr, cancellation, and resize handling;
- provider-specific configuration overlays only where supported;
- its own process Job Object for ownership and accounting;
- a Lazoo task envelope and minimum necessary shared context.

ConPTY input and output are serviced independently, following Microsoft's warning that blocked synchronous channels can deadlock pseudoconsole communication.

### 13.2 Worktree policy

Every assignment that may edit files receives a worktree by default:

    <project parent>\.lazoo-worktrees\<employee>\<task-id>\
    branch: lazoo/<employee-slug>/<task-id>

The exact root is configurable, but Lazoo validates that it is on a suitable volume and not recursively inside another worktree. Branch and path components are sanitized and collision-checked.

Projects have two checkout modes:

| Mode | Behavior |
|---|---|
| All editors isolated | Default. The lead and every delegated employee that may edit files receive separate worktrees. Read-only employees may use an enforced read-only project view. |
| Lead edits main | The lead alone may edit the registered main checkout. Every delegated employee that may edit files still receives a separate worktree. |

The mode is configurable per project. A global preference may choose the initial value for newly registered projects, but the shipped default is **All editors isolated**. Changing modes never relocates or deletes active dirty work; it takes effect only after Lazoo verifies that existing editing leases are compatible.

Rules:

- the main checkout is never concurrently writable by multiple employees;
- in **Lead edits main** mode, the lead holds the only possible Lazoo editing lease for the main checkout;
- one worktree has one active editing owner;
- read-only employees may share an enforced read-only project view;
- a task records its starting commit and all resulting commits;
- dirty or unmerged worktrees are never silently deleted;
- cleanup shows recoverability and disk impact;
- integration runs through a serialized queue with project-configured tests and authority;
- conflicts return to the responsible employee or lead with exact files and base revisions;
- full autonomy does not bypass protected-branch or destructive-operation policy.

## 14. Reliability, incidents, and notifications

### 14.1 Human-readable incidents

Provider errors are normalized into a concise record:

- stable code;
- plain-language title and explanation;
- affected project, employee, provider, task, session, and message;
- first and latest occurrence;
- count and threshold state;
- likely cause and evidence;
- safe remedies already attempted;
- next recommended action;
- priority and acknowledgement state;
- raw diagnostic attachment, hidden behind an explicit details action.

The desktop and CLI expose the incident stream directly. Employees may read incidents permitted by their project and task scope through **incident.list**; the lead also receives subscribed threshold events automatically. Incident visibility never includes unredacted secrets or unrelated project data.

Initial codes include:

| Code | Meaning |
|---|---|
| LZ-PROVIDER-AUTH | Provider authentication or entitlement is unavailable |
| LZ-PROVIDER-INCOMPATIBLE | Installed provider version or protocol is unsupported |
| LZ-PROCESS-EXIT | Employee process exited unexpectedly |
| LZ-SESSION-STALLED | Session accepted work but produced no required progress evidence |
| LZ-MESSAGE-TIMEOUT | Delivery or acknowledgement deadline expired |
| LZ-MESSAGE-DEADLETTER | Message exhausted safe delivery attempts |
| LZ-WORKTREE-CONFLICT | Worktree, branch, or integration ownership conflict |
| LZ-RATE-LIMIT | Provider or external API rate limit blocked work |
| LZ-CONFIG-INCOMPATIBLE | Desired integration cannot be represented safely for the provider |
| LZ-RESOURCE-BUDGET | Lazoo's own resource budget was exceeded |
| LZ-DAEMON-UNAVAILABLE | Desktop or keeper cannot reach the daemon |

### 14.2 Thresholds and escalation

Default persistent-failure thresholds are configurable but begin with:

- three equivalent failures within five minutes;
- two unexpected exits of the same employee process;
- one unrecoverable employee death while assigned work is active;
- missed heartbeat or progress evidence beyond the adapter's deadline;
- one dead-lettered priority message;
- one daemon crash or corrupted primary incident path.

When a threshold is crossed:

1. the incident is promoted and the lead receives a structured notification;
2. the lead may attempt at most two policy-approved remedies within five minutes;
3. deterministic retry budgets still apply regardless of the lead's request;
4. unresolved failure becomes a priority user notification;
5. affected work is paused or rerouted only if policy permits;
6. the system records what it did and stops wasting time on repeated speculative repair.

Lazoo never automatically resets authentication, performs repeated login attempts, changes authority, deletes work, or applies destructive recovery.

### 14.3 Dual-path failsafe

The normal notification stream uses SQLite and the daemon event bus. If that path is unhealthy, the keeper writes a small append-only emergency NDJSON record under local application data and raises a native Windows notification. The desktop and CLI import and reconcile these records after recovery.

The desktop independently detects pipe loss and shows a persistent daemon-disconnected state. The keeper has a bounded restart budget; restart loops escalate instead of running forever.

### 14.4 Restart recovery

On daemon restart, Lazoo:

1. validates the database and applies only known migrations;
2. reconciles owned processes and worktrees;
3. marks uncertain in-flight deliveries for idempotent reconciliation;
4. resumes or fails sessions according to adapter evidence;
5. replays the transactional outbox;
6. restores event subscriptions by sequence;
7. emits one recovery summary rather than a flood of duplicate incidents.

## 15. Shared project memory

Memory modes are composable. A project can use one, several, or none.

The default active memory experience is the managed, indexed typed ledger in Mode B. Its indexing and summarization work may run through a dedicated employee worker or an explicitly configured external API. Raw transcript injection is never the default.

Compressed source-transcript retention is enabled by default so indexed memories can be audited and corrected. It is bounded by visible retention and disk quotas and can be disabled globally or per project. Disabling source retention does not silently delete existing archives; Lazoo previews the effect and requires an explicit cleanup action.

### 15.1 Mode A: local transcript archive

- Stores full session and action history locally with compression and configurable retention.
- Has no external inference dependency.
- Is never injected wholesale into another session.
- Supports indexed metadata, full-text search, timestamp and event filtering, and precise transcript slices.
- Allows a memory result to point to a session, turn, message, task, command, file, or outcome without copying the entire source.
- Shows expected and actual disk use, with archives outside Git.

This is evidence storage, not the default context strategy.

### 15.2 Mode B: summarized typed ledger

This is the recommended active memory mode.

A configured employee/model or external API converts completed or checkpointed activity into typed records:

- decisions and rationale;
- actions and outcomes;
- achievements and verification;
- questions and unresolved answers;
- failures and remedies;
- constraints and user preferences;
- project facts and architecture contracts;
- task and session summaries;
- source pointers and confidence.

The initial external-provider set may include configured native employees, OpenRouter, NVIDIA NIM, and Gemini API. A provider is used only after its credentials, privacy policy, model, rate limit, quota, and project permission are configured. “Free” inference is treated as a quota/cost class, not as a reliability guarantee.

Summarization uses:

- a durable job queue;
- provider-specific token and request buckets;
- exponential backoff with jitter;
- batch checkpoints rather than per-token calls;
- redaction rules;
- content hashes for deduplication;
- typed validation before insertion;
- links to source evidence;
- visible failure and retry state.

### 15.3 Mode C: local vector retrieval

Local embeddings and a vector index are optional and second to the summarized ledger. They ship only if a benchmark on the baseline laptop shows a meaningful retrieval-quality improvement without violating CPU, memory, disk, startup, and latency budgets.

The experiment must compare:

- SQLite FTS5 plus structured filters;
- summarized typed records with recency and relationship scoring;
- hybrid lexical/vector retrieval;
- index build and incremental update cost;
- quality on real Lazoo session questions;
- model download size and steady-state resource use.

Lazoo does not download an embedding model silently. The user sees size, license, expected performance, and removal controls.

### 15.4 Retrieval and injection

The memory query engine first filters by project, branch, task lineage, employee, record type, and permissions. It then ranks by lexical relevance, explicit relationships, confidence, recency, and configured importance.

Each session has:

- maximum injected memory count;
- maximum token budget;
- allowed record types;
- transcript-slice policy;
- provider/model-specific formatting;
- optional preview and exclusion controls.

Injected records include short source citations. Employees retrieve deeper evidence on demand. Contradictory memories remain visible and are reconciled rather than silently overwritten.

## 16. MCP, plugins, skills, and configuration federation

Lazoo maintains **desired state** and **observed actual state**. Provider-native files and authentication remain authoritative for provider behavior.

### 16.1 Import

Import is read-only by default:

1. discover installations and supported config locations;
2. parse only known formats;
3. redact secrets;
4. hash and normalize integrations;
5. identify exact duplicates, compatible equivalents, conflicts, and unsupported entries;
6. present a migration preview;
7. let the user select what Lazoo should manage.

### 16.2 Apply

For supported targets Lazoo:

- renders provider-native configuration;
- validates against the detected provider version;
- shows a diff;
- creates a timestamped backup;
- applies atomically where possible;
- re-probes the provider;
- rolls back on validation failure;
- records the revision and actual state.

It does not force MCP, plugin, or skill concepts into a provider that does not support them. Compatibility is represented explicitly:

- Native
- Translated with documented limitations
- Lazoo-only orchestration tool
- Unsupported
- Requires provider restart
- Requires user OAuth or native setup

### 16.3 Duplicate management

Duplicates are compared by normalized endpoint/command, arguments, environment-key names, package identity, source, and content checksum. Secrets are never compared by logging their values. Conflicting same-name integrations require an explicit choice or provider-specific alias.

## 17. Connector management

Connectors remain provider-specific bindings. Lazoo is a manager and router, not a fake universal connector host.

A connector record describes:

- provider and employee bindings;
- native connector identifier;
- capabilities and scopes;
- authentication state reference;
- setup and revocation entry points;
- policy and approval requirements;
- health and last successful use;
- provider-specific limitations.

OAuth and consent occur through the provider's native supported flow whenever possible. Lazoo stores only the routing metadata and credential references it requires. Unsupported connectors are marked unsupported for that employee and are not advertised to delegators.

Cross-provider migration means “create the nearest supported native configuration after preview,” not “copy credentials.” Removal can detach a Lazoo binding without revoking provider credentials unless the user explicitly chooses native revocation.

## 18. Voice mode

Voice is a post-core quality-of-life phase with two surfaces:

1. an integrated mode for speaking with the selected lead employee; and
2. a standalone Lazoo Voice overlay for push-to-talk dictation and, when enabled, general Windows text insertion.

The lead does not need to respond immediately. Speech is transcribed into the same durable conversation turn as typed input. As the lead's text stream becomes stable, Lazoo segments it at sensible phrase boundaries and feeds it to TTS. Playback is interruptible, and the transcript remains the authoritative response.

Requirements:

- configurable global push-to-talk hotkey;
- clear recording and external-processing indicators;
- selectable STT and TTS providers, including future local options;
- partial transcription with user correction before send when configured;
- streamed TTS with backpressure and sentence-aware chunking;
- stop, interrupt, replay, and text-only fallback;
- active conversation and target-employee selection;
- audio not retained by default;
- explicit policy before audio or transcript leaves the machine;
- separate CPU and GPU accounting from employee provider processes, but included in Lazoo's own resource budget when Lazoo runs the voice engine.

Voice does not bypass approval cards or transform spoken confirmation into broad permission. Confirmation must bind to the visible action.

## 19. Security and privacy

The initial threat model is one trusted Windows user operating on a personal machine, with untrusted repository content, tool output, prompts, downloaded packages, and external services.

### 19.1 Local boundary

- No network listener by default.
- Named pipe restricted by explicit current-user ACL.
- Protocol clients authenticate by Windows identity and a per-installation local nonce.
- Terminal bytes and provider text are treated as untrusted display data.
- The webview cannot invoke arbitrary daemon commands; Tauri capabilities allow only explicit commands.
- Provider arguments and environment values are passed as structured process parameters, never through an interpolated shell command.

### 19.2 Secrets

Provider credentials stay in provider-native stores. Lazoo stores references and health metadata. Lazoo-owned secrets use Windows Credential Manager or DPAPI scoped to the current user and machine. Secrets are redacted from logs, incidents, memory summaries, exports, and model context.

External summarization and voice calls show which provider receives what data. Per-project policy can prohibit external processing.

### 19.3 Files and Git

- Paths are canonicalized before any worktree, backup, cleanup, or provider-config mutation.
- Repository output cannot select an arbitrary cleanup root.
- Symlink/reparse-point boundaries are checked for managed directories.
- Dirty work is retained unless the user explicitly authorizes deletion.
- Integration actions record commits, tests, approvals, and actor identity.

### 19.4 Updates and diagnostics

Any future updater must verify signed artifacts. Diagnostic bundles are previewed and redacted before export. Telemetry is off by default; local performance and failure metrics remain available to the user.

## 20. Performance and resource budget

“Lazoo itself” includes the desktop, daemon, keeper, CLI while active, memory/index workers, and Lazoo-owned voice processing. Native provider harnesses and their descendants are measured separately and are not counted against the 20 percent harness ceiling, though Lazoo still displays their use.

### 20.1 Targets on the baseline laptop

| State | Target |
|---|---|
| Daemon and desktop idle CPU | Under 1 percent sustained aggregate |
| Normal routing and UI activity | Under 5 percent sustained aggregate |
| Background-throttle threshold | Approximately 15 percent aggregate |
| Lazoo-owned hard ceiling | 20 percent aggregate over a rolling measurement window |
| Idle daemon plus desktop memory | Under 350 MiB |
| Active core without optional local models | Under 750 MiB |
| Core installed/runtime storage | Under 500 MiB, excluding transcripts, worktrees, models, caches, and provider installations |

CPU percentages mean share of total machine capacity, aggregated across Lazoo-owned non-provider processes. Acceptance uses rolling 30-second and 5-minute windows, not a single Task Manager sample.

### 20.2 Enforcement

- Infrastructure and employee provider processes use separate Job Objects and accounting groups.
- Background summaries, indexing, config scans, and UI animation throttle at the soft threshold.
- The infrastructure job uses Windows CPU-rate controls where compatible, plus daemon scheduling and queue backpressure.
- User turns, cancellation, delivery acknowledgements, watchdog, and incident persistence retain priority.
- Optional local models have independent concurrency and temperature/thermal pause rules.
- Worktree and archive disk use is continuously attributable by project and task.

The 20 percent gate must be demonstrated under idle, routing, failure storm, memory indexing, terminal streaming, and voice scenarios. A provider's resource spike is reported separately and does not let Lazoo hide its own overhead.

## 21. Testing and acceptance

### 21.1 Test layers

**Contract tests**

Recorded and synthetic provider streams verify parsing, capability negotiation, version changes, cancellation, progress evidence, permission events, and clean error mapping.

**Live adapter smoke tests**

Each available provider must pass install detection, authentication readiness, start, prompt, stream, tool call, cancel, resume where supported, terminal attachment, graceful stop, and crash recovery.

**Windows integration tests**

Cover named-pipe ACLs, reconnect, ConPTY input/output/resize, Job Object process trees, daemon/keeper restart, DPAPI or Credential Manager, long paths, reparse points, and Windows installer behavior.

**Bus and storage tests**

Cover transactional outbox, duplicate delivery, lost acknowledgement, recipient activation, restart during delivery, dead lettering, event-sequence recovery, database migration, and emergency-spool reconciliation.

**Git tests**

Cover concurrent assignments, branch collisions, dirty worktrees, repository relocation, conflicts, failed validation, integration authority, disk pressure, and recoverable cleanup.

**Policy tests**

Cover precedence, expired approvals, action fingerprints, offline employees, live policy changes, peer-message non-authority, full-autonomy boundaries, and audit immutability.

**Fault injection**

Kill providers and daemon processes, block pipes, corrupt adapter frames, simulate auth expiry, rate limits, stalled sessions, full queues, full disk, broken configuration, and unavailable summarization services.

**Performance tests**

Measure aggregate Lazoo CPU, memory, handles, threads, database latency, terminal throughput, event lag, and disk growth on the baseline laptop.

### 21.2 Core end-to-end acceptance scenario

1. The user configures Rhea as the lead and four peers across different providers.
2. All employees begin dormant; one is disabled.
3. The user starts a project conversation with Rhea.
4. Rhea receives a current roster and proposes a multi-part plan.
5. Under ask mode, Lazoo presents precise delegation approvals.
6. Approved employees activate into independent native sessions and editing worktrees.
7. An employee asks a peer a direct clarification without routing through Rhea.
8. The disabled employee is not selected and cannot receive dispatch.
9. One provider stalls after accepting work. Lazoo detects missing progress, retries safely, then raises a clean incident.
10. The threshold notifies Rhea, allows bounded remediation, and escalates to the user when unresolved.
11. Lazoo restarts during an in-flight message and reconciles without losing or duplicating the logical task.
12. Results include commits, verification, task links, messages, incidents, and concise memory records.
13. Integration follows configured authority and never overwrites dirty user work.
14. Employees return to idle and suspend after policy timeout.
15. Lazoo remains inside its measured resource budget.

### 21.3 Provider completion gates

| Provider | Gate before claiming support |
|---|---|
| Codex | App Server contract and fallback tested against supported installed versions |
| Claude Code | Live Windows session, messaging, permissions, cancellation, and resume/recovery tested through the restored native subscription or a configured external endpoint/model; record the channel actually verified |
| OpenCode | Server/SSE or ACP lifecycle tested, including accepted-but-stalled fault handling |
| Grok Build | Excluded from core v1 by the owner on 2026-10-16; historical adapter evidence does not count toward required gates |
| Antigravity | Supported agy CLI installed; long-lived stream-JSON lifecycle and resume tested |

Until those gates pass, the UI must say **Detected**, **Configured**, **Degraded**, or **Unverified**—never “Supported” based only on documentation.

## 22. Delivery stages

### Stage 1: repository and contracts

- architecture decision records;
- domain and event schemas;
- adapter capability contract;
- IPC and terminal-stream contract;
- policy and approval contract;
- incident taxonomy;
- benchmark harness and baseline capture;
- threat model and data-retention contract.

The first implementation milestone was a narrow Stage 1/Stage 2 bridge: a runnable control-plane vertical slice. It includes the Rust workspace, daemon, CLI, keeper, versioned control IPC, SQLite event/outbox persistence, current-user pipe security, one durable project-registration flow, restart recovery, and the emergency incident spool. That milestone excluded provider adapters, ConPTY, worktree creation, approvals, the task/message bus, memory processing, and the Tauri desktop shell.

### Stage 2: Windows control-plane kernel

- daemon, keeper, CLI, and minimal Tauri shell;
- SQLite event/outbox storage;
- named-pipe IPC;
- employee lifecycle;
- ConPTY and Job Object supervision;
- worktree manager;
- task/message bus;
- approvals;
- incidents, watchdog, emergency spool;
- live roster and resource governor.

Current Stage 2 status: daemon, keeper, CLI, SQLite event/outbox storage, named-pipe IPC, watchdog/emergency spool, durable employee configuration, a revisioned provider-truth roster, native-session Job Object ownership, canonical message/conversation/turn ledgers, schema-7 task dispatch, and a redesigned read-only Tauri shell are implemented. Accepted Codex children are durably claimed once and wake automatically when their worker becomes free. Each canonical delegation message is reused by the worker turn; projectless work runs in an owner-protected directory and repository work runs in a deterministic retained Git worktree. The dirty canonical checkout is not modified. Configuration drift and pre-provider restart terminalize task, message, and dispatch atomically; possible in-flight restart becomes uncertain and is never retried automatically. Task cancellation before provider I/O closes the queued message, turn, and dispatch without invoking the provider. Once provider I/O begins, cancellation requires native evidence; completion remains authoritative if it arrives first. Provider evidence remains required before `running`. Dispatch currently supports Codex only and fails closed for unsupported providers. Schema 8 adds the approval ledger: Codex command-execution and file-change approval requests are recorded first, answered by policy under `self-decision` (in scope) and `full-autonomy`, and held for the local user under `ask`; decisions are revision-bound to one fingerprinted action, pending requests expire or cancel with their turn, and restart closes prior-boot requests as owner-lost. Every path without an approval answers the provider `decline`. Daemon-managed Codex sessions receive the first four collaboration tools (`employees.list`, `task.delegate`, `task.wait`, `task.report`, declared on the wire as `employees_list`, `task_delegate`, `task_wait`, `task_report` because the Codex Responses API rejects dotted tool names) through the app-server dynamic-tool surface; each call acts only as the turn's own employee and session, goes through the same fail-closed task guards as the CLI, never widens authority, and answers the model a stable failure code instead of ending the turn. Schema 9 adds the human-readable incident ledger: session start failures, provider stalls, rejections, exits, and missing evidence, dead letters, dispatch failures, restart owner loss, and keeper emergency records fold into one incident per code, employee, and project with a plain-language title, explanation, and next action; three equivalent failures inside five minutes promote it to priority; only the local user acknowledges or resolves it; employees read and report incidents through `incident_list` and `incident_report`. Lead notice delivery closes the loop back to the lead without a schema change: every terminal child task and every priority incident about a child becomes a durable `result` or `status` notice to the lead of the task tree, delivered as one turn on the lead's idle session under its task authority, consumed inside `task_wait` when the lead is already waiting, and held while the lead is dormant until the local user resumes it; employee-authored traffic never activates a dormant employee. Peer messaging (`message.send` and `message.reply`, declared as `message_send` and `message_reply`) rides the same notice path without a schema change: an employee can message another employee that holds an accepted or running task in its own task tree, each message links both tasks and threads under the message it answers, the recipient reads it inside its own `task_wait` or `message_send` wait or as one turn on its idle session, a peer turn never completes the recipient's task, cross-tree messaging is refused, and a dormant recipient is never activated. Coalesced notice turns deliver every notice and peer message queued for one recipient and task as one turn, oldest first, each with its own acknowledgement evidence and without a schema change. The schema-10 activation policy lets the user opt an employee into bounded automatic activation: a recorded notice turn may start one session for a dormant `notice-turns` recipient on its own task and stops it after the turn; `manual` stays the default. Schema 11 adds per-employee idle suspension: an idle session owned by the daemon is stopped once its employee's `idleTimeoutSeconds` (60 to 86400, or 0 for never, the default) has passed with no turn on it, and never while a turn on it is unfinished; a turn recorded against a session that has since stopped is delivered on the employee's current session. Schema 12 adds bounded lead remediation (section 14.2, steps 2 to 6): an incident promoted on a direct child of an active root task opens a five-minute window (`escalation: lead`, `remediationDeadlineAt`) in which that root's lead may apply at most two remedies through the ninth tool, `incident_remedy` (`cancel-task` the affected child as the lead, or `redelegate-task` a failed or cancelled child's objective through the ordinary delegation guards); each remedy commits with its action, a refused one consumes no attempt, and the incident keeps every remedy as evidence. When the window closes, an incident that has not recurred since the lead's last remedy becomes `remediated`, and anything else unresolved becomes `escalation: user` with a stated reason; a recurrence after the second remedy or after a remediated window, a promotion with no lead, and a priority incident from before schema 12 reach the user at once. Only the user resolves an incident, and escalation never retries, restarts, re-authenticates, or changes authority. Section 14.2's default thresholds apply per code without a schema change: a stalled session, a daemon crash, or an employee process death under assigned work is promoted by its first occurrence, unexpected exits of one employee process by their second, and everything else by three inside five minutes. Schema 13 makes the user escalation a priority notification (section 14.2 step 4): every open incident that reaches `escalation: user` is announced at most once with the Windows notification the keeper already uses (section 14.3), claimed before it is shown, one notification per pass, never retried, and never repeated for a keeper record. The `task_wait` and `message_send` waits wake on the daemon's own store write instead of polling the store, with unchanged deadlines and evidence. Schema 14 adds the envelope's priority (section 11.1) and section 14.2's last default threshold: every incident notice to a lead is a `priority` message, the local user may send one through `message.send`, no collaboration tool can set it, and one dead-lettered priority message promotes `LZ-MESSAGE-DEADLETTER` at once, whether `message.fail` exhausted it or a failed notice turn, dispatch, or restart did (the timed sweep raises those from the durable dead letter). Priority does not change delivery order. Schema 15 makes those thresholds configurable, as section 14.2 says they are: the local user overrides one code's occurrences and window through `incident.threshold.set` (`lazooctl incident threshold set`), resets it to the default, and lists every described code with its effective threshold; a change applies from the next occurrence and never demotes anything, a death under assigned work and a dead-lettered priority message promote at once whatever is configured, and no collaboration tool can read or set a threshold. The desktop Inbox read model (information architecture section 9) is derived from those ledgers on every call without a schema change: `inbox.list` and `inbox.counts` return pending approvals, `proposed` and `uncertain` tasks, and open user-escalated incidents as `action-required` (the only items the rail badge counts), a lead's remediation window as `watching`, acknowledged escalations as visible but uncounted, and decided, terminal, or resolved rows as the paged `resolved` archive, each item carrying its row's id, revision, and state, Lazoo's own title and required action, a deep-link scope, kind-specific detail, and its applicable actions as data naming an existing revision-checked RPC; nothing is stored, no new mutation exists, and no collaboration tool can read it. Resource accounting (section 20) is displayed without a schema change: `system.resources` reports the daemon's own process and `session.resources` reports a live session's native process tree from its Job Object, as point samples that are never stored, never enforced, and never counted against each other. The shell retains one explicit `desktop.snapshot` product-data command and no employee, provider, roster, session, message, conversation, turn, task, dispatch, approval, or incident fields. Schema 16 to 18 (2026-10-15) add the remaining kernel services as bounded backend increments: owner-managed ConPTY terminals (`terminal.open`, `terminal.list`, `terminal.show`, `terminal.resize`, `terminal.stop`, a separate authenticated byte pipe, eight live terminals, 1 MiB output rings, contained process trees); ACP and Claude Code per-action approval requests bridged into the existing one-action approval observer, while governed delivery for OpenCode and for Claude Code under `ask` and `self-decision` still fails closed because native allow rules can bypass the observer; bounded authenticated control IPC admission and deadlines; and a shared Windows Job Object for the daemon, keeper, and CLI with a 20% CPU cap, 15% background throttling, and rolling accounting, with provider trees and the desktop outside it. Since 2026-10-17 every `lazooctl` request wait and the governor initialization lock are bounded; the owner can attach a terminal that resumes a suspended Claude Code or Codex session; a suspended Claude Code session is resumed on its next activation at the same configuration revision in its prior working directory, while Codex starts fresh with a recorded reason (`thread/resume` carries no dynamic tools in any installed codex-cli); cancellation is proven deterministically for every adapter; and OpenCode employees receive turns and tasks under `full-autonomy` only, launched prompt-free through a per-launch `OPENCODE_PERMISSION` block, the same shape as Claude Code. A completed `provider.smoke` records the provider `verified` with a ready native channel (sticky across probes that cannot see a login; replaced by degradation, `needs-login`, or a version change), which is what lets an OpenCode employee become dormant and lead. OpenCode then passed the same live daemon lifecycle as Claude Code and Codex (messaging, cancellation, stop, fresh wake, planned shutdown) on 2026-10-17 after the ACP adapter learned to report completion in the contract vocabulary. The desktop-inclusive budget was measured the same day with the built desktop open (`desktop-budget.ps1`): CPU idle 0.076 percent and routing 0.307 percent are inside the 1 and 5 percent targets; idle memory is 127 MiB by private working set (the Task Manager basis) against the 350 MiB target, with the summed working set of 417 MiB kept as the upper bound because six WebView2 processes share pages. OpenCode then gained governed `ask` and `self-decision` delivery through a forced `{"*":"ask"}` block and the ACP approval bridge, with network and connector kinds declined until a third approval kind is admitted to the Inbox contract. Per-capability policy, session-wide grants, notification actions and quiet hours, live provider-session terminal takeover (the section 27 choice between takeover, fork, and a rendered transcript is pending), and UI binding remain later slices.

### Stage 3: first live provider adapters

- Codex;
- OpenCode;
- Grok Build;
- common adapter conformance suite;
- native terminal takeover and session inspection.

These three are first because their local installations are currently usable and expose documented machine-oriented interfaces.

Current Stage 3 status: Codex, OpenCode, and Grok Build adapters are implemented. Earlier provider-smoke exact-output and resume gates passed through the full CLI, named-pipe, daemon, and native-adapter path. The newer Codex conversation-delivery path now has both deterministic release proof and a separate authorized post-fix exact-output live gate through the full daemon/CLI path. Grok Build was excluded from core v1 by the owner on 2026-10-16; its historical adapter evidence is retained but counts toward no required gate. On 2026-10-17 the live section 21.3 runs passed for Codex and OpenCode (readiness, model-backed turn, resume, daemon smoke; OpenCode needed one retry after the free model leaked a `</think>` tag into an exact-token reply); see `docs/verification/2026-10-17-live-lifecycle-gates.md`.

### Stage 4: remaining provider adapters

- validate agy authentication and a model-backed stream, then build the Antigravity adapter against the installed CLI;
- implement the Claude Code adapter against recorded/documented fixtures;
- complete Claude live harness acceptance through the restored native subscription or a configured external endpoint/model;
- verify and report the native Anthropic subscription path separately when a subscription is available;
- validate optional native-message acceleration without weakening the Lazoo bus.

Current Stage 4 status: the Antigravity adapter is implemented and passed fresh model and same-process continuation gates. The Claude Code adapter is implemented and passes discovery, version, authentication-status, and stream-protocol startup checks, and on 2026-10-16, after the owner restored the native subscription, Claude Code 2.1.300 passed a short live prompt, stream, and completion smoke through Lazoo in 11.3 seconds, as did OpenCode 1.19.0 with Big Pickle; Codex and Antigravity retain their 2026-10-15 passes. Claude Code is still not claimed as fully supported: the section 21.3 lifecycle gates (messaging, permissions, cancellation, resume and recovery) remain open. On 2026-10-17 Antigravity passed its live readiness, model-backed turn, `--conversation` resume, and daemon smoke, and Claude Code passed a full live lifecycle (messaging, cancellation, resume on wake, planned shutdown) through the native subscription, after a cancellation defect found by that run was fixed; see `docs/verification/2026-10-17-live-lifecycle-gates.md`.

### Stage 5: memory and integration federation

- compressed local transcript archives;
- SQLite FTS and typed memory ledger;
- external/provider summarization queue;
- context selection and source pointers;
- read-only provider config discovery;
- MCP/plugin/skill compatibility model;
- preview, backup, apply, and rollback.

Current Stage 5 status: read-only provider configuration discovery is implemented for MCP servers (`provider.config`): Codex, Claude Code, and OpenCode configuration files are parsed, secrets are redacted to key names, integrations are hashed to one identity, and duplicates, conflicts, and unsupported entries are named; nothing is written. Schema 16 to 18 implement the rest of the stage as bounded services: compressed, hash-checked canonical turn archives under a 256 MiB default quota with a 4 MiB per-source cap, global and per-project retention policies, recoverable write and cleanup intents, and explicit cleanup preview and apply; a typed FTS memory ledger with source citations, scoped retrieval (project memory across tasks, Home within its conversation), and bounded context injection of eight facts or 2 KiB; an opt-in native summary worker (exact employee and model, one active job, per-scope admission spacing, a four-minute deadline, strict JSON validation, a fresh reserved session per attempt, no automatic resend); and selected Codex MCP configuration preview, apply, show, and rollback with private backups and redacted metadata for the exactly reviewed Codex 0.155.1 and 0.162.0-alpha.17.2 formats. Schema 19 (2026-10-17) adds the redacted raw provider frames of completed turns as a second archive kind within the same quota and cleanup preview; wire frames that map to no event are not captured. Other configuration formats and versions, plugin and skill apply, external API summarizers, and vector retrieval remain unimplemented or refused.

### Stage 6: connectors

- provider-native connector registry;
- OAuth/setup routing;
- scope and policy visibility;
- health, detach, and revocation flows;
- migration previews without credential copying.

### Stage 7: vector retrieval experiment

- baseline dataset and relevance rubric;
- local model evaluation on the owner's laptop;
- hybrid retrieval comparison;
- ship only if quality and resource gates pass.

### Stage 8: Lazoo Voice

- integrated lead conversation;
- standalone push-to-talk overlay;
- streaming STT/TTS;
- interruption, privacy, and retention controls;
- performance and accessibility validation.

Each stage requires a separate implementation plan and acceptance evidence. Later stages must not be used to postpone reliability work in the control-plane kernel.

## 23. Key risks and mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| Provider interfaces change rapidly | Broken adapters and false capability claims | Version negotiation, generated/vendor schemas, fixture matrix, explicit supported ranges, degradation instead of guessing |
| Native provider behavior differs from docs | Hangs or lost work | Live conformance gates, progress evidence, fault injection, durable state, bounded timeouts |
| Cross-provider messages duplicate | Repeated actions | At-least-once contract, idempotency keys, immutable logical message identity |
| Daemon or database failure hides incidents | Silent orchestration failure | Keeper, emergency spool, desktop pipe-loss detection, outbox reconciliation |
| Employees overwrite each other | Lost or conflicting code | Worktree default, single editing owner, serialized integration, retained dirty state |
| “Full autonomy” expands unexpectedly | Unsafe actions | Explicit autonomy envelope, hard invariants, capability policies, immutable approvals/audit |
| External memory leaks sensitive content | Privacy loss | Per-project opt-in, redaction, provider visibility, source minimization, native secret stores |
| Local memory/model overwhelms laptop | Poor responsiveness | FTS and summaries first, optional benchmarked vectors, CPU/disk budgets, throttling |
| Too many provider-specific abstractions create a lowest common denominator | Loss of native value | Capability-aware adapter contract and provider-native extensions |
| Close competitors make greenfield wasteful | Time spent rebuilding existing work | Reassess Relaydeck/Muster at each stage boundary against Lazoo's differentiators |
| Low disk space accumulates worktrees/transcripts | Build failures or system pressure | Attribution, quotas, warnings, compressed archives, recoverable cleanup, no silent deletion |
| Claude cannot be live-tested now, and agy's model-backed path has not yet been exercised | False completion | Keep explicit verification states; test agy during adapter work and do not claim Claude live support until the subscription returns |

## 24. Definition of core v1

Core v1 is complete when:

- the Windows desktop, daemon, keeper, and CLI are installable and recoverable;
- named employees can be created and changed live;
- any verified employee can be the lead;
- dormant activation, native sessions, terminals, cancellation, and suspension work;
- editing assignments use safe worktrees by default;
- delegation and peer messaging are durable across restarts;
- ask, self-decision, and full-autonomy modes enforce documented boundaries;
- live roster changes prevent offline dispatch;
- clean incidents, thresholds, bounded lead remediation, emergency reporting, and user notifications work;
- Codex and OpenCode pass live adapter gates; Grok Build is excluded from core v1;
- Antigravity passes its adapter gates against the installed agy CLI;
- Claude Code passes its live harness gate through the restored native subscription or a configured external endpoint/model, with the verified authentication channel recorded;
- local transcript archive and summarized typed-memory modes work with bounded context injection;
- MCP/plugin/skill federation can import, preview, apply, and roll back at least the verified supported configurations;
- the end-to-end acceptance scenario passes;
- Lazoo stays within the baseline performance budget.

Connectors, vector retrieval, and voice are planned product phases but are not allowed to weaken the core v1 completion criteria.

## 25. Decisions intentionally deferred

These choices belong in implementation plans or benchmarks and do not block the architecture:

- the TypeScript UI framework inside Tauri;
- the exact Rust async/runtime and database libraries;
- final JSON-RPC method names and schema serialization library;
- default idle timeout and per-provider concurrency;
- which free summarization model/provider is selected first;
- the local embedding model, if any;
- STT and TTS providers;
- visual design language and final tray behavior;
- installer/update channel and code-signing procurement.

The constraints in this brief remain binding whichever implementation choices win.

## 26. Research sources

### Provider and protocol sources

- [OpenAI Codex App Server reference](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
- [Codex generic cross-session messaging proposal](https://github.com/openai/codex/issues/16447)
- [Codex cross-session communication feature request](https://github.com/openai/codex/issues/22768)
- [Claude Code cross-session messaging](https://code.claude.com/docs/en/cross-session-messaging)
- [Claude Code agent teams](https://code.claude.com/docs/en/agent-teams)
- [OpenCode server documentation](https://opencode.ai/docs/server/)
- [OpenCode SDK documentation](https://opencode.ai/docs/sdk/)
- [OpenCode accepted-but-stalled session report](https://github.com/anomalyco/opencode/issues/33394)
- [Grok Build ACP agent mode guide](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/15-agent-mode.md)
- [Antigravity CLI headless mode](https://antigravity.google/docs/cli/headless)
- [Agent Client Protocol](https://agentclientprotocol.com/)

### Windows and desktop sources

- [Tauri 2 documentation](https://v2.tauri.app/)
- [Tauri security and capabilities](https://v2.tauri.app/security/)
- [Tauri sidecars](https://v2.tauri.app/develop/sidecar/)
- [Tauri Windows installer guidance](https://v2.tauri.app/distribute/windows-installer/)
- [Microsoft pseudoconsole guidance](https://learn.microsoft.com/en-us/windows/console/pseudoconsoles)
- [Microsoft Job Objects](https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects)
- [Microsoft named-pipe security and access rights](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
- [Microsoft Data Protection API example](https://learn.microsoft.com/en-us/windows/win32/seccrypto/example-c-program-using-cryptprotectdata)

### Landscape sources

- [Relaydeck](https://relaydeck.ai/)
- [Relaydeck source repository](https://github.com/relaydeck/relaydeck)
- [Muster desktop](https://musterup.dev/)
- [Asterism source repository](https://github.com/qmilab/asterism)
- [Muster mission control](https://musterai.org/)
- [Orrery source repository](https://github.com/DivyamTalwar/ORRERY)

## 27. Owner-reviewed decisions and next gate

The owner accepted this product and engineering contract on 2026-08-28 with the following clarifications:

- Lazoo remains greenfield, with Relaydeck used as a benchmark rather than an implementation base.
- The keeper and dual-path emergency incident spool are required in the first runnable control-plane milestone.
- Ask, self-decision, and full-autonomy semantics remain as documented, with **Ask** as the default.
- Worktree isolation is configurable between **All editors isolated** and **Lead edits main**; new projects default to full isolation.
- Managed indexed typed memory is the default active mode. A dedicated worker or approved external API may produce it.
- Compressed source transcripts are retained by default within visible limits, with global and per-project opt-out.
- Claude Code live harness verification may use a configured external endpoint/model; native Anthropic subscription verification is tracked separately.
- Desktop Home is a new-work launcher with first-class projectless conversations; registered projects use stable rail icons and conversation-first history.
- The user chooses a lead per Home/project space and Lazoo remembers the choice; prompt, lead, and authority remain visible while advanced launch controls stay behind Options.
- Lazoo embeds each lead and worker's actual native harness TUI in its own supervised terminal. It does not substitute a generic merged chat UI.
- Delegated workers appear in a bottom dock, and one worker terminal may expand below the still-visible lead terminal.
- One global Inbox owns approvals, permissions, failures, and incidents; Settings is a pinned full-page destination.

The runnable control-plane milestone, polished read-only Tauri desktop shell, backend native-provider adapters, durable employee roster, native-session Job Object ownership, canonical collaboration-message and conversation/turn ledgers, schema-7 automatic Codex task dispatch, and the schema-8 approval gate are implemented with deterministic acceptance. The earlier conversation-delivery slice also has explicit live Codex evidence; the automatic-dispatch and approval-gate slices deliberately used only fake provider transports, the disposable Codex protocol fixture, and disposable repositories. Provider, roster, session, message, conversation, turn, task, dispatch, workspace, and approval truth are available through the daemon and CLI, while `desktop.snapshot` remains unchanged. `accepted` still means queued control-plane truth and `approved` means Lazoo answered the provider `accept`; only native acceptance or progress proves execution. ConPTY, per-capability policy, session-wide grants, managed memory, resource governance, other-provider task delivery and approval bridging, and UI binding remain separate reviewed slices. Embedded native terminals still must not be claimed until the ConPTY and terminal-I/O gates pass. Do not emit `UI BACKEND CONTRACT: GO` until a typed desktop-facing task/dispatch contract is separately reviewed. Each later slice still requires its own design, plan, and acceptance evidence.
