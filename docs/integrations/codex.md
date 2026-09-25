# Codex Conversation Integration

Codex is the first local agent, selected by the user. Prioritize observing existing local Codex conversations, including desktop sessions; do not launch or take over tasks to make monitoring work. Luma must not approve requests, answer prompts, resume threads, or start another daemon implicitly.

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
