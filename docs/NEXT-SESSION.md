# Next session: review the approved desktop structure specification

## Current owner verdict

The owner approved the Lazoo desktop information architecture and low-fidelity interaction design on 2026-08-30.

The target desktop is work-focused:

- Home is a new-work launcher and owns projectless conversation history.
- A Discord-style rail selects Home or a registered project.
- The contextual sidebar contains New work and user conversation history.
- The lead and every delegated worker use their actual native harness TUI in separate embedded terminal sessions.
- Worker cards occupy a bottom dock; one selected worker expands into a resizable terminal below the lead.
- One global Inbox collects approvals, permissions, failures, and incidents.
- Settings is a pinned full-page destination.

The written contract is `docs/superpowers/specs/2026-08-30-lazoo-desktop-information-architecture-design.md`. It is awaiting the owner's final written-spec review.

## Next-session objective

Review the written specification for any wording or scope changes. After explicit approval, write a narrow implementation plan. Do not begin UI implementation before that review gate.

The implementation plan must respect dependency truth:

1. Provider/harness status and first employee setup must be real before the launcher claims a startable lead.
2. The native lead terminal requires an approved ConPTY/provider-adapter slice.
3. The worker dock requires durable delegation, task, and employee-session state.
4. Inbox actions require typed approval, permission, and incident contracts.
5. Settings pages appear only as their underlying configuration contracts become functional.

## Approved navigation summary

```text
Home
Projects (one rail icon each)
Add project
Inbox
Settings
```

There is no target top-level Control room, Activity, System, global task board, or synthetic multi-provider chat screen.

## Guardrails

- Preserve the Windows-first, local-first, single-user boundary.
- Preserve the approved dark visual system and existing minimum 860 by 620 window.
- Treat the current desktop as a technically verified baseline, not the target structure.
- Keep `load_desktop_snapshot` unchanged until an approved implementation slice defines additive typed contracts.
- Do not show fake terminals, workers, settings, or optimistic harness status.
- Use actual harness TUIs in supervised embedded terminals; Lazoo supplies the orchestration shell.
- Keep the generated visual-companion artifacts under ignored `.superpowers/`; they are not product assets.
