export function usageState(auth, options = {}) {
  const { errorCode, hasError = false, signingIn = false, busy = false, now = Date.now(), refreshIntervalSeconds = 300 } = options;
  if (signingIn) return { label: 'Signing in', symbol: '◷', kind: 'loading' };
  if (!auth && !hasError) return { label: 'Loading', symbol: '◷', kind: 'loading' };
  const code = errorCode ?? auth?.error?.code;
  if (auth?.state === 'reauth_required' || code === 'reauth_required') return { label: 'Reconnect', symbol: '!', kind: 'error' };
  if (code === 'permission_denied') return { label: 'Permission needed', symbol: '!', kind: 'error' };
  if (['schema', 'endpoint_unavailable', 'metric_unavailable'].includes(code)) return { label: 'Unsupported', symbol: '!', kind: 'error' };
  if (code === 'rate_limited') return { label: 'Rate limited', symbol: '◷', kind: 'error' };
  if (code === 'cached_response') return { label: auth?.snapshot ? 'Stale' : 'Awaiting fresh usage', symbol: '◷', kind: 'stale' };
  if (auth?.snapshot && ((hasError && code !== 'cooldown') || auth.error || now - auth.snapshot.fetched_at > refreshIntervalSeconds * 1000 + 30000)) return { label: 'Stale', symbol: '!', kind: 'stale' };
  if ((hasError || auth?.error) && code !== 'cooldown') return { label: 'Request failed', symbol: '!', kind: 'error' };
  if (busy) return { label: 'Refreshing', symbol: '◷', kind: 'loading' };
  if (auth?.state === 'connected') return { label: 'Connected', symbol: '✓', kind: 'healthy' };
  return { label: 'Disconnected', symbol: '○', kind: 'disconnected' };
}

export function refreshWaitSeconds(deadline, now) {
  return typeof deadline === 'number' && Number.isFinite(deadline) ? Math.max(0, Math.ceil((deadline - now) / 1000)) : 0;
}
