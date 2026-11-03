# Skills and plugins backend

The owner-approved slice implements PROJECT-BRIEF section 16. Native files remain authoritative. The desktop is separately owned; only its Rust RPC allowlist changes here. Git staging and commits belong to Claude.

## Boundaries

Use a Lazoo-owned, owner-protected library under the data root, never the shared `.agents/skills` directory. A source preview stages bounded untrusted input without writing harness directories. Install selects skills, providers and link/copy mode; explicit replacement applies only to existing Lazoo-owned entries. Unmanaged collisions always refuse. Inventory reports actual paths and shared visibility, independently of desired exposures.

Provider helpers own source normalization, bounded filesystem traversal/checksums, junctions, environment-derived directories and native plugin commands. The daemon owns managed-profile expansion, serialization, revision fences, durable intent/audit, and event publication. The store owns a versioned extension ledger; previews contain no credential values. The CLI only constructs the same RPC requests as the desktop.

## Skill lifecycle

GitHub shorthand, GitHub tree URLs, skills.sh pages, HTTPS Git URLs and local directories normalize to a recorded source. Git clones are shallow, non-interactive, time/output/disk bounded, with hooks/submodules disabled. Source links, reparse points, traversal and special files refuse. Discovery prioritizes root SKILL.md, conventional collections and declared plugin skill paths, then bounded fallback; a discovered skill shadows its descendants. Content hashes cover sorted relative names and bytes. Metadata validation reports portable naming, frontmatter and description problems.

Previews have identifiers, expiry, source revision, checksums and conflict records. Every apply checks the ledger revision and staged bytes again. Library replacements and exposures use prepared directories, timestamped backups, rename, read-back checksum/target verification and rollback. Removal first verifies all owned paths. Changed or foreign paths refuse without deleting them. Interrupted intents stay visible as recovery-needed and block further mutations rather than silently assuming completion.

Selected harnesses expand to the personal directory plus every existing native account profile. Targets derive from USERPROFILE/HOME, XDG_CONFIG_HOME, CODEX_HOME, CLAUDE_CONFIG_DIR and COPILOT_HOME. Shared discovery roots are read-only. Newly created profiles need an explicit expose refresh. Codex-only exposure uses CODEX_HOME/skills if the installed-version evidence supports it. Antigravity's primary target and alternate discovery roots are separately documented with uncertainty.

## Plugin lifecycle

Expose a capability matrix using Native, Translated with documented limitations, Lazoo-only, Unsupported, Requires provider restart, and Requires user OAuth or native setup labels. Claude, Codex, Copilot and Antigravity use native CLIs; OpenCode supports npm installation and read-only JSONC/local-module inventory. Unsupported configuration edits refuse, including Codex enable/disable unless integrated with its reviewed config writer, and OpenCode removal/disable.

Plugin preview records target/profile, action, before-state and exact command description. Apply rechecks the observed state and revision, backs up known native configuration, runs non-interactively under the existing process-job helper, re-probes, and records each target's actual outcome. Cross-harness install adds a named marketplace when absent before installing. Failed native mutations are not called successful; compensation and rollback limitations are explicit. Provider-native package managers cannot offer a cross-provider atomic transaction. Do not restore a config backup over concurrent external changes or delete unknown native cache files.

## Verification

Deterministic temporary-home tests cover source/path safety, preview immutability, checksum conflicts, install/update/expose/unexpose/remove, managed profile expansion, copy/junction ownership drift, revision races, native CLI arguments, process limits, redaction and partial plugin outcomes. Acceptance runs only against redirected homes, writes a manifest, and is registered with gate.ps1. Run format, strict workspace clippy and workspace tests using the owner's bounded build environment and existing target directory. Read-only live evidence never substitutes for fixture acceptance.
