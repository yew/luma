---
template_version: 0.3.0
created_at: "2026-09-26T00:06:58+08:00"
approved_at: "2026-09-26T00:38:07+08:00"
source_type: direct_request
source_refs: []
size: XL
---

# Luma Desktop Dashboard MVP

## Goal

Deliver Luma, a lightweight local desktop dashboard for individual developers on macOS and Windows. Users can inspect AI usage and local agent activity from a floating window without opening each provider or agent application.

Deliver one bounded MVP candidate: one validated usage provider, one validated agent, a desktop shell, and installable packages. Live integration feasibility is the first dependency, not an assumed capability.

Implementation is underway from an initially empty workspace. Native Copilot access has been validated; complete Codex waiting-state observation and cross-platform device acceptance remain unresolved.

### Decisions and Constraints

- Use Tauri 2, React, TypeScript, Vite, CSS variables/component styles, and Rust with Tokio. Use SQLite for settings and cached metadata and the OS credential store for secrets.
- Keep the application local and single-user, with no custom cloud service. The backend owns credentials, network access, and local file reads; expose only necessary data through minimally privileged Tauri commands/events.
- Use GitHub Copilot GET /copilot_internal/user through native HTTPS requests and Luma-managed GitHub authorization as the first usage source; validate the contract below in P0. Organization statistics must not substitute for personal subscription allowances. Codex is the first local agent, selected by the user; prioritize observing existing desktop/local conversations using supported events or version-identifiable structured session data.
- P0 must establish the account type, authentication method, supported metrics, first agent, and state-event coverage before dependent live integration implementation. If credentials, account access, or agent selection are unavailable, document the gap and obtain the missing input; do not claim validation from fixtures.
- Provider substitution or reduction of required live functionality is a material scope change requiring an updated plan and human steering. Independent desktop work may proceed while an integration dependency is unresolved, but the MVP cannot be marked complete.
- Prefer supported hooks/events/SDKs, then version-identifiable local structured files. Process observation is supplementary. If a local HTTP event endpoint is necessary, bind to loopback and authenticate every request.
- Store recent session metadata, the latest usage projection, and bounded local usage history by default; retain usage observations for 90 days initially with retention and deletion controls. Do not copy conversation bodies, scrape browser pages, or reuse another application's login cookies.
- Keep tracked documents and code in English. This is a standard XL plan covering the original P0–P4 MVP; do not introduce lightweight or coordinated workflow profiles.
- Plan creation is not implementation approval. Record explicit human approval through the harness before execution. Review, archive, publish, and merge follow the repository lifecycle rather than implementation steps.

### Current Implementation Status

- Native GitHub OAuth feasibility passed with Luma's own client ID, no additional scopes, and HTTP 200 from identity and Copilot endpoints; see `docs/integrations/github-auth.md`. Native device authorization, keychain/credential-manager storage, direct Copilot requests, and a sign-in UI are implemented; production end-to-end sign-in and Windows validation remain pending.
- Codex start/completion/failure metadata can be inspected without retaining message content. Existing desktop waiting-state observation is unresolved: no supported listener was found on its owning App Server. See `docs/integrations/codex.md`; AC1 and Step 1 remain incomplete.
- Independent desktop foundation work provides a React demo/disconnected UI and a Tauri tray/window scaffold. Frontend build and five focused diagnostic tests pass. Proxy-assisted Rust installation and native macOS compilation now pass. A running 360 × 480 Luma window at the floating window layer was observed. Full window/tray interaction checks and Windows validation remain pending; Step 2 is not complete.

- Usage persistence now stores immutable account/metric observations and collection outcomes in SQLite, with exact decimal text, schema migrations, replay protection, time-range pagination, rebuildable latest cache, and conservative continuity boundaries. Retention defaults to 90 days with collection disablement and separate account/global history/cache deletion; cleanup runs on collection, settings changes, startup, and hourly while offline.
- Automatic usage refresh now runs in the Rust backend, including while the dashboard is hidden. Persisted intervals range from 60 to 3600 seconds (default 300); manual cooldown is 30 seconds and failure backoff starts at 10 minutes with a one-hour cap, respecting longer provider delays. Frontend subscriptions and focus resynchronization consume backend state.
- User-requested HTTP proxy support adds persistent environment, direct, and explicit HTTP modes in Settings. Saved settings apply to subsequent OAuth and usage requests without restart; URL validation rejects credentials, TLS verification stays enabled, and local CONNECT tests verify routing and failed-save rollback.
- SQLite preferences persist privacy, pin/collapse, refresh interval, and window geometry; launch-at-login uses the native plugin. Monitor/DPI recovery logic is tested; native login, disconnected-monitor, and Windows interaction checks remain pending.
- Cross-platform CI is configured to validate macOS ARM64/Intel and Windows x64 and prepare unsigned DMG/NSIS artifacts. Local unsigned Apple Silicon DMG generation, integrity verification, and packaged-process launch succeed; 35 Rust tests, six Node tests, frontend build, and strict clippy pass. CI execution, Windows packages/device testing, live Codex event coverage, and extended performance checks are still required. No complete MVP, signing, or release-readiness claim is made.

