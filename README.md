# Luma

A local desktop dashboard for AI usage and agent activity, built with Tauri 2, React, TypeScript, and Rust.

## Current Status

The current MVP targets macOS. Windows implementation and build configuration are retained for a later release; Windows validation does not block this candidate.

The native app includes GitHub device sign-in, secure credential storage, and direct Copilot usage requests. The desktop shell provides a floating window and tray; browser preview includes explicitly labeled demo and disconnected states. Browser preview cannot invoke native authentication.

Native OAuth feasibility was verified with Luma's own client ID, no extra scopes, and an enterprise Copilot account. Codex Hooks monitoring is implemented: enable it in Settings, trust the hooks in Codex, and receive metadata for subsequent session events. Stop is shown as Turn stopped rather than verified completion; real macOS Waiting and Turn stopped behavior has been validated. See [GitHub authentication](docs/integrations/github-auth.md) and [Codex integration findings](docs/integrations/codex.md).

Persistent settings now cover refresh interval, launch at login, title/path privacy, pin/collapse state, and window geometry. Core macOS persistence/recovery checks passed; Windows device validation is deferred. SQLite usage history now preserves exact decimal observations, supports paginated queries, and provides retention/deletion controls. See [usage history](docs/integrations/usage-history.md). The approved [MVP plan](docs/plans/archived/2026-09-26-luma-desktop-mvp.md) has completed implementation acceptance and independent review and is archived for merge handoff.

## Development

Install Node.js 24, Rust, and the native build prerequisites for the target OS. GitHub CLI and jq are not application dependencies.

```bash
npm ci
npm run tauri -- dev
```

For the browser preview at `http://127.0.0.1:1420/`, use `npm run dev`. See [development instructions](docs/development.md) for prerequisites, local Rust configuration, proxy setup, checks, and unsigned DMG/NSIS builds.

## Validation and Packages

The [desktop workflow](.github/workflows/desktop.yml) defines macOS Apple Silicon/Intel and Windows x64 checks and unsigned installer artifacts. It does not publish releases or use signing credentials. Workflow configuration is not evidence of a successful run or installation.

Existing local frontend and native macOS compilation checks passed and a visible native window was observed. Core macOS installed-app, sleep/wake, monitor, network, and login-start checks passed. The approved five-state functional acceptance is complete; final review and release handoff remain; canceled device/performance tests and deferred Windows work are outside this candidate. The [support matrix and manual checklist](docs/validation.md) distinguish actual evidence from unverified targets; signing prerequisites are documented separately from unsigned validation builds.

## Codex Metadata Diagnostic

A development-only script inspects explicit start/completion metadata from a selected JSONL session without printing titles, paths, messages, tool arguments, or error contents:

```bash
node tools/inspect-codex-events.mjs /absolute/path/to/session.jsonl
```

It reports historical evidence and always labels current live status as unknown. It does not detect waiting state or serve as the production adapter. Its supported event shape is version-specific.

## Configuration and Privacy

`config/github-oauth.json` contains Luma's public OAuth client ID, never a client secret. Tokens belong only in the OS credential store. Private probes, generated schemas, and local toolchains belong under ignored `.local/`; never commit credentials or conversation bodies.
