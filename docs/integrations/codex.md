# Codex Conversation Integration

Codex is the first local agent, selected by the user. Prioritize observing existing local Codex conversations, including desktop sessions; do not launch or take over tasks to make monitoring work. Luma must not approve requests, answer prompts, resume threads, or start another daemon implicitly.

## Hook Monitoring (Implemented)

The user selected Codex lifecycle hooks as the monitoring source. Luma now provides a native headless command in the same executable, a metadata-only SQLite event projection, dashboard session cards, and explicit install/disable/clear controls. No private App Server socket, conversation transcript reader, cloud service, Python, Node, or GitHub CLI is required at runtime. The App Server findings below are historical context rather than a blocker for this implementation path.

### Enable and Remove

1. Install Luma in a stable location before enabling hooks; do not enable from a temporary DMG mount or a build output that will be deleted.
2. Open Settings → Codex monitoring → Enable Codex hooks. Luma merges its eight handlers into the user hooks.json under CODEX_HOME when that is an absolute configured path, otherwise ~/.codex/hooks.json. Existing unrelated handlers are preserved. Inline config.toml hooks remain untouched.
3. In Codex CLI, use /hooks to review and trust the new definitions. Luma does not bypass trust or approve any agent action. Desktop sessions may need to be reopened or restarted to load the definitions; a real installed-version check is still required.
4. Start a new turn in Codex. Luma shows sessions only after their first valid event; it does not discover or backfill all existing conversations.
5. Disable monitoring stops collection and removes only Luma-owned handlers. Clear saved sessions is a separate confirmed operation. If Luma moves, use Repair hooks (or enable again) to update executable paths, then review trust again.

Setup uses a process-safe advisory lock, an atomic JSON replacement, and a pre-write concurrent-edit check. Invalid or symlinked configuration is rejected instead of overwritten. An install manifest remembers old Luma commands so interrupted repairs can remove only those commands. A local failure is shown in Settings. External applications do not honor Luma's advisory lock; avoid simultaneously editing hooks.json during setup.

### Events and Confidence

| Hook | Luma projection | Limit |
| --- | --- | --- |
| SessionStart | Unknown | Opening/resuming is not proof of running |
| UserPromptSubmit | Running | Prompt entered the current turn |
| PreToolUse | Running; recognized input tools create Waiting | Explicit local-tool event; hosted/special tool coverage may differ |
| PermissionRequest | Waiting, with approval-request detail | Another hook may auto-approve; this is a provisional request observation |
| PostToolUse | Running after the matching pending request resolves | Unrelated tool callbacks do not clear waiting requests |
| Stop | Turn stopped | Other hooks can request continuation; no verified success claim |
| Interrupt | Canceled | Main-turn interruption only |
| SessionEnd | Unknown / ended detail | Does not establish successful completion |

Exact recognized input tool names are request_user_input, ask_user, functions.request_user_input and functions.ask_user. Their actual desktop invocation must be verified; an absent hook never becomes inferred waiting. Subagent-tagged payloads are ignored to avoid assigning child state to a parent session. Failed tools do not imply failed turns, and no generic Failed or Completed state is fabricated from these hooks.

### Local Data and Delivery

A command handler invokes the installed Luma executable with --luma-codex-hook-v1 and the private application database path. The handler consumes at most 1 MiB JSON on stdin, retaining only session/turn IDs, tool identity, event kind, receive time, and project basename. Titles are synthetic session labels. Prompts, assistant text, tool arguments/results, absolute paths, and transcript paths are discarded. The helper returns an empty JSON object, never approval/denial/continuation output, and exits within a bounded lifetime even if stdin stalls. Windows commands use the built-in PowerShell launcher with an encoded literal invocation and explicit stdin forwarding; no separate scripting runtime is bundled.

The helper writes the dedicated codex-hooks.sqlite3 directly using SQLite WAL and a 500 ms busy timeout. No network listener is exposed, and proxy settings do not affect it. This is a same-user local queue, not a security boundary against other processes running as that user. Hook failures do not interrupt Codex. Unreceived events cannot be reconstructed. The GUI polls the projection every 500 ms and emits changes; it re-reads and resubscribes on focus after a subscription failure. Status revisions prevent delayed reads/actions replacing newer events.

