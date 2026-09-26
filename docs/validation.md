# Support Matrix and Validation

This is the current evidence record and a repeatable manual checklist, not a release certification. Unchecked items remain unverified. Update each result with the tested commit, exact OS/build, CPU architecture, device, package type, and sanitized evidence location. CI results establish build/test coverage; they do not substitute for interactive device checks.

## Platform Evidence

| Platform | Architecture | Evidence | Remaining validation |
| --- | --- | --- | --- |
| macOS 26.6.2, build 25G83 | Apple Silicon / arm64 | Existing local frontend and Rust checks/native compilation passed; a visible 360 × 480 native window was observed. Current host version was read on 2026-09-26. | Frontend build, 35 Rust tests, strict clippy, and plan lint pass for the current implementation; unsigned DMG creation/integrity and launch directly from the read-only DMG pass. SQLite schema 2 and default retention initialize. Installed-app interactions, live authorization, and extended performance remain unverified. |
| macOS 15 (`macos-15` CI runner) | arm64 | Automated validation and DMG job configured; no run result recorded. | CI execution, package installation, and physical-device tests. |
| macOS 15 (`macos-15-intel` CI runner) | x86_64 | Automated validation and DMG job configured; no run result recorded. | CI execution, package installation, and physical-device tests. |
| Windows Server 2022 (`windows-2022` CI runner) | x86_64 / MSVC | Automated validation and NSIS job configured; no run result recorded. | CI execution and installer creation. Server build coverage is not Windows desktop validation. |
| Windows desktop | x86_64 | No real-device result recorded. | Record exact Windows edition/version and run the complete checklist. |