- Refresh scheduling now honors Retry-After seconds/HTTP dates, rate-limit resets, and usable Cache-Control/Age guidance. Cache-only/304 responses retain original observation timestamps and do not add history. The frontend displays backend retry deadlines and separates loading, stale, permission, unsupported, rate-limit, and disconnected states. This checkpoint passes 47 Rust tests, eight Node tests, frontend build, strict clippy, and plan lint; complete MVP acceptance remains pending.
- Compact mode now resizes the native window to 240 logical pixels high, preserves expanded dimensions, temporarily expands for Settings, and clamps restored geometry using the destination monitor DPI. Deterministic geometry checks pass; native multi-display interaction validation remains pending.
- A 120-second macOS release baseline measured native-parent CPU at 0.042% of one core and maximum RSS at 104.27 MiB; provisional parent RSS budget is 160 MiB. Unattributed WebKit helpers, short settling, and unknown UI configuration prevent full AC9 acceptance. See docs/validation.md.
- A new read-only Codex audit confirms the default daemon control socket is absent and local metadata has no runtime/waiting status. Existing desktop waiting/resume observation still needs a supported owning endpoint; no production status is inferred from private IPC or incomplete files.

### Delivery Estimates

These are rough estimates for one developer familiar with the stack, subject to integration validation. Signing credential applications and external review time are additional.

| Phase | Estimate | Deliverable and acceptance criteria |
| --- | --- | --- |
| P0: Integration validation | 2–3 days | Retrieve usage from a real account; trigger running, waiting, and completion events from one agent; document authorization and unsupported capabilities. Adjust scope first if these cannot be achieved |
| P1: Desktop foundation | 2–3 days | Launch on macOS and Windows; working floating window, tray, close/quit behavior, layout, and demo data flows |
| P2: Live usage | 2–4 days | Complete the first provider's authentication, historical observations, latest projections, range queries, retention controls, polling, secure credential storage, and stale/error states |
| P3: Live conversations | 3–5 days | Complete the first agent integration; verify waiting-to-resume transitions, duplicate and out-of-order events, cancellation, restart recovery, and disconnection |
| P4: Release preparation | 3–5 days | Installers for both platforms, real-device validation, extended runtime checks, installation and integration documentation, and signing before public distribution |

The original first-release estimate is approximately 3–4 weeks; re-estimate the live-usage phase after assessing native authentication feasibility and the added history storage and retention work. Commit to live-data features only after P0 succeeds or the scope has been explicitly revised.

The P0–P4 phases map to Steps 1–5 below. These estimates describe implementation outcomes, not harness review, publication, or merge milestones.

## Scope

### In Scope

- Verify a real usage source and real running, waiting, and completion signals from one local agent; document capabilities and unsupported cases.
- Provide native GitHub sign-in and direct API access without a GitHub CLI, jq, or external helper dependency.
- Build a resizable floating dashboard, tray integration, settings, and separate provider/agent adapters.
- Display usage with metric, unit, reporting period, available allowance, freshness, and reset information. Persist time-stamped observations with series identity and reset/continuity metadata to support future usage trends.
- Display sessions with source, title, project, status, and activity time; maintain trustworthy state across retries, duplicate events, new runs, and restarts.
- Support a persistent Settings proxy mode (environment, direct, or HTTP proxy), with a validated HTTP server URL applied to new native OAuth and usage operations without restart. Keep TLS verification enabled; reject credentials in the URL instead of persisting proxy secrets in plaintext.
- Support secure credentials, refresh controls, local persistence, privacy controls for hiding project paths or session titles, and explicit demo/error/unsupported states.
- Prepare macOS DMG and Windows NSIS packages, build automation, installation/integration documentation, and documented platform validation.

### Out of Scope

