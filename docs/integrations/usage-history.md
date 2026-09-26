# Usage History

The native provider saves successful Copilot polls to usage.sqlite3 in Tauri's application data directory. SQLite is bundled into the executable; no external database or helper service is required. Credentials remain in the OS credential store.

## Observations and Identity

A series is scoped by provider, host, stable account ID, metric, unit, and semantics version. Copilot uses github_copilot / api.github.com / the verified numeric GitHub ID / premium_interactions / provider_quota_credit / copilot-premium-v1. Login and plan changes never merge unrelated accounts.

Each successful fresh poll saves an immutable observation and collection outcome, then updates the current cache in one transaction. Replayed collection IDs are idempotent while retained; separate unchanged polls remain separate observations. Failed, missing-metric, and cache-only results create no quota samples. Late results cannot replace newer cached values. Collection IDs use random UUIDs and the provider serializes requests.

Amounts remain exact decimal text in history. JSON number spelling is preserved before floating-point UI formatting. Values outside the supported exact decimal range are rejected rather than rounded into history. Used percentage is presentation-only; remaining quota and percentage retain independent provider values. Absent period boundaries stay null. Collection times are UTC Unix milliseconds; reset time is never observation time.

Copilot counter semantics and reporting-period continuity remain unverified, so observations carry unknown continuity and cannot support inferred consumption deltas yet. Reset/period, allowance/plan, source/quality, decreases, out-of-order arrivals, semantics/unit changes, and gaps greater than 15 minutes create boundaries. No API sums snapshots or fabricates missing intervals.

## Storage and Queries

Migrations use SQLite user_version and reject newer schemas. Migration 1 creates series, observations, collection outcomes, latest projections, and settings; migration 2 adds range/retention indexes. Observation UPDATE is prohibited by a trigger.

The query_usage_history command accepts this query shape. The upper time bound is exclusive. Limits are 1–500; pass the returned next_cursor verbatim to continue timestamp/observation-ID order.

```json
{
  "query": {
    "account": { "provider": "github_copilot", "host": "api.github.com", "account_id": "42" },
    "metric": "premium_interactions",
    "unit": "provider_quota_credit",
    "semantics_version": "copilot-premium-v1",
    "from": 1790377200000,
    "to": 1790463600000,
    "limit": 100,
    "cursor": null
  }
}
```

This is an IPC storage API for future trends; no historical charts are included. Cache reads never manufacture observations. Explicit rebuild_usage_cache recreates a projection from retained successful collections. Clearing cache does not rebuild it automatically; a fresh provider poll can populate it again.

## Retention and Deletion

Collection starts enabled with 90-day retention. The service allows 1–3650 days; the UI offers 7, 30, 90, 180, and 365 days. Cleanup occurs on collection, settings changes, startup, and hourly while disconnected. Disabled collection still updates current usage; old history ages out normally.

History/cache deletion APIs accept a complete account identity or null for all accounts. History deletion preserves current cache and credentials. Disconnect cancels polling and deletes the local credential; history follows retention or explicit deletion. Reducing retention removes expired samples immediately. Secure-delete and a history-deletion checkpoint reduce residual database data without claiming forensic erasure.

## Validation

Tests cover exact decimals, unchanged/replayed polls, failures, rollback, account/unit/semantics isolation, pagination, reset/decrease/gap boundaries, cache/history deletion, retention/disablement, restart, and migrations. A Copilot-to-storage test verifies precision and account identity. Installed-app and Windows checks remain tracked in the support matrix.
