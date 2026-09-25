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

Sanitized HTTP-status evidence is stored in the local harness runtime directory. Production Rust authentication, token lifecycle, secure storage, and the first local-agent integration remain unimplemented; this result completes only the provider authentication feasibility check, not all of P0.