- Starting or controlling agent tasks, conversation-body browsing, cloud sync, and team management.
- Multiple live providers or agents beyond the first validated integration of each type.
- Browser scraping, external login-cookie access, a general-purpose plugin loader, click-through windows, desktop-layer embedding, and forced overlays above full-screen apps.
- Historical analytics UI, forecasts, complex charts, automatic updates, and completion notifications. History collection, storage, retention, and bounded range queries are in scope.
- Buying signing credentials, changing external account settings, or publicly distributing installers as part of this implementation approval.

### Technology Stack

| Component | Choice | Purpose |
| --- | --- | --- |
| Desktop shell | Tauri 2 | Windows, tray, application lifecycle, and OS integration |
| UI | React + TypeScript + Vite | Cards, lists, and settings |
| Styling | CSS variables and component styles | Consistent themes and compact layouts with minimal initial dependencies |
| Backend | Rust + Tokio | Integrations, asynchronous requests, normalized state, and event delivery |
| Local storage | SQLite | Settings, recent session metadata, immutable usage history, and latest-state projections |
| Credentials | macOS Keychain / Windows Credential Manager | Store user-authorized integration credentials |
| Validation and builds | Rust tests, Vitest, GitHub Actions | Core logic tests and builds for both platforms |

The backend handles credentials, network access, and local file reads. The frontend receives necessary data through Tauri commands and events with minimal permissions. A general-purpose plugin loader is unnecessary for the first release.

### Interface Design

The default floating window is approximately 360 × 480 logical pixels and can be resized.

1. Header: Luma, connection overview, always-on-top control, collapse control, and settings.
2. Usage section: provider, metric name, consumed / total allowance, progress bar, and reset time. Hide percentages when allowance data is unavailable.
3. Conversations section: session title, project, agent type, and status text and icon. Prioritize sessions waiting for input, then running sessions, with recently completed sessions grouped below.
4. Footer: last refresh time and manual refresh. Show errors beside the affected integration without blocking the entire dashboard.

Collapsed mode shows waiting and running counts plus one user-selected usage metric. Communicate status through text, icons, and color together.

### Usage Adapters

Each provider has a separate adapter for authentication, capability discovery, snapshot retrieval, rate limiting, and error mapping. The metric model includes at least:

- series_id, provider/host, account identity, metric, unit, semantics version, aggregation kind.
- used, limit (nullable), allowance kind, reported remaining quota/percentage, plan, overage permission.
- provider period ID, period_start, period_end, reset_at (all nullable when unavailable), continuity segment and boundary metadata.
- observation_id, collection_id, fetched_at, optional observed_at, source/schema version, quality flags.
- freshness and errors belong to current state/collection outcomes; they do not rewrite historical observations.

Calculate a percentage only when used and limit share the same metric, unit, and period, and limit > 0. Handle zero allowance, unlimited allowance, missing data, and not-applicable values separately. Percentages may exceed 100% when over quota, while progress bars remain capped. Do not combine costs, tokens, and subscription request allowances into a single total.

For Copilot, use the user-selected internal endpoint below. Do not assume public API stability; isolate parsing in the Copilot adapter and verify access and field semantics in P0. Organization statistics cannot replace the authenticated user quota, including enterprise-plan users. Endpoint or schema failures must preserve the last valid snapshot and surface an integration error. Provider substitution or reduced functionality follows the scope-change rule above. Demo data does not count as completed integration.

Start with a five-minute polling interval, respecting API rate limits and caching guidance. Apply a cooldown to manual refresh and exponential backoff after failures. Preserve the last successful snapshot and mark it stale; failed refreshes must not reset usage to zero.

#### Usage History and Trend Compatibility

The MVP collects historical observations from the first successful fetch so that a later trend UI can use existing data. Storing only the latest snapshot would discard that information. Keep the latest-state cache as a rebuildable projection; it is not the history source of truth. Charts, forecasts, and historical dashboards remain deferred.

Persist these logical records in SQLite with schema-versioned migrations:

| Record | Required data and semantics |
| --- | --- |
| Usage series | Stable `series_id`, provider/host, account identity, metric, unit, unit/metric semantics version, aggregation kind (`cumulative_counter`, `gauge`, or `interval_total`) |
| Usage observation | `observation_id`, `collection_id`, `series_id`, `fetched_at` in UTC, optional provider `observed_at`, `used`, nullable `limit`, explicit allowance kind (finite/unlimited/unknown/not-applicable), reported remaining quota/percentage, plan, overage permission, source/schema version, quality flags |
| Period and continuity metadata | Nullable provider period ID, period start/end, reset instant, local continuity segment ID, and boundary reason/confidence; preserve these per observation rather than overwriting history |
| Collection outcome | Attempt ID, account/integration, start/end time and status, sanitized error category, optional provider timestamp; failures contain no invented usage observation |

