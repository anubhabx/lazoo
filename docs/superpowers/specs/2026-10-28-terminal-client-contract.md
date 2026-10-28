# Native terminal client contract

This document describes the implemented backend transport. It does not claim that
a native TUI can display the session concurrently with Lazoo driving that session,
or that the desktop has implemented its terminal renderer.

## Ownership and control

The Rust client connects to the local installation using `lazoo_ipc::PipeClient`.
Credentials stay in the Rust process. The frontend must not receive the installation
nonce, pipe credentials, or an arbitrary shell/RPC execution primitive.

Control requests use the normal authenticated JSON-RPC pipe. Existing methods:

| Method | Purpose | Required concurrency checks |
| --- | --- | --- |
| `terminal.open` | Open an owner-selected executable in ConPTY | Exact idempotency key and request body |
| `terminal.list` | Read terminals for this daemon boot | Result includes `bootId` |
| `terminal.show` | Refresh one terminal record | `id` |
| `terminal.resize` | Resize one terminal | `id`, `expectedRevision`, `expectedBootId` |
| `terminal.stop` | Stop and reap its contained process tree | `id`, `expectedRevision`, `expectedBootId` |
| `session.terminal.attach` | Resume a suspended employee session in an owner terminal | Session `id`, `expectedRevision`, `columns`, `rows`, `idempotencyKey` |
| `session.terminal.status` | Recover attachment and eligibility after reconnect | Session `id`; returns current `sessionRevision` and `bootId` |

The status result contains `sessionId`, `sessionRevision`, `bootId`, `opening`,
nullable `terminal`, `attachable`, nullable `blockingCode`, and nullable
`blockingReason`. It does not reserve an attachment or activate a session.
Use the live terminal's ID and byte offset to reconnect. A retained exited record
has `live: false`; it is metadata, not retained screen content. Eligibility is
advisory: the attach mutation rechecks revision, ownership and native command
validity. The current daemon's bounded terminal cache may evict old metadata.

An attachment resumes the provider's own saved native conversation using its
supported command. It does not transfer an active protocol connection. The session
must be suspended; summary-worker sessions and providers without a proven resume
command are refused. The backend derives the executable, resume ID and working
directory from its own records. Active native ownership and attachment reservations
are checked to prevent two processes from driving the same saved conversation.

`TerminalRecord` carries `id`, `bootId`, `revision`, dimensions, `pid`, executable,
arguments, working directory, `live`, and nullable `exitCode`. These are lifecycle
facts, not provider turn or task acceptance evidence. A terminal is not a control
channel for delegation or approval decisions.

## Separate byte channel

Use `PipeClient::connect_terminal(installation, client_name, client_version)`.
It selects the installation's pipe plus `.terminal.v1` and performs the ordinary
authenticated handshake. Every operation uses a new authenticated connection;
the server consumes one request and closes the connection after its response.
The terminal listener has a separate admission budget of 16 peers and a five-second
deadline for an exchange after authentication.

All frames have a four-byte unsigned little-endian payload length followed by that
many bytes. Use the existing connection framing helpers rather than concatenating
raw JSON and terminal bytes. A metadata request is bounded to 4 KiB.

Read request (`TerminalIoRequest::Read`):

```json
{"operation":"read","id":"<terminal UUID>","expectedBootId":"<boot UUID>","after":0,"limit":65536}
```

The server returns a JSON `TerminalIoResponse` frame. On success its `output`
contains `start`, `end`, `length`, `gap`, `live`, and nullable `exitCode`, followed
by a separate raw-byte frame, including an empty frame for zero output. On failure,
`error` is populated, `output` is null, and there is no raw-byte frame.

Read limits are 1 through 65,536 bytes. Keep the returned `end` offset for the next
read. When `gap` is true, earlier bytes fell outside the bounded ring: report the
gap and consume from `start`; never silently describe that output as complete.
The ring holds at most 1 MiB per terminal. Maintain an incremental byte decoder
or pass raw chunks directly to a terminal emulator; frame boundaries are not
UTF-8 character or escape-sequence boundaries.

Write request (`TerminalIoRequest::Write`):

```json
{"operation":"write","id":"<terminal UUID>","expectedBootId":"<boot UUID>"}
```

Send that JSON frame followed by a raw input frame of at most 16,384 bytes. The
server returns one response with null `output`; a non-null `error` means failure.
Do not replay an input write automatically after losing its acknowledgement:
the bytes may already have reached the process.

## Reconnect and process exit

Keep terminal ID, boot ID, and last read offset across a UI reconnect. Refresh
lifecycle metadata before resuming control. A daemon restart invalidates the old
boot and all old terminal ownership; an old terminal must never be silently
rebound to a new process. Output polling is bounded and should back off when empty.
The byte endpoint returns currently buffered output, not a long-lived push stream.

Stopped terminals retain compact metadata; their live process and output ring are
released. The UI should retain the output it has already rendered and show the
exit state. It must not promise that a later reconnect can reconstruct the full
screen of an exited terminal. At most eight terminals may be live or reserved.

## Verification anchors

`crates/lazoo-protocol/src/terminal.rs` and `terminal_stream.rs` define wire shapes.
`apps/lazood/src/terminal_server.rs` implements framing and admission.
`apps/lazood/src/app/terminal.rs` implements lifecycle and attachment ownership.
`apps/lazood/tests/terminal_service.rs` exercises the authenticated byte pipe.
`scripts/acceptance/session-terminal-attach.ps1` exercises attachment guards.
Fresh run results belong in the readiness verification record, not this contract.
