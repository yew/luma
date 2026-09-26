# Development and Validation Packages

Luma is a local Tauri 2 desktop application. Node.js, npm, Rust, and native build tools are development dependencies. Installed users do not need GitHub CLI, jq, Node.js, or Rust. Native GitHub authorization uses Luma's public OAuth client ID and the OS credential store.

## Prerequisites

Use Node.js 24, npm, and a stable Rust toolchain with `rustfmt` and `clippy`. Install the [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/):

- macOS: Xcode Command Line Tools (or Xcode).
- Windows: Microsoft C++ Build Tools with the Desktop development with C++ workload and Windows SDK, Microsoft Edge WebView2, and the MSVC Rust toolchain.

Build each installer on its target OS. CI defines Apple Silicon and Intel macOS jobs plus a Windows x64 job. See the [support and validation matrix](validation.md) for evidence and remaining checks; build configuration alone is not a supported-platform claim.

## Run Locally

From the repository root:

```bash
npm ci
npm run tauri -- dev
```

For a browser-only preview:

```bash
npm run dev
```

The preview is at `http://127.0.0.1:1420/`. Browser preview cannot access native authentication, credential storage, or desktop commands. Demo data is labeled explicitly; a demo status does not establish live integration support.

This checkout can use its existing repository-local Rust installation without changing shell startup files:

```bash
export CARGO_HOME="$PWD/.local/harness/toolchains/cargo"
export RUSTUP_HOME="$PWD/.local/harness/toolchains/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
```

The user's optional `vpn` command configures shell proxies for downloads. It is an environment convenience, not an application dependency. Use `no_proxy=127.0.0.1,localhost` when running the development app with a proxy.

## HTTP Proxy

Open Settings → Proxy, choose HTTP proxy, enter a server such as http://127.0.0.1:7890, and select Save proxy. Settings persist across restarts and apply to subsequent native GitHub device authorization, account validation, and Copilot requests. An in-flight request finishes using its original connection (bounded by the existing timeout). Existing refresh cooldown/backoff is preserved.

Environment proxy preserves the HTTPS client's existing environment-variable proxy behavior (HTTP_PROXY, HTTPS_PROXY, ALL_PROXY and NO_PROXY). OS proxy discovery is not enabled in this build. Direct connection explicitly disables automatic proxies. HTTP proxy explicitly selects the configured server; HTTPS origins use CONNECT and retain certificate verification. The system browser's GitHub verification page uses the browser's own network configuration.

Proxy URLs support HTTP, DNS/IPv4/IPv6 hosts, and an optional port (default 80). Username/password authentication, URL paths, query strings, and fragments are unsupported and rejected; no proxy credentials enter preferences. Invalid settings are not saved, and a storage failure keeps the existing active client. Saving validates configuration rather than claiming connectivity. A failed connection preserves the last valid usage and never silently falls back to a direct request.

Proxy validation: local HTTP CONNECT tests cover GitHub API and OAuth routing, and verify a failed settings save keeps the prior client. Preference tests cover legacy defaults and persistence. The full suite now passes 39 Rust tests and six Node tests, plus frontend build and strict clippy. Real proxy authentication is outside this feature.

## Checks

```bash
npm ci
npm run build
npm test
npm run test:ui
cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --locked --all-targets
cargo clippy --manifest-path src-tauri/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --locked --all-targets
```

`npm run build` checks TypeScript and bundles the frontend. `npm test` runs Node domain/diagnostic tests and Vitest component interaction tests. The component suite uses a mocked native bridge and fake clocks; it is not native-device acceptance evidence. Rust checks cover the native backend. Desktop interaction, authorization, and recovery require the manual checks in [validation.md](validation.md).

## Unsigned Validation Packages

These commands build local testing packages without developer signing or notarization. The frontend build runs automatically before native compilation. Run only the command for the current host; the macOS command defaults to that host's architecture.

macOS:

```bash
npm run tauri -- build --ci --no-sign --bundles dmg -- --locked
```

Windows (PowerShell):

```powershell
npm run tauri -- build --ci --no-sign --bundles nsis -- --locked
```

Local outputs are `src-tauri/target/release/bundle/dmg/*.dmg` and `src-tauri/target/release/bundle/nsis/*-setup.exe`. Passing `--target <triple>` puts outputs under `src-tauri/target/<triple>/release/bundle/` instead. For macOS, open the DMG and copy Luma to Applications; for Windows, run the NSIS setup executable. Test installation, launch, upgrades, and uninstall on a disposable test account or device. An unsigned package may be blocked by OS trust checks; record the result without claiming distribution readiness.

The Windows bundle uses Tauri's default WebView2 bootstrapper mode, which may need internet access when the runtime is absent. Luma's normal provider connection also requires internet access. No offline Windows installation claim has been validated.

[Desktop validation](../.github/workflows/desktop.yml) runs on pull requests, pushes to `main`, and manual dispatch. Each job installs locked npm dependencies, checks/builds the frontend, checks/tests Rust, and builds a target-specific installer with `--no-sign`. Only installer files are uploaded, as `luma-unsigned-<platform>-<commit>` workflow artifacts retained for 14 days. The workflow has read-only repository permissions, takes no signing secrets, and creates no GitHub Release. No CI execution or package installation is implied by adding this workflow.