Scope a series by provider host and stable account ID where available, otherwise an explicitly identified login fallback, plus metric, unit, and semantics version. A plan label or quota limit is a snapshot attribute and may change; old observations retain their original values. Provider timestamps indicate observation time only when their semantics are verified; otherwise use collection time and record that basis. Do not interpret `reset_at` as observation time. Preserve decimal quantities using decimal strings or scaled integers with explicit scale, and retain unknown values as null with appropriate semantics.

A successful collection appends an immutable observation for each valid returned metric and updates the latest projection transactionally. Enforce uniqueness on `(collection_id, series_id)` so replaying one collection cannot duplicate samples. Separate successful polls with unchanged counters remain valid observations; never deduplicate solely by value. Missing/invalid metrics are marked unavailable for that attempt, without converting them to zero. Cache reuse and failed requests do not generate fresh observations. Prevent overlapping collection per integration or order updates so a late response cannot regress the latest projection.

Index history by `(series_id, fetched_at, observation_id)` and expose a bounded, paginated storage/service query for a UTC time range and series. Keep timestamped samples as the primary data; derived deltas and aggregates are rebuildable. Do not sum cumulative counter snapshots. A counter delta is meaningful only between compatible observations in a confirmed continuous segment. Copilot `credits_used` is provisionally a period-cumulative counter, subject to P0 verification; while that semantic remains unverified, raw snapshot trends are available but consumption deltas are not.

For Copilot, retain each reset instant as boundary evidence without inventing absent period start/end. A reset change, counter decrease, unit/semantics change, incompatible plan/allowance change, or ambiguous continuity starts a new segment or marks a discontinuity. Unknown cause remains unknown: do not clamp negative deltas to zero or invent post-reset usage. Do not subtract across a reset or incompatible series, and never backfill missing pre-installation data. A long gap may permit a net interval delta only if continuity is independently established; it cannot establish the timing of consumption within that gap. Future daily/hourly views must expose gaps and estimated allocation rather than presenting evenly distributed usage as observed data. Reporting-timezone boundaries are applied at query time without changing stored UTC instants.

Retain local usage history for 90 days by default, configurable in settings; this is an initial product default that may be revised. Provide an option to disable history collection and an explicit action to delete retained history per account or globally. Disabling collection stops future historical writes while the dashboard can retain its latest snapshot; deletion removes historical samples and derived data. Clearing the latest cache must not silently erase history. Disconnecting stops collection and offers an explicit choice to delete that account's retained data; reconnecting must not mix account identities. Apply scheduled retention cleanup to observations, derived data, and collection outcomes. Keep history free of credentials and conversation content, and document that the dashboard only knows the periods actually observed within retention.

#### GitHub Copilot API Contract

The following user-provided command documents the endpoint and response projection only. Luma calls the same API directly; GitHub CLI is not required:

```bash
gh api -H 'Accept: application/json' /copilot_internal/user --jq '{login, plan: .copilot_plan, reset_at: .quota_reset_date_utc, premium: (.quota_snapshots.premium_interactions | {entitlement, credits_used, quota_remaining, percent_remaining, overage_permitted})}'
```

User-provided projected response (reference data, not independently fetched during planning):

```json
{
  "login": "yew",
  "plan": "enterprise",
  "premium": {
    "credits_used": 722342,
    "entitlement": 2000000,
    "overage_permitted": true,
    "percent_remaining": 63.8,
    "quota_remaining": 1277551.1
  },
  "reset_at": "2026-10-01T00:00:00.000Z"
}
```

This is a read-only GET request with `Accept: application/json`. The `--jq` expression projects the raw response: `plan`, `reset_at`, and `premium` are aliases. The Rust backend parses raw JSON from a native HTTPS request without executing this command or using `--jq`.

| Raw API field | Normalized meaning |
| --- | --- |
| `login` | Account identity; scope caches to this account |
| `copilot_plan` | Plan label, such as `enterprise`; not organization-wide usage |
| `quota_reset_date_utc` | `reset_at`; preserve UTC and display in local time |
| `quota_snapshots.premium_interactions.entitlement` | Total allowance (`limit`) |
| `quota_snapshots.premium_interactions.credits_used` | Used quota (`used`) |
| `quota_snapshots.premium_interactions.quota_remaining` | Provider-reported remaining quota; preserve fractional precision |
| `quota_snapshots.premium_interactions.percent_remaining` | Provider-reported remaining percentage |
| `quota_snapshots.premium_interactions.overage_permitted` | Whether overage is permitted; not unlimited quota or permission for Luma to enable spending |

