# User Notifications Design

**Date:** 2026-10-10

**Status:** Implemented on `feat/user-notifications` (stacked on `feat/incident-thresholds`, pull request #4) with deterministic acceptance, for owner review in a pull request before merge.

**Scope:** Backend-only. Schema 13 adds two incident columns. The daemon announces each incident that reaches `escalation: user` with one Windows notification, using the toast the keeper already uses. No desktop app, Tauri, tool, CLI, or RPC shape change beyond the two new incident fields; no live model use.

## 1. Outcome

After bounded lead remediation, an unresolved failure became `escalation: user` in the ledger and the event stream, but the user only learned about it by running `lazooctl incident list --escalation user`. The brief's section 14.2 step 4 asks for a priority *notification*, and section 14.3 names the native Windows notification as the channel the keeper already uses when the normal path is down.

The daemon now shows one Windows notification for user-escalated incidents, and answers three questions without inference:

1. Was the user told? `userNotification` on the incident: `shown`, `failed`, `keeper` (the keeper already showed its own), or `claimed` (claimed, but the daemon stopped before recording the outcome), with `userNotifiedAt`.
2. Can the user be told twice? No. The claim commits before the notification is shown, in the same transaction that re-checks the incident is still unannounced.
3. Can a burst flood the desktop? No. One pass shows one notification: the incident itself when it is alone (its title, explanation, and next action, bounded to 240 bytes), otherwise "Lazoo: N incidents need you" with their titles.

## 2. Rule

The timed sweep (`run_timed_sweeps`, every 15 seconds) runs `notify_user_escalations_once(now)` after closing remediation windows:

1. `claim_user_notifications` selects, oldest first and at most 64 per pass, incidents with `escalation = 'user'`, `state = 'open'`, and no `user_notified_at`. A keeper-sourced record is settled as `keeper` with no new notification. Every other one is marked `claimed`.
2. The daemon shows one notification for everything it claimed.
3. `settle_user_notifications` records `shown` or `failed` on rows still `claimed`, so a settled outcome is never rewritten. A failed notification is logged and never retried; the incident stays in the ledger for `lazooctl`.

Only open incidents are announced. One the user already acknowledged or resolved has been seen, and a lead window (`escalation: lead`) is not the user's until the sweep or a recurrence escalates it. Each claim and settlement bumps the incident revision and emits `incident.user-notified`.

## 3. Channel

`UserNotifier` is a one-method trait in `lazood`. The daemon's default is `lazoo_runtime::NativeNotifier`, the same Windows toast (application id `Lazoo`) the keeper shows when the daemon exhausts its restart budget. Tests substitute a recorder with `App::with_user_notifier`, so no test shows a real notification. The real-binary acceptance scripts run the real daemon, so like `first-slice.ps1` (whose keeper notification predates this slice) they may show a Windows notification on the machine running them.

## 4. Durable records

Migration `0013_user_notifications.sql`: `incidents.user_notified_at TEXT` and `incidents.user_notification TEXT` checked to `claimed`, `shown`, `failed`, or `keeper`. `IncidentRecord` gains `userNotifiedAt` and `userNotification` (`IncidentUserNotification`), serde-defaulted. Incidents escalated before schema 13 start unannounced, and the first pass names them in one notification.

## 5. Rules kept

- Notification is evidence of telling the user, nothing more: it never acknowledges, resolves, retries, restarts, or changes any work.
- The ledger and event stream stay authoritative; the notification is a best-effort pointer to them.

## 6. Deterministic acceptance

`scripts/acceptance/user-notifications.ps1` runs protocol, store, and daemon fake-backend proofs with a recording notifier and confirms the desktop tree is unchanged, writing `artifacts/acceptance/user-notifications-manifest.json`. No provider smoke or live model turn.

## 7. Explicitly deferred work

- an opt-out or quiet-hours setting, and per-project notification policy;
- actions on the notification (open the incident, acknowledge);
- typed desktop Inbox binding, and everything else NEXT-SESSION lists.

## 8. Review gate

Merge authorizes only the schema-13 columns, the at-most-once Windows notification for open user-escalated incidents, and the two incident fields; it does not authorize desktop app changes, notification actions, or any other channel.