No tested minimum OS version is established. Windows ARM64 and universal macOS packages are not currently in the build matrix. Runner labels are explicit and their architectures follow the [GitHub runner image table](https://github.com/actions/runner-images); preserve the actual runner image version from every run because hosted images change. No signed/notarized package or public-distribution readiness is claimed.

## Existing Integration Evidence

- Luma's own OAuth client, empty scopes, and an enterprise Copilot account returned HTTP 200 from identity and usage endpoints during the 2026-09-26 native feasibility probe. This does not prove other account classes or policies. See [GitHub authentication](integrations/github-auth.md).
- Installed `codex-cli 0.155.0-alpha.16.4` provides candidate start/end metadata. A supported observer transport and the required live waiting/resume sequence remain unresolved. See [Codex findings](integrations/codex.md).
- Current validation on 2026-09-26: frontend type checking and bundling, six Node diagnostic/state-order tests, 35 Rust tests, strict clippy, and plan lint pass. Rust coverage includes OAuth/metric parsing, Copilot-to-storage precision, six preference tests, and thirteen history tests. Workflow actionlint and YAML validation pass. This is local automated evidence; CI execution and native interaction checks remain pending.

## Manual Test Record

Create one record per actual device/package under the local harness runtime root. Include test date, commit, OS/build, CPU model/architecture, memory, display scale, monitor arrangement, package filename, credential-store availability, and the exact tests run. Use pass/fail/blocked with a reason; keep unchecked checks incomplete. Do not record secrets or conversation content.

### Installation and Lifecycle

- [ ] Build the matching DMG or NSIS installer and record the package hash and commit.
- [ ] Install on a clean test account/device; record any unsigned-package trust prompt or block.
- [ ] Launch the installed app without Node.js, Rust, GitHub CLI, or jq.
- [ ] Confirm the default window is approximately 360 × 480 logical pixels; resize, drag, pin/unpin, and collapse/expand.
- [ ] Close the window: the process remains available from the tray. Show restores it, and Quit terminates it.
- [ ] Restart and verify persisted settings, window bounds, collapsed state, and pin preference.
- [ ] Move between differently scaled displays, unplug the current display, and restart with the old display absent. The window stays reachable.
- [ ] Enable launch at login, log out/in, then disable it and repeat. Record OS policy restrictions.
- [ ] Install an upgrade, verify retained preferences and supported database migration, then uninstall. Record which local data and credentials remain and test explicit deletion before uninstall.

### Usage, Authorization, and Persistence

- [ ] Connect from the installed app, verify the trusted GitHub URL and code, authorize, and observe real Copilot usage for the expected account.
- [ ] Cancel sign-in, deny authorization, and retry; no duplicate polling or stale request can reconnect unexpectedly.
- [ ] Validate expiry, revocation/401, policy/permission/403, rate limits/429, malformed responses, and unavailable credential storage. Use deterministic fixtures for failures that cannot be safely induced.
- [ ] Restart the app and verify the OS-stored credential is restored without exposing token/device-code data to the frontend, SQLite, or logs.
- [ ] Verify five-minute default refresh, manual cooldown, exponential failure backoff, and provider retry timing.
- [ ] Lose network access and restore it; preserve the last successful quota as stale and recover without fabricating history.
- [ ] Compare usage, allowance, fractional remaining quota, reported remaining percentage, reset timestamp, and freshness with the source response. Missing/zero/unlimited allowances remain distinct.
- [ ] Check successful polls, unchanged polls, replayed collections, failed polls, cache-only reads, and restart against persisted observation counts and latest projection.
- [ ] Query bounded time ranges/pages and test reset, decrease, gap, plan, unit, and semantics boundaries. Do not infer consumption across uncertain continuity.
- [ ] Exercise the 90-day default, configured retention, history collection disablement, independent cache/history clearing, and per-account/global deletion.
- [ ] Disconnect and switch between two authorized accounts; verify credential deletion and no cross-account cache/history leakage. Local disconnect must not claim remote grant revocation.

### Conversations and Privacy

These checks are blocked until a supported production Codex observation source is validated. Demo or diagnostic output cannot satisfy them.

- [ ] Observe real running → waiting → resumed → completed transitions on an existing user-controlled task.
- [ ] Exercise explicit failure/cancellation, duplicates, out-of-order events, overlapping runs, disconnection, and restart recovery.
- [ ] Confirm silence and process exit never become completion; unavailable evidence shows Unknown.
- [ ] Waiting precedes running; recent completed sessions are grouped; collapsed counts and the selected metric match the full view.
- [ ] Toggle privacy settings and verify hidden titles/paths remain hidden after restart. No conversation bodies are persisted or logged.

### Recovery and Accessibility

- [ ] Sleep and wake with an active connection, an in-flight refresh, and a pending device authorization; verify bounded recovery and correct expiry.
- [ ] Change network/proxy availability while running; errors stay local to the affected integration.
- [ ] Confirm keyboard access, visible focus, accessible button labels, readable text, and status text/icons independent of color at supported display scales.
- [ ] Verify loading, disconnected, healthy, stale, permission, request-failure, unsupported, and demo states are visibly distinct.

## Performance Protocol

Performance acceptance remains unmet until measurements are recorded on documented devices. Establish and justify an explicit memory budget from the native desktop baseline before evaluating it; no budget or result is invented here.

1. Use an installed release build with debug tooling closed. Record device, power mode, displays, account count, session count, refresh interval, and background workload.
2. Allow five minutes to settle, then sample idle process CPU and memory once per second for ten minutes. Normalize CPU as CPU seconds divided by wall seconds, multiplied by 100, so 100% represents one fully occupied logical core. Include child processes belonging to the app when present; document metric/collection differences between Activity Monitor and Windows tooling.
3. Record median/95th-percentile CPU, total CPU time, and steady memory. AC9 requires steady idle CPU below 1% of one core. Record any short refresh spikes separately.
4. Once live observation is available, measure at least 30 source-event-to-render intervals with a monotonic clock and report median, 95th percentile, and maximum. The plan expects typical hook-driven updates within one second; do not substitute polling or demo timings.
5. Run at least eight hours with ordinary refreshes and representative network loss, sleep/wake, and session activity. Compare memory after equivalent settling periods at start/end and inspect resource counts for unresolved growth. Archive sanitized measurements and a concise result with the measured memory budget.

## Distribution Gate

Before any later public distribution, separately authorize the release and satisfy all required platform/integration checks. Use the [signing configuration and external credential checklist](development.md#signing-prerequisites-for-later-distribution), then verify signatures, Apple notarization/stapling, installer trust behavior, and clean-device launch for the actual packages. Unsigned CI artifacts remain validation packages even when every automated job is green.
