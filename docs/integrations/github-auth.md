# GitHub Authentication Validation

Luma uses its own public OAuth client ID, configured in `config/github-oauth.json`. This ID is not a secret. Device Flow must be enabled in the GitHub OAuth application's settings.

P0 begins with an empty scope request to test the minimum permission boundary. This does not establish that Copilot accepts such tokens. Validate `/user` and `/copilot_internal/user` after authorization; distinguish successful device authorization from actual endpoint access. Scope expansion requires evidence and explicit consent rather than silently requesting broad permissions.

The diagnostic device request is a temporary P0 probe, not the application implementation. Device codes remain in permission-restricted local runtime storage only until authorization or expiry; access tokens must not be written to files or tool output. The production application will use Rust HTTPS and OS credential storage without GitHub CLI.

## P0 Validation Result

On 2026-09-26 (Asia/Shanghai), device authorization using Luma's own OAuth Client ID succeeded with an empty scope request. The returned bearer token reported no scopes. Native HTTPS requests returned:

| Request | Result |
| --- | --- |
| GET /user | HTTP 200; authenticated account identity available |
| GET /copilot_internal/user | HTTP 200; returned login matched the authenticated identity |

The tested account has an enterprise Copilot plan. The premium-interactions response contained entitlement, credits_used, fractional quota_remaining, percent_remaining, overage_permitted, and a UTC reset timestamp as expected. The API's field/unit stability and other account/policy combinations still require validation; one successful account does not establish universal access.

No GitHub CLI, external app identity, client secret, or additional OAuth scope was used. Keep the initial scope list empty; broader permissions are not justified by this result. The diagnostic token was held only in process memory, never printed or saved, and the temporary device-code file was removed after exchange. The process then exited without retaining the token. This does not revoke the GitHub application grant; the production application will need a new sign-in and secure OS token storage.

Sanitized HTTP-status evidence is stored in the local harness runtime directory. This probe completed only provider authentication feasibility, not all of P0. The subsequent Rust implementation and remaining validation are described below.

## Native Implementation

The Rust backend now implements device authorization, interval/expiry enforcement, cancellation generations, minimum-scope validation, identity-checked Copilot projection, OS credential-store persistence/restoration/deletion, bounded HTTPS with redirects disabled, refresh cooldown/backoff, and explicit failure categories. The frontend exposes connect, browser verification, cancel, refresh, retry, and disconnect. Tokens and device codes remain backend-only.

OAuth bearer tokens are accepted even when lifecycle metadata is present. Positive expires_in values are converted to an expiry timestamp and stored with the access token in the OS credential store; absent, null, or zero lifetimes do not establish a positive expiry and remain subject to server-side validation. The app requests reconnection on known expiry or HTTP 401. Refresh tokens are neither persisted nor used; no undocumented refresh is attempted. Legacy plain-token credential entries remain readable. This fixes an overly strict parser that rejected valid responses solely for including lifecycle fields; the earlier probe had not established that lifecycle fields were absent. The app does not depend on GitHub CLI or jq. Successful fresh polls persist immutable account-scoped observations in SQLite. Current cache is a rebuildable projection; cached/304 provider replies preserve the original observation without creating history. See [usage history](usage-history.md).

Native Rust tests and compilation pass on macOS. Installed-app new sign-in, secure-store restoration, cache clearing, disconnect, restart and network recovery are now verified in the macOS acceptance record. Windows execution is deferred. The original feasibility probe did not retain its token; the accepted desktop sign-in used its own authorization.

## Provider Timing and Freshness

Automatic refresh uses a monotonic successful-poll clock with the configured interval. The native next_refresh_at deadline communicates the earliest permitted manual retry to the dashboard. Failure retries wait 5 seconds after each of the first three failures, then 15, 30, 60, 120, and 300 seconds; repeated failures stay at the five-minute cap. A successful poll resets the streak. The schedule is independent of the normal polling interval and respects longer Retry-After values (seconds or HTTP dates) and rate-limit reset epochs. Backend scheduler ticks may add up to approximately five seconds. Manual retries use the same failure deadline; the usual 30-second cooldown applies after successful requests. Usable Cache-Control max-age is reduced by Age/apparent Date age; clearing local cache cannot bypass an existing provider wait.

A 304 or positive-Age response is treated as reused data: retain the last live snapshot and timestamp, schedule the next eligible poll, and create no history sample or failure outcome. Initial sign-in can retain the authorized account with no usage snapshot until a fresh reply arrives. Permission, unsupported schema/endpoint, reconnect, rate-limit, stale, and loading states have separate text/icons. Cached replies are not presented as new consumption.

## Credential Recovery

A failed OS credential-store read leaves restoration retryable; unlocking the store and selecting Retry connection retries stored credentials without requiring a new grant or app restart. Failure to write a replacement credential leaves the prior account and usage untouched. Disconnect blocks further polling even if secure deletion fails, and deletion can be retried. After a successful disconnect, the UI offers Keep history or Delete disconnected account history using the captured account identity; keeping is the default unless deletion is explicitly selected. An unsuccessful or canceled reconnect preserves the existing token's reauthentication state. Unexpected identity changes use the same bounded failure retry schedule and never replace the prior account's history. Failed legacy token metadata updates are retried on subsequent successful fetches.

Fake-store tests cover these failure paths without accessing live credentials. UI component tests verify retry/cancel/order behavior with a mocked IPC bridge; neither substitutes for a real new sign-in in a packaged application.

## Installed-App Acceptance Confirmation

The user completed Clear usage cache → Disconnect → Connect GitHub → authorize → observe live usage → Quit/relaunch and confirmed each step succeeded. Metadata-only follow-up verified a new process, an account-scoped cache, and fresh successful usage collection. Earlier native checks independently verified credential restoration, proxy failure/recovery, login startup, and persisted settings. See [macOS acceptance](../validation/macos-2026-09-26.md). Account-switch failure/isolation and unavailable secure storage remain covered by deterministic fake-store tests; no second live account is claimed.
