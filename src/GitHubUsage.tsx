import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
type Failure = { code: string; message: string; retry_after?: number };
type Snapshot = { account_id: number; login: string; plan: string|null; reset_at: string|null; fetched_at: number; used_percent: number|null; premium: { entitlement: number|null; credits_used: number|null; quota_remaining: number|null; percent_remaining: number|null; overage_permitted: boolean|null } };
type Auth = { state: string; account: { id: number; login: string }|null; snapshot: Snapshot|null; error: Failure|null };
type Device = { user_code: string; verification_uri: string; expires_in: number; interval: number };
type Poll = { state: 'pending'|'connected'; retry_after: number|null; status: Auth|null };
const errorMessage = (error: unknown) => typeof error === 'object' && error && 'message' in error ? String(error.message) : 'Unable to connect to GitHub.';
const number = (value: number|null) => value == null ? 'Unknown' : value.toLocaleString(undefined, { maximumFractionDigits: 4 });

let initialStatus: Promise<Auth> | null = null;
function restoreStatus() {
  initialStatus ??= invoke<Auth>('github_status');
  return initialStatus;
}

export function GitHubUsage() {
  const [auth, setAuth] = useState<Auth|null>(null);
  const [device, setDevice] = useState<Device|null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [refreshReady, setRefreshReady] = useState(true);
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>|undefined>(undefined);
  const retryCount = useRef(0);
  const refreshDelay = useRef(300_000);
  const clearPoll = () => { generation.current++; clearTimeout(timer.current); };
  const refresh = useCallback(async () => {
    setBusy(true); setRefreshReady(false);
    try {
      const snapshot = await invoke<Snapshot>('github_refresh');
      setAuth({ state: 'connected', account: { id: snapshot.account_id, login: snapshot.login }, snapshot, error: null });
      setError(''); retryCount.current = 0; refreshDelay.current = 300_000;
    } catch (error) { setError(errorMessage(error)); retryCount.current++; refreshDelay.current = Math.max(Math.min(300_000 * 2 ** retryCount.current, 3_600_000), ((error as Failure)?.retry_after ?? 0) * 1000); if ((error as Failure)?.code === 'reauth_required') setAuth(previous => previous ? { ...previous, state: 'reauth_required' } : previous); }
    finally { setBusy(false); }
  }, []);
  useEffect(() => {
    if (!isTauri()) return;
    let alive = true;
    restoreStatus().then(status => { if (alive) { setAuth(status); if (status.error) setError(status.error.message); } }).catch(error => { if (alive) setError(errorMessage(error)); });
    return () => { alive = false; clearPoll(); };
  }, []);
  useEffect(() => {
    if (refreshReady) return;
    const timeout = setTimeout(() => setRefreshReady(true), 30_000);
    return () => clearTimeout(timeout);
  }, [refreshReady]);
  useEffect(() => {
    if (auth?.state !== 'connected' || device || busy) return;
    const timeout = setTimeout(() => void refresh(), auth.snapshot || retryCount.current > 0 ? refreshDelay.current : 100);
    return () => clearTimeout(timeout);
  }, [auth, device, busy, refresh]);
  async function begin() {
    clearPoll(); const current = generation.current;
    setBusy(true); setError('');
    try {
      const result = await invoke<Device>('github_begin');
      if (current !== generation.current) return;
      setDevice(result);
      const deadline = Date.now() + result.expires_in * 1000;
      const poll = async () => {
        if (current !== generation.current) return;
        if (Date.now() >= deadline) { setDevice(null); setError('Authorization expired. Connect again.'); void invoke('github_cancel'); return; }
        try {
          const response = await invoke<Poll>('github_poll');
          if (current !== generation.current) return;
          if (response.state === 'connected') { setAuth(response.status); setDevice(null); setError(''); return; }
          timer.current = setTimeout(poll, Math.max(response.retry_after ?? result.interval, 1) * 1000);
        } catch (error) {
          if (current !== generation.current) return;
          setError(errorMessage(error));
          const failure = error as Failure;
          if (failure?.code === 'network' || failure?.code === 'rate_limited') {
            timer.current = setTimeout(poll, Math.max(failure.retry_after ?? result.interval, result.interval, 5) * 1000);
          } else { setDevice(null); void invoke('github_cancel').catch(() => {}); }
        }
      };
      timer.current = setTimeout(poll, result.interval * 1000);
    } catch (error) { if (current === generation.current) setError(errorMessage(error)); }
    finally { if (current === generation.current) setBusy(false); }
  }
  async function cancel() { clearPoll(); setDevice(null); setBusy(false); try { await invoke('github_cancel'); setAuth(await invoke<Auth>('github_status')); } catch (error) { setError(errorMessage(error)); } }
  async function disconnect() {
    clearPoll(); setDevice(null); setBusy(true);
    try { setAuth(await invoke<Auth>('github_disconnect')); setError(''); }
    catch (error) { setError(errorMessage(error)); } finally { setBusy(false); }
  }
  const snapshot = auth?.snapshot;
  return <article className="usage-card">
    <div className="provider-row"><div className="provider-icon">⌘</div><div><h3>GitHub Copilot</h3><span className="muted">{snapshot ? `${snapshot.login} · ${snapshot.plan ?? 'Premium quota'}` : 'Premium quota'}</span></div><span className="chip">{device ? 'Sign in' : error && snapshot ? 'Stale' : auth?.state === 'connected' ? 'Connected' : 'Offline'}</span></div>
    {snapshot && <>
      <div className="amount"><strong>{snapshot.used_percent == null ? '—' : snapshot.used_percent.toFixed(1)}<span>%</span></strong><span className="muted">used</span></div>
      {snapshot.used_percent != null && <progress value={Math.min(100, Math.max(0, snapshot.used_percent))} max={100} aria-label="Premium quota used" />}
      <div className="quota-detail"><span>{number(snapshot.premium.credits_used)} / {number(snapshot.premium.entitlement)} credits</span><span>{number(snapshot.premium.percent_remaining)}% remaining*</span></div>
      <p className="footnote">{snapshot.reset_at ? `Resets ${new Date(snapshot.reset_at).toLocaleString()}` : 'Reset time unavailable'}<br/>*Remaining reported by provider · Updated {new Date(snapshot.fetched_at).toLocaleTimeString()}</p>
    </>}
    {!snapshot && !device && <div className="empty"><p>Your usage, at a glance.</p><p className="muted">{isTauri() ? 'Sign in securely through GitHub.' : 'Open the desktop app to connect GitHub.'}</p></div>}
    {device && <div className="device-auth"><p>Enter this code on GitHub:</p><strong>{device.user_code}</strong><button className="primary-button" onClick={() => void invoke('open_github_verification').catch(error => setError(errorMessage(error)))}>Open GitHub</button><p className="muted">Waiting for authorization…</p><button className="text-button" onClick={cancel}>Cancel sign-in</button></div>}
    {error && <p role="status" className="notice">{error}</p>}
    {!device && <div className="usage-actions">
      {auth?.state === 'connected' ? <><button className="text-button" disabled={busy || !refreshReady} onClick={() => void refresh()}>{busy ? 'Refreshing…' : !refreshReady ? 'Refresh shortly' : 'Refresh'}</button><button className="text-button" disabled={busy} onClick={disconnect}>Disconnect</button></> : <button className="primary-button" disabled={!isTauri() || busy} onClick={begin}>{busy ? 'Connecting…' : 'Connect GitHub'}</button>}
      {auth?.state === 'error' && <button className="text-button" disabled={busy || !refreshReady} onClick={() => void refresh()}>Retry connection</button>}
      {auth?.state !== 'connected' && (auth?.account || auth?.state === 'error') && <button className="text-button" disabled={busy} onClick={disconnect}>Disconnect</button>}
    </div>}
  </article>;
}
