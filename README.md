# Luma

A local desktop dashboard for AI usage and agent activity, built with Tauri 2, React, TypeScript, and Rust.

## Current Status

The desktop scaffold contains an explicitly labeled demo preview and disconnected states. It does not yet connect to GitHub or display live Codex sessions. Native OAuth feasibility has been verified separately with Luma's own client ID and no extra scopes. Existing Codex desktop waiting-state observation remains unresolved; see [the integration findings](docs/integrations/codex.md).

## Development

Install Node.js and the Tauri platform prerequisites (Rust and the native build tools for the target OS). GitHub CLI and jq are not application dependencies.

```bash
npm ci
npm run dev
npm run build
npm test
npm run tauri -- dev
```

The browser preview runs at http://127.0.0.1:1420. The desktop scaffold adds a tray with Show and Quit, close-to-tray behavior, and an always-on-top toggle. Window-position persistence, native authentication, usage storage, live session integration, launch at login, signing, and platform validation are pending.

## Codex Metadata Diagnostic

A development-only Node script inspects explicit start/completion metadata from one selected JSONL session, without printing titles, paths, messages, tool arguments, or error contents:

```bash
node tools/inspect-codex-events.mjs /absolute/path/to/session.jsonl
```

It reports historical evidence and always labels current live status as unknown. It does not detect waiting state or serve as the production adapter. Its supported event shape is version-specific.

## Configuration and Privacy

`config/github-oauth.json` contains Luma's public OAuth client ID, never a client secret. Private probe state, generated protocol schemas, and local toolchains belong under ignored `.local/`; never commit authentication tokens or conversation bodies.

The approved implementation plan is [Luma Desktop Dashboard MVP](docs/plans/active/2026-09-26-luma-desktop-mvp.md). Steps remain incomplete until their acceptance criteria are verified on the required platforms.

## Validation So Far

Frontend type checking and production bundling pass. Five focused metadata-parser tests pass, and a real local session was inspected with aggregate-only output. The Tauri configuration is recognized by its CLI. Native compilation and real window/tray behavior are not yet verified: the host initially had no Rust toolchain, and a local-only installation attempt was stopped after a slow download; partial files remain under `.local/harness/toolchains/`. No global PATH or shell startup files were changed. Windows validation is pending.
