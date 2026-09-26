import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createStatusGate } from './lib/status-order.mjs';
import { usageState, refreshWaitSeconds } from './lib/usage-state.mjs';
type Failure = { code: string; message: string; retry_after?: number };
type Snapshot = { account_id: number; login: string; plan: string|null; reset_at: string|null; fetched_at: number; used_percent: number|null; premium: { entitlement: number|null; credits_used: number|null; quota_remaining: number|null; percent_remaining: number|null; overage_permitted: boolean|null; unlimited: boolean|null } };
export type Auth = { revision: number; next_refresh_at: number|null; state: string; account: { id: number; login: string }|null; snapshot: Snapshot|null; error: Failure|null };
type Device = { user_code: string; verification_uri: string; expires_in: number; interval: number };
type Poll = { state: 'pending'|'connected'; retry_after: number|null; status: Auth|null };
const errorMessage = (error: unknown) => typeof error === 'object' && error && 'message' in error ? String(error.message) : 'Unable to connect to GitHub.';
const number = (value: number|null) => value == null ? 'Unknown' : value.toLocaleString(undefined, { maximumFractionDigits: 4 });

export function GitHubUsage({ onStatus, refreshIntervalSeconds = 300 }: { onStatus?: (status: Auth) => void; refreshIntervalSeconds?: number }) {
  const [auth, setAuth] = useState<Auth|null>(null);
  const [disconnectedAccount, setDisconnectedAccount] = useState<Auth['account']>(null);
  const [deletingHistory, setDeletingHistory] = useState(false);
  const [device, setDevice] = useState<Device|null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now());
  const [localDeadline, setLocalDeadline] = useState(0);
  const [failureCode, setFailureCode] = useState<string | undefined>();
  const generation = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout>|undefined>(undefined);

  const acceptStatus = useRef(createStatusGate());
  const applyStatus = useCallback((status: Auth | null) => {
    if (status && acceptStatus.current(status)) { setAuth(status); setLocalDeadline(0); setError(status.error?.message ?? ''); setFailureCode(status.error?.code); }
  }, []);
  const clearPoll = () => { generation.current++; clearTimeout(timer.current); };
  const refresh = useCallback(async () => {
    const previousError = error; const previousCode = failureCode;
    setBusy(true); setLocalDeadline(Date.now() + 30_000);
    try {
      if (auth?.state === 'error' && !auth.account) {
        const restored = await invoke<Auth>('github_status');
        applyStatus(restored);
        if (restored.state !== 'error' || restored.error?.code === 'secure_storage' || (restored.next_refresh_at ?? 0) > Date.now()) return;
      }
      const snapshot = await invoke<Snapshot>('github_refresh');
      void snapshot;
      const status = await invoke<Auth>('github_status');
      applyStatus(status);
    } catch (error) {
      if ((error as Failure)?.code === 'cooldown') { setError(previousError); setFailureCode(previousCode); }
      else { setError(errorMessage(error)); setFailureCode((error as Failure)?.code); }
      const delay = (error as Failure)?.retry_after;
      if (delay) setLocalDeadline(Date.now() + delay * 1000);
      void invoke<Auth>('github_status').then(applyStatus).catch(() => {});
    }
    finally { setBusy(false); }
  }, [applyStatus, auth, error, failureCode]);
  useEffect(() => {
    if (!isTauri()) { setAuth({ revision: 0, next_refresh_at: null, state: 'disconnected', account: null, snapshot: null, error: null }); return; }
    let alive = true;
    let listening = false;
    let subscribing = false;
    let unsubscribe: (() => void) | undefined;
    const subscribe = async () => {
      if (listening || subscribing || !alive) return;
      subscribing = true;
      try {
        const stop = await listen<Auth>('github-status', event => { if (alive) applyStatus(event.payload); });
        if (!alive) stop(); else { unsubscribe = stop; listening = true; }
      } catch { /* A snapshot can still be read; retry subscription on focus. */ }
      finally { subscribing = false; }
    };
    const synchronize = async () => {
      await subscribe();
      if (!alive) return;
      try { const status = await invoke<Auth>('github_status'); if (alive) applyStatus(status); }
      catch (failure) { if (alive) { setError(errorMessage(failure)); setFailureCode((failure as Failure)?.code); } }
    };
    void synchronize();
    const onFocus = () => { void synchronize(); };
    window.addEventListener('focus', onFocus);
    return () => { alive = false; clearPoll(); window.removeEventListener('focus', onFocus); unsubscribe?.(); };
  }, [applyStatus]);
  useEffect(() => { if (auth) onStatus?.(auth); }, [auth, onStatus]);
  useEffect(() => {
    const tick = setInterval(() => setNow(Date.now()), auth && ((auth.next_refresh_at ?? 0) > Date.now() || localDeadline > Date.now()) ? 1000 : 15000);
    return () => clearInterval(tick);
  }, [auth?.next_refresh_at, localDeadline]);
  const retrySeconds = refreshWaitSeconds(Math.max(localDeadline, auth?.next_refresh_at ?? 0), now);
  const refreshReady = retrySeconds === 0;
  const state = usageState(auth, { now, refreshIntervalSeconds, hasError: !!error, errorCode: failureCode, signingIn: !!device, busy });
  async function begin() {
    clearPoll(); const current = generation.current;
    setBusy(true); setError(''); setFailureCode(undefined);
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
          if (response.state === 'connected') { applyStatus(response.status); setDevice(null); return; }
          timer.current = setTimeout(poll, Math.max(response.retry_after ?? result.interval, 1) * 1000);
        } catch (error) {
          if (current !== generation.current) return;
          setError(errorMessage(error)); setFailureCode((error as Failure)?.code);
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
  async function cancel() { clearPoll(); setDevice(null); setBusy(false); try { await invoke('github_cancel'); applyStatus(await invoke<Auth>('github_status')); } catch (error) { setError(errorMessage(error)); } }
  async function disconnect() {
    clearPoll(); setDevice(null); setBusy(true);
    const previousAccount = auth?.account;
    try { applyStatus(await invoke<Auth>('github_disconnect')); if (previousAccount) setDisconnectedAccount(previousAccount); }
    catch (error) { setError(errorMessage(error)); } finally { setBusy(false); }
  }
  async function deleteDisconnectedHistory() {
    const account = disconnectedAccount;
    if (!account) return;
    setDeletingHistory(true);
    try {
      await invoke('clear_usage_history', { account: { provider: 'github_copilot', host: 'api.github.com', account_id: String(account.id) } });
      setDisconnectedAccount(null);
    } catch { setError('Unable to delete saved usage history. You can retry.'); }
    finally { setDeletingHistory(false); }
  }
  const snapshot = auth?.snapshot;
  return <article className="usage-card">
    <div className="provider-row"><div className="provider-icon">⌘</div><div><h3>GitHub Copilot</h3><span className="muted">{snapshot ? `${snapshot.login} · ${snapshot.plan ?? 'Premium quota'}` : 'Premium quota'}</span></div><span className={"chip chip-" + state.kind} role="status"><span aria-hidden="true">{state.symbol} </span>{state.label}</span></div>
    {snapshot && <>
      <div className="amount"><strong>{snapshot.used_percent == null ? '—' : snapshot.used_percent.toFixed(1)}{snapshot.used_percent != null && <span>%</span>}</strong><span className="muted">used</span></div>
      {snapshot.used_percent != null && <progress value={Math.min(100, Math.max(0, snapshot.used_percent))} max={100} aria-label="Premium quota used" />}
      <div className="quota-detail"><span>{number(snapshot.premium.credits_used)} / {snapshot.premium.unlimited ? 'Unlimited' : number(snapshot.premium.entitlement)} quota credits</span><span>{snapshot.premium.percent_remaining == null ? 'Remaining unknown' : number(snapshot.premium.percent_remaining) + '% remaining*'}</span></div>
      <p className="footnote">{snapshot.reset_at ? `Resets ${new Date(snapshot.reset_at).toLocaleString()}` : 'Reset time unavailable'}<br/>*Remaining reported by provider · Updated {new Date(snapshot.fetched_at).toLocaleTimeString()}</p>
    </>}
    {!snapshot && !device && <div className="empty"><p>{!auth && !error ? 'Loading GitHub connection…' : 'Your usage, at a glance.'}</p><p className="muted">{isTauri() ? 'Sign in securely through GitHub.' : 'Open the desktop app to connect GitHub.'}</p></div>}
    {device && <div className="device-auth"><p>Enter this code on GitHub:</p><strong>{device.user_code}</strong><button className="primary-button" onClick={() => void invoke('open_github_verification').catch(error => setError(errorMessage(error)))}>Open GitHub</button><p className="muted">Waiting for authorization…</p><button className="text-button" onClick={cancel}>Cancel sign-in</button></div>}
    {disconnectedAccount && <div className="notice" role="group" aria-label="Disconnected account history">
      <p>Disconnected {disconnectedAccount.login}. Keep this account’s saved usage history, or permanently delete it from Luma?</p>
      <button className="text-button" disabled={deletingHistory} onClick={() => setDisconnectedAccount(null)}>Keep history</button>{' '}
      <button className="text-button" disabled={deletingHistory} onClick={() => void deleteDisconnectedHistory()}>{deletingHistory ? 'Deleting…' : 'Delete disconnected account history'}</button>
    </div>}
    {error && <p role="status" className="notice">{error}</p>}
    {!device && <div className="usage-actions">
      {auth?.state === 'connected' ? <><button className="text-button" disabled={busy || !refreshReady} onClick={() => void refresh()}>{busy ? 'Refreshing…' : !refreshReady ? 'Retry in ' + retrySeconds + 's' : 'Refresh'}</button><button className="text-button" disabled={busy} onClick={begin}>Switch account</button><button className="text-button" disabled={busy} onClick={disconnect}>Disconnect</button></> : <button className="primary-button" disabled={!isTauri() || busy} onClick={begin}>{busy ? 'Connecting…' : 'Connect GitHub'}</button>}
      {auth?.state === 'error' && <button className="text-button" disabled={busy || !refreshReady} onClick={() => void refresh()}>Retry connection</button>}
      {auth?.state !== 'connected' && (auth?.account || auth?.state === 'error') && <button className="text-button" disabled={busy} onClick={disconnect}>Disconnect</button>}
    </div>}
  </article>;
}