Use `premium_interactions` as the metric identifier. Label amounts as provider quota credits pending P0 verification of unit semantics; do not interpret them as tokens, currency, or request counts. Record fetch time locally. The sample provides reset time but no explicit reporting-period start/end; keep absent boundaries unknown rather than inventing calendar dates. Confirm a common quota window before calculating usage percentage.

For a valid positive allowance, compute used percentage as `credits_used / entitlement * 100`. The example yields `36.1171%`, displayed as approximately `36.1%`. Preserve the reported `63.8%` remaining and `1277551.1` remaining quota independently. These values do not exactly complement used quota; do not overwrite them with derived values, assume the cause, or equate `100 - percent_remaining` with calculated usage. Clearly label reported remaining and calculated used values wherever both appear.

#### Native GitHub Authentication

Luma owns the sign-in flow and sends HTTPS requests directly from its Rust backend. Users do not install GitHub CLI, jq, a scripting runtime, or a local authentication helper. The supplied `gh api` command is only a reference for the endpoint and response projection, never a runtime dependency or credential source.

The preferred sign-in mechanism is GitHub OAuth device authorization using an OAuth application registered for Luma with device flow enabled. The application client ID is public configuration; never embed a client secret or reuse GitHub CLI, VS Code, or another application's client identity. App registration is a one-time maintainer prerequisite, not a per-user developer setup step. No custom authentication server or embedded GitHub login form is required.

1. The user selects Connect GitHub. The backend requests a device code from `https://github.com/login/device/code` using Luma's client ID and the minimum scopes established during P0.
2. Show the returned user code and verification URL, with an action to open that trusted GitHub URL in the system browser. The user authenticates and grants access on GitHub; Luma never handles the account password. Keep the device code backend-only and temporary.
3. Poll `https://github.com/login/oauth/access_token` at the server-provided interval. Handle authorization pending, slowdown, denial, expiry, cancellation, and network failure explicitly. Stop on completion/cancellation/expiry, and never run duplicate sign-in loops.
4. Validate identity and access using authenticated requests to `https://api.github.com/user` and `https://api.github.com/copilot_internal/user`. An issued OAuth token alone does not prove Copilot access. Associate the verified stable GitHub account ID with the returned login and reject mismatched identity before saving usage.
5. Store the authorized token in macOS Keychain or Windows Credential Manager and expose only connection/account metadata to the frontend. If secure storage is unavailable, report the connection problem without falling back to plaintext storage.

P0 must prove that a token obtained through Luma's own OAuth application can call this internal endpoint and document the minimum accepted scopes/token type, including enterprise policy or SSO restrictions where applicable. Success through the user-supplied GitHub CLI command does not establish compatibility with an independently registered OAuth application. Do not invent a required scope or broaden permissions silently. If device-flow tokens are rejected, investigate an explicitly user-provided compatible token as a secondary native setup option; add it only after validating its access and documenting its permissions. Failure of both routes is a concrete integration blocker requiring scope steering, not a reason to restore the CLI dependency.

Use an in-process HTTPS client with TLS verification, bounded requests/responses, cancellation, and redacted diagnostics. Send `Authorization: Bearer <token>`, `Accept: application/json`, and an identifying User-Agent to the selected API endpoint. Keep authorization headers out of URLs and cross-host redirects. Parse the raw JSON in Rust without `--jq`; preserve the documented field mapping and history contract.

Persist only the token metadata actually supplied by the provider. Refresh only when the token type supplies a documented supported refresh mechanism; otherwise require reconnect on expiry/revocation. A 401 triggers reauthentication; distinguish 403 permission/policy/limit failures, 429 rate limits, and endpoint/schema errors using the available response evidence rather than treating every failure as an expired token. Keep last successful quota marked stale, never fabricate history, and stop automatic auth retries until user action is appropriate.

Disconnect cancels polling and sign-in work and deletes Luma's locally stored token. Do not claim that local deletion revokes the GitHub grant; provide guidance for revoking it in GitHub settings. History retention/deletion remains a separate explicit choice. Account switching validates a new identity before reading or writing its account-scoped cache/history.


### Conversation Adapters