The store defaults to disabled, retains at most 200 sessions and 20,000 dedup IDs for seven days, and keeps only bounded pending-wait/run metadata. Duplicate event IDs and older timestamps cannot regress state; callbacks from retired turns cannot end a newer turn. Equal-timestamp terminal ambiguity becomes Unknown. Startup invalidates retained running/waiting state until fresh evidence arrives. Silence is never interpreted as completion. Separate deletion leaves the enabled setting intact.

### Validation Boundary

Rust tests cover normalization, configuration merge/install/remove and malformed-file preservation, decimal-free metadata persistence, state ordering/dedup, parallel waits, restart, retention, and concurrent SQLite reads. UI tests cover status ordering/privacy and explicit setup/removal/deletion. A native executable smoke test injects synthetic stdin events into a temporary database and verifies running → waiting → running → stopped plus cancellation, disabled collection, malformed input, and no prompt/body retention.

Validation currently passes 73 Rust tests, 24 UI component tests, eight Node tests, frontend build, strict clippy, and plan lint.

These automated/synthetic checks do not establish that the current Codex desktop has loaded and trusted the handlers. Actual desktop hooks, Windows execution/stdin forwarding, and complete final-success/failure evidence remain unverified. AC1/AC6 are not marked complete from fixtures. The official reference is [Codex Hooks](https://developers.openai.com/codex/hooks); the installed protocol schema confirms all eight event names and command handler fields.

## Local Evidence

Inspection on 2026-09-26 used installed `codex-cli 0.155.0-alpha.16.4`. This is a version-specific baseline, not a compatibility promise for all Codex versions.

- The installed CLI can generate App Server JSON Schema. Generated artifacts live under `.local/harness/codex-schema/` and are not product dependencies.
- `thread/status/changed` carries `notLoaded`, `idle`, `systemError`, or `active`; active flags include `waitingOnApproval` and `waitingOnUserInput`.
- `turn/started` and `turn/completed` are present. Turn status values include `completed`, `interrupted`, `failed`, and `inProgress`.
- `thread/read` supports `includeTurns: false`; metadata-only reads avoid requesting full history. Discard any unnecessary preview/content fields returned with metadata.
- The default app-server control socket was absent. Therefore existing desktop sessions cannot yet be assumed observable through a shared live App Server connection. Starting an unrelated server would not prove visibility into the existing application.
- A read-only inspection of event types in one recent session file found `task_started` and `task_complete`. No waiting/approval event type appeared in that sample. Only aggregate event-type names/counts were examined; no transcript was copied into the repository.
- Fetching the official App Server page at https://developers.openai.com/codex/app-server failed with HTTP 403 in this environment. Protocol findings above come from the installed schema, not a successful documentation fetch.

## Adapter Contract

Prefer a supported, accessible connection to the server that actually owns the monitored threads. Luma should use native local IPC/network transport; schema generation through the installed CLI is development-only. Verify transport discovery, authentication, observer permissions, subscription behavior, and cross-platform availability before committing to live coverage.

| Codex evidence | Luma interpretation |
| --- | --- |
| Active with waitingOnApproval or waitingOnUserInput | Waiting for input; approvals are resolved in Codex |
| Active without waiting flags, or explicit turn start | Running for the current turn |
| Explicit current-turn status completed | Completed for that turn |
| Explicit current-turn status failed | Failed |
| Explicit current-turn status interrupted | Canceled/interrupted, without assuming who canceled it |
| Idle alone | No proof of completion; use validated current-turn evidence, otherwise Unknown |
| Not loaded, inaccessible source, or disconnected transport | Unknown unless trustworthy terminal evidence is already available |
| systemError | Source error; do not fabricate a failed turn |

For session-file fallback, incrementally parse supported structured event envelopes and retain only identifiers, timestamps, project/title metadata, and normalized status. Handle partial lines, truncation/rotation, duplicates, version changes, and initial reconciliation. Do not copy conversation bodies. Validate the exact task/turn identifiers and completion semantics before mapping `task_started` / `task_complete`. File silence and process presence never prove completion or waiting.

Waiting-state support is still unverified for existing desktop sessions. Do not substitute heuristic waiting detection, report full P0 success, or silently reduce the approved scope. Local JSONL evidence currently establishes candidate start/end signals only.

## Remaining P0 Checks

1. Identify a supported observation transport for the user's existing Codex sessions, or demonstrate sufficient versioned session events without requiring additional helpers.
2. Observe an actual running → waiting for input/approval → resumed → completed sequence in a user-controlled Codex task. Do not generate a new billable task or approve an action merely for this test.
3. Validate current-turn identity, ordering, reconnect, multiple simultaneous sessions, failure/interruption, and unavailable-source behavior. Use sanitized protocol fixtures for repeatable parser checks.
4. Record supported Codex versions and macOS/Windows differences. Keep AC1 and Step 1 incomplete until the required live sequence is verified.

## Extended Observer Investigation

A bounded independent inspection found no named Unix listener or TCP listener on the existing desktop Codex app-server process; it communicates through unnamed socket pairs. The desktop host owns a private IPC socket, but the inspected protocol schema/help does not establish it as a supported App Server observer endpoint. No connection to that private socket was attempted and no task lifecycle or application configuration was changed.

Three recent session files confirmed start/end events without a waiting/approval event type. Completion records omit `error` on sampled successful runs and include an error object on a sampled failed run. This is version-specific local evidence, not a universal file-format guarantee.

The development diagnostic in `tools/inspect-codex-events.mjs` projects only turn ID, timestamp, event type, and normalized status internally; output contains aggregate counts and last recorded status only. It never promotes file silence to live status. Tests cover content stripping, explicit start/end transitions, stale/duplicate events, old-run completions, malformed input, and terminal evidence without a captured start. Production incremental reading, live observation, and waiting/resume validation remain pending.

P0 cannot yet satisfy waiting-state acceptance for existing desktop conversations. The approved plan allows independent desktop foundation work while this dependency remains unresolved; neither P0 nor the full MVP is marked complete.

## Official Documentation Recheck

The user's local proxy made the official App Server documentation accessible on 2026-09-26: https://developers.openai.com/codex/app-server. The fetched page confirms `thread/read` is a read without resuming and returns runtime status, and `thread/status/changed` exposes active waiting flags. It does not establish a public observer endpoint for the currently running desktop process. The native transport availability gap therefore remains; no private IPC connection or task takeover was attempted.

## Observer and Local Metadata Audit

A further bounded read-only check on 2026-09-26 kept the same installed CLI baseline (`0.155.0-alpha.16.4`) and narrowed the remaining transport and storage questions:

- `codex app-server daemon version` failed with `No such file or directory` for the documented default control socket, `~/.codex/app-server-control/app-server-control.sock`. This checks the existing managed server without starting one. The `agents --help` command describes browsing sessions on that shared daemon; `app-server proxy --help` describes forwarding stdio to the control socket. Neither establishes access to the separately running desktop server. Only help was invoked for `agents` and `proxy`.
- The installed default schema and a freshly generated `--experimental` schema both expose `thread/read`, `thread/list`, `thread/loaded/list`, and `thread/unsubscribe`, but no standalone `thread/subscribe` or observer request. This is not proof that metadata polling or broadcast status notifications cannot work on an accessible server; the missing connection to the server owning the existing desktop threads remains the first blocker. The temporary experimental schema was deleted after inspection.
- The [official App Server documentation](https://developers.openai.com/codex/app-server) confirms that `thread/read` reads without resuming and returns runtime status. It documents stdio, Unix sockets, disabled local transport, and experimental/unsupported TCP WebSocket transport. It does not establish a discovery mechanism for an already running desktop server using unnamed socket pairs. The [official CLI reference](https://developers.openai.com/codex/cli/reference) describes explicit remote endpoints rather than discovering that desktop transport.
- A schema-only read of the installed `state_5.sqlite` found no persisted runtime-status, waiting-state, or current-turn-status column on `threads`, and no dedicated turn/status/event table. The similarly named `thread_spawn_edges.status` is not a verified conversation status source; the inspected aggregate contained only `open`. No thread rows, prompts, previews, credentials, or conversation bodies were selected or copied. SQLite was opened in read-only mode with `query_only` enabled.

These checks confirm that local metadata storage does not close the waiting-state gap for this installed version. Future work needs an accessible supported endpoint owned by the existing desktop sessions, or a validated structured event source that includes waiting and resume transitions. Do not implement a production database-status guess, automatically start a daemon, change Codex configuration, or connect to private desktop IPC to bypass this dependency. P0 and AC1 remain incomplete.
