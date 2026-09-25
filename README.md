# Luma

A local desktop dashboard for AI usage and agent activity, built with Tauri 2, React, TypeScript, and Rust.

## Current Status

The desktop scaffold contains an explicitly labeled demo preview and disconnected states. The native app now includes GitHub device sign-in, secure credential storage, and direct Copilot usage requests. Browser preview cannot invoke native authentication, and live Codex sessions are not connected. Native OAuth feasibility has been verified separately with Luma's own client ID and no extra scopes. Existing Codex desktop waiting-state observation remains unresolved; see [the integration findings](docs/integrations/codex.md).

## Development

Install Node.js and the Tauri platform prerequisites (Rust and the native build tools for the target OS). GitHub CLI and jq are not application dependencies.

```bash
npm ci
npm run dev
npm run build
npm test
npm run tauri -- dev
```

The browser preview runs at http://127.0.0.1:1420. The desktop scaffold adds a tray with Show and Quit, close-to-tray behavior, and an always-on-top toggle. Native GitHub sign-in uses the system browser and OS credential store. Window-position persistence, usage history storage, live session integration, launch at login, signing, and full platform validation are pending.

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

Frontend type checking and production bundling pass. Five focused metadata-parser tests pass, and a real local session was inspected with aggregate-only output. The Tauri configuration is recognized by its CLI. Using the user-provided vpn proxy completed the repository-local Rust installation. Native compilation passes, and a macOS Luma process with a visible window was confirmed. Complete window/tray interactions and the new native sign-in UI still require end-to-end validation. No global PATH or shell startup files were changed. Windows validation is pending.

## Repository-local Rust Toolchain

For this checkout, select the local toolchain without changing global shell configuration:

```bash
export CARGO_HOME="$PWD/.local/harness/toolchains/cargo"
export RUSTUP_HOME="$PWD/.local/harness/toolchains/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
cargo test --manifest-path src-tauri/Cargo.toml
```

The user's `vpn` alias can be enabled in an interactive shell for downloads. Keep localhost out of the proxy (`no_proxy=127.0.0.1,localhost`) when running the development app. Proxy addresses and shell setup are user environment details, not application requirements.
