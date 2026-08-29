# Next session: decide desktop structure before implementation

## Current owner verdict

The dark visual system, frontend behavior, and packaged-window mechanics are technically verified, but the current desktop UI is still not sufficiently understandable. Treat the existing shell as an implementation baseline, not as an approved information architecture.

Do not continue adding screens, visual polish, navigation items, provider surfaces, or other desktop UI implementation until the structure below is reviewed and approved.

## Next-session objective

Decide the Lazoo desktop information architecture and sitemap before writing more UI code.

The session must establish:

1. The primary jobs the owner needs Lazoo to make easy.
2. The first-run and everyday user journeys, including what should be understandable within the first minute.
3. The sitemap: every top-level destination, its purpose, and why it deserves navigation space.
4. The screen inventory beneath each destination, including inspector, modal, command-palette, and settings responsibilities.
5. The navigation model: persistent navigation, contextual navigation, command search, breadcrumbs, and back behavior.
6. The information hierarchy for each screen: primary question, essential data, primary action, secondary actions, and progressive disclosure.
7. The terminology and labels used for projects, employees, providers, tasks, activity, incidents, approvals, and system state.
8. Required loading, empty, degraded, disconnected, permission, and first-run states for each approved surface.

## Required deliverables before implementation resumes

- An owner-approved sitemap.
- A screen-by-screen purpose and content inventory.
- The core navigation and task-flow diagrams.
- Low-fidelity wireframes for the primary route and the most important operational screen.
- A decision on what remains visible globally versus what belongs in contextual inspectors or commands.
- A revised UI specification and implementation plan based on those decisions.

## Guardrails

- Start from user intent and comprehension, not from the current navigation labels or existing component layout.
- Preserve the Windows-first, local-first, single-user product boundary.
- Keep the current backend and `load_desktop_snapshot` boundary unchanged unless a separately reviewed product slice requires expansion.
- Do not treat the technically passing redesign acceptance as evidence that the sitemap or UX structure is accepted.
- Do not begin the next implementation slice until the owner explicitly approves the proposed structure.