The first adapter is Codex. Validate access to the server or session source that owns existing conversations; starting a separate App Server is not proof of visibility into desktop tasks. Installed protocol schemas expose explicit waiting flags, but live observer access and a complete running/waiting/resume/completion sequence remain P0 requirements. Initial findings and version-specific status mapping are maintained in `docs/integrations/codex.md`. Do not replace missing waiting signals with process or inactivity heuristics.

Prefer supported hooks, events, or SDKs, followed by local structured session files with identifiable format versions. Process observation is supplementary only. Complete an end-to-end integration for one actively used agent before expanding.

Prefer agent hooks that call a local event entry point; choose CLI or local IPC during prototyping. If HTTP is necessary, bind only to loopback and validate local credentials on every request.

Normalized events contain source, session_id, run_id, event_id, event timestamp, event type, and necessary project metadata. Identify sessions by source + session_id and individual runs by run_id. Handle duplicate and out-of-order events so that an old completion event cannot overwrite a newer run's state.

| State | Entry condition |
| --- | --- |
| Running | A run-start event or explicit execution event is received |
| Waiting for input | An input, authorization, or selection request is received |
| Completed | A successful end-of-run event is received; this does not imply the user's overall objective is complete |
| Failed | An explicit failure event is received |
| Canceled | An explicit cancellation event is received |
| Unknown | The integration cannot determine state, its data connection is lost, or trustworthy state cannot be recovered |

Return to Running when a resume event follows a waiting state. Process exit or silent logs must not automatically imply completion. Define confidence and staleness policies per integration. Use heartbeat expiry to detect lost connections only for integrations that support heartbeats, avoiding false completion detection during long-running tool calls.

### Repository Structure

```text
luma/
  src/
    features/usage/
    features/conversations/
    features/settings/
    components/
    lib/
  src-tauri/
    src/adapters/providers/
    src/adapters/agents/
    src/domain/
    src/services/
    src/storage/
    src/commands/
  tests/fixtures/
  docs/integrations/
  .github/workflows/
```

Adapters parse raw data, domain handles metrics and session state, services manages scheduling and lifecycle, and storage handles persistence. The frontend fetches an initial backend snapshot and then subscribes to changes, resynchronizing after reconnection.

## Acceptance Criteria

- [ ] AC1: Validate the specified Copilot GET /copilot_internal/user contract through native HTTPS using credentials authorized for Luma against a real response. P0 verifies the token type and minimum permissions independently of GitHub CLI. Integration documentation identifies the real account type, authorization method, supported usage metrics, selected agent/version, and capability limits. A real-account usage retrieval and real running/waiting/completion sequence have been validated; sanitized examples support repeatable checks. Unsupported required capabilities trigger scope steering rather than silent substitution.
- [ ] AC2: Luma launches on macOS and Windows with a roughly 360 × 480 logical-pixel default window, resizing, dragging, an always-on-top toggle, collapsing, and restored window position. Closing hides to the tray, and tray Quit terminates the application. A disconnected monitor cannot leave the window inaccessible.
- [ ] AC3: The dashboard shows provider usage and session metadata. Waiting sessions precede running sessions and recently completed sessions. Collapsed mode shows waiting/running counts and one selected metric. Status uses text/icons as well as color; loading, disconnected, healthy, stale, insufficient-permission, request-failure, unsupported, and demo states are distinguishable and isolated per integration.
- [ ] AC4: The live usage adapter supplies supported consumption, allowance, unit, period, reset time, and last-update fields. Percentages require matching metrics/units/periods and a positive limit; zero, unlimited, missing, and not-applicable allowances are distinct. Over-quota percentages may exceed 100% while bars are capped. Incompatible units are never summed. Copilot preserves fractional remaining quota and independently reported remaining percentages; the supplied sample computes to approximately 36.1% used without forcing agreement with reported remaining values.
- [ ] AC5: Usage refresh defaults to five minutes, respects provider limits/cache guidance, supports manual refresh with cooldown, and uses exponential backoff on failure. A failed refresh preserves the last successful snapshot and marks it stale instead of clearing usage.
- [ ] AC6: The first agent maps available signals to Running, Waiting for input, Completed, Failed, Canceled, and Unknown. Resume returns to Running; completion means the current run ended successfully. Session/run identity and event deduplication prevent stale or out-of-order events from overwriting a newer run. Silence/process exit alone never imply completion; heartbeat expiry is used only when supported. Reconnection/restart restores or resynchronizes trustworthy state, otherwise showing Unknown.
- [ ] AC7: Settings persist integrations, refresh interval, network proxy mode/server, launch-at-login preference, privacy options, and window preferences. Luma-managed secrets use macOS Keychain/Windows Credential Manager and do not enter frontend payloads, plaintext caches, fixtures, or logs. Users can disconnect integrations and clear caches. Logs redact secrets and sensitive paths. Copilot supports native sign-in, secure token storage, reauthentication, cancellation, disconnect, and account switching without an installed CLI or helper. OAuth client identity belongs to Luma and no client secret is embedded. Verify expiry/revocation, permission failures, and unavailable secure storage.
- [ ] AC8: A documented support matrix names tested OS versions and CPU architectures. Automated builds and locally installable DMG/NSIS artifacts are available. Installation/setup documentation and real-device checks cover both platforms; sleep/wake, network recovery, DPI changes, monitor changes, and launch at login work. Signing/notarization configuration and required external credentials are documented; unsigned validation packages are clearly labeled, and no public-distribution readiness is claimed without actual signing validation.
- [ ] AC9: Core logic and adapter parsing pass meaningful automated checks. Measured steady idle CPU is below 1% of one core on documented test devices, hook-driven updates typically appear within one second, and a memory budget established from the desktop baseline is documented and met. Extended runtime checks show no unresolved resource-growth issue.