## Signing Prerequisites for Later Distribution

Signing credential acquisition and public distribution are outside the approved implementation scope. The checked-in validation workflow intentionally does not configure signing. A later authorized signing pipeline needs the following external credentials and verification. Keep private keys, passwords, and temporary signing files out of git, application resources, and logs.

### macOS

A Developer ID Application certificate and its private key require access to the appropriate Apple Developer team. Tauri can use `bundle.macOS.signingIdentity` or `APPLE_SIGNING_IDENTITY`; a CI import uses `APPLE_CERTIFICATE` (base64-encoded P12) and `APPLE_CERTIFICATE_PASSWORD`. Notarization additionally requires either `APPLE_API_ISSUER`, `APPLE_API_KEY`, and a temporary `.p8` file identified by `APPLE_API_KEY_PATH`, or `APPLE_ID`, an app-specific `APPLE_PASSWORD`, and `APPLE_TEAM_ID`. These are alternatives, not secrets to embed in the application. See [Tauri macOS signing](https://v2.tauri.app/distribute/sign/macos/) and [environment variables](https://v2.tauri.app/reference/environment-variables/).

Use a temporary keychain on CI and remove it and private-key files afterward. A signing build must omit `--no-sign` and validate hardened runtime, notarization completion, and the stapled ticket. Verify the actual packaged app and DMG, including Gatekeeper launch on a clean device; an imported certificate or successful upload to Apple alone is insufficient.

### Windows

Obtain an Authenticode code-signing identity and authorized access to its private key through the issuer's supported certificate store, hardware device, or cloud signing service. Tauri's built-in signing uses `bundle.windows.certificateThumbprint`, `digestAlgorithm`, and `timestampUrl`; a provider-specific workflow can instead configure `bundle.windows.signCommand`. The Windows SDK supplies `signtool`. See [Tauri Windows signing](https://v2.tauri.app/distribute/sign/windows/).

Do not assume the key is exportable or that a PFX password is sufficient for every issuer. Configure only the selected provider's required credentials in a separately authorized signing job, omit `--no-sign`, timestamp signatures, and verify both installed executables and the final installer. A valid signature does not guarantee immediate SmartScreen reputation.

## Integration Setup and Local Data

`config/github-oauth.json` contains the public client ID. Device Flow must be enabled by the OAuth application's maintainer. End users select Connect GitHub, open the trusted GitHub verification page, and enter the displayed code. The current validated account uses no extra scopes; see [GitHub integration](integrations/github-auth.md) for access and lifecycle limits. Disconnect removes Luma's local token; revoking the GitHub grant is a separate action in GitHub's application settings.

Settings now persist in `preferences.sqlite3` in the application data directory. The UI configures refresh from 60 to 3600 seconds (default 300), launch at login, and title/path privacy; pin/collapse state and window geometry also persist. Geometry changes are debounced and restoration clamps the window to available monitor work areas. Launch at login reads the OS registration at startup and follows explicit user toggles. Installed macOS login, monitor removal, resolution changes, and restart checks are recorded in [macOS acceptance](validation/macos-2026-09-26.md). The user canceled dedicated 1×/2× transition, isolated clean-user installation, and long-duration performance tests; Windows functional checks remain separately required.

Usage history persists in usage.sqlite3 with immutable exact-decimal observations, bounded paginated queries, migrations, and independent latest projections. Collection defaults to enabled with 90-day retention; settings provide disablement and separate history/cache deletion. Backend tests cover these contracts; installed-app and Windows checks remain pending. See [the history contract](integrations/usage-history.md). Live Codex observation remains unresolved; see [Codex integration](integrations/codex.md).

Disposable logs, screenshots, and measurements belong under the local runtime root returned by `harness repo config get paths.local_runtime`. Record only sanitized metadata. Never attach tokens, device codes, conversation bodies, or private project paths to a workflow artifact or validation report.

## Compact Window and Recovery

Collapse reduces the native dashboard to 240 logical pixels high. Moving the compact window updates position while retaining the expanded dimensions. Opening Settings expands temporarily; closing Settings restores the compact view. Expanding and tray recovery clamp geometry to connected monitor work areas using the destination monitor DPI. Privacy and collapsed preferences remain persisted independently.

Usage status labels include freshness and error categories. Manual refresh shows a countdown from the backend's actual cooldown/provider deadline; switching accounts cannot inherit an earlier account's local retry fallback.

## Recovery Regression Coverage

Component tests exercise rendered Settings proxy save/failure, account-scoped history confirmation and disconnect, authentication cancellation with late replies, failed subscription recovery, remount state synchronization, backend retry deadlines, secure-storage retry, and preference startup ordering. Browser preview shows Disconnected without attempting native commands. CI runs these development-only tests through npm test; test libraries are not shipped in the application.

Native credential storage has a replaceable test boundary, with production still using only the OS credential store. Deterministic tests simulate locked/unlocked, failed save/delete, expired credentials, canceled reconnect, mismatched account identity, and retrying failed legacy metadata persistence. No test reads or modifies a developer's real credentials. Live sign-in and cross-platform credential-store interactions remain separate acceptance checks.