- [ ] AC10: Successful usage polls append immutable, timestamped, account/metric-scoped observations with historical plan/allowance, precision, period/reset, and continuity metadata; latest-state projection remains consistent. Replayed collections are idempotent, unchanged new polls remain represented, and failed/cache-only polls never fabricate samples. Bounded time-range queries survive application restarts and schema migrations. Reset/decrease/gap and semantics changes cannot produce false consumption deltas. Default 90-day retention, configurable retention, history disablement, separate cache/history clearing, and per-account/global history deletion work without cross-account leakage.

## Review Focus

- Verify that live integration evidence is real and authorized, personal Copilot quotas are not confused with organization reporting, and unsupported capabilities are visible rather than fabricated.
- Check percentage semantics, period/unit compatibility, overage handling, retained snapshots, cooldowns, and failure backoff. Verify Copilot raw-versus-projected field mapping, internal-API failures, account-scoped caches, and preservation of discrepant provider-reported remaining values.
- Exercise stale, duplicate, and out-of-order events across multiple runs; distinguish waiting, cancellation, failure, successful turn completion, and unknown state.
- Verify OAuth app ownership, minimum permissions established by endpoint validation, device-flow lifecycle, secure credential deletion, native requests without CLI dependencies, and permission-versus-expiry handling.
- Verify least-privilege IPC, local endpoint authentication if applicable, credential storage, redaction, and that no conversation bodies or external cookies are collected.
- Check historical series identity, decimal precision, collection idempotency, transactional latest updates, time-range queries, migrations, and retention/deletion. Confirm reset boundaries and gaps cannot be misrepresented as observed consumption; history collection is in scope even though charts are deferred.
- Check frontend snapshot/subscription synchronization and recovery after backend restart, network loss, sleep, or agent disconnection.
- Confirm native window/tray behavior and accessibility on both platforms, supported-platform claims against actual evidence, and accurate labeling of demo and unsigned artifacts.

## Deferred Items

- Use manual upgrades initially. Additional providers/agents, configurable completion notifications, historical trend charts/analytics UI, cost forecasting, and automatic updates are deferred. History collection, compatible storage, retention, and range queries are part of this MVP.
- Advanced window behavior, cross-device sync, team features, and task orchestration.
- External signing credential acquisition and public distribution. Actual signing/notarization validation remains a prerequisite for later public-distribution claims.

## Work Breakdown

### Step 1: Validate live integration capabilities

- Done: [ ]
- Outcome: A real usage source and one local agent have demonstrated the required signals, with account/version details, sanitized samples, authorization requirements, and explicit capability limits documented. Unresolved access or feasibility issues are surfaced before dependent integration work.
- Covers: AC1
- Check: Complete authorization with Luma credentials and retrieve Copilot usage through native HTTPS without gh installed, confirm account/metric semantics and raw-field mapping, and trigger an actual running → waiting → resumed → completed agent sequence; compare observations with documented source semantics.

### Step 2: Establish the cross-platform desktop experience

- Done: [ ]
- Outcome: The Tauri application provides the floating dashboard, tray lifecycle, collapsed view, settings foundation, and explicitly labeled demo states on both platforms. Baseline measurements establish a memory budget.
- Covers: AC2, AC3, AC9
- Check: Launch on both target platforms and verify window/tray interactions, session ordering, empty/error/demo states, and baseline resource use.

### Step 3: Deliver native authentication and persistent live usage monitoring

- Done: [ ]
- Outcome: The validated provider powers usage cards with native GitHub sign-in and secure token lifecycle management, normalized metrics, scheduled/manual refresh, immutable historical observations, consistent latest-state caching, range queries, retention/deletion controls, rate-limit handling, and isolated failure states.
- Covers: AC4, AC5, AC7, AC10
- Check: Verify live Copilot retrieval and metric-boundary, stale-data, permission, retry, and persistence cases. A sanitized fixture based on the sample must yield 36.1171% calculated usage while retaining 63.8% reported remaining and fractional quota; also cover missing/null fields, schema changes, device-flow denial/expiry/slowdown/cancellation, missing or revoked credentials, permission errors, unavailable secure storage, and account switching. Verify history append/query/replay, unchanged polls, missing data, reset/decrease/gap cases, precision, migrations, retention cleanup, and explicit history deletion.

### Step 4: Deliver trustworthy live session monitoring

- Done: [ ]
- Outcome: The validated agent powers live session cards, recovery-aware state transitions, event deduplication, initial snapshots and change subscriptions, with privacy controls and durable settings.
- Covers: AC3, AC6, AC7
- Check: Exercise actual waiting/resume/completion and repeatable duplicate, out-of-order, cancellation, failure, disconnection, restart, and concurrent-run scenarios.

### Step 5: Complete installation and operational readiness

- Done: [ ]
- Outcome: The support matrix, automated builds, locally installable platform packages, setup documentation, signing configuration, and real-device/performance validation establish the MVP's supported operating envelope. Any unavailable external signing credentials remain explicitly documented.
- Covers: AC2, AC7, AC8, AC9
- Check: Install and exercise packages on both platforms, verify recovery and launch-at-login behavior, run core checks, and compare extended-run measurements with the stated budgets.

## Validation Strategy

- Use Rust tests for backend domain/state logic and adapter parsing, and Vitest for frontend logic where behavior warrants tests. Use sanitized provider responses and versioned agent events for deterministic error, boundary, deduplication, and recovery cases.
- Verify AC10 using deterministic collection times, fractional values, reset/decrease and account/plan-change sequences, duplicate and unchanged polls, failed requests, restart/migration cases, bounded range queries, and retention/deletion boundaries. No trend UI is required to validate the storage contract.
- Validate sign-in and live Copilot retrieval on clean macOS/Windows environments without GitHub CLI or jq. Exercise authorization pending/slowdown/denied/expired states, cancellation, revoked credentials, permission restrictions, reconnect, secure-store errors, disconnect, and account isolation; use protocol fixtures for repeatable failure cases.
- Require real integration checks for AC1; fixtures support regression testing but cannot prove live access. Identify the exact account class and agent version without committing secrets or private transcripts.
- Validate frontend type checking/builds and backend checks on supported build hosts. GitHub Actions provides repeatable macOS/Windows build evidence once repository hosting is available. CI success does not substitute for native desktop interaction checks.
- Run a real-device matrix covering close/quit, tray recovery, pin/collapse/resize, display and DPI changes, sleep/wake, network loss, launch at login, credential persistence, cache clearing, and integration disconnection. Unavailable platform access remains an unmet criterion, not an inferred pass.
- Record devices, workload, interval, CPU normalization, memory budget, and event-latency observations for repeatable performance evaluation. Compare idle and extended-runtime results with AC9.
- Keep durable capability/support/setup documentation in tracked files and disposable logs, screenshots, test output, and measurement evidence under the runtime root returned by `harness repo config get paths.local_runtime`. Do not commit raw credentials or user conversation content.
- Reread the complete self-contained plan, run `harness plan lint` before approval, and retain unmet criteria until evidence supports completion. Harness review/archive/publish/merge remain lifecycle gates outside the work breakdown.

### Execution Entry Point

After explicit plan approval, begin with P0: use the validated Copilot OAuth access and selected Codex agent to finish validating real data sources, and document sample data and capability boundaries. Then initialize the Tauri 2 project and implement the floating-window foundation.

## Closeout

- Validation: PENDING_UNTIL_ARCHIVE
- Review: PENDING_UNTIL_ARCHIVE
- Delivered: PENDING_UNTIL_ARCHIVE
- Not Delivered: PENDING_UNTIL_ARCHIVE
- Follow-Up Issues: NONE
