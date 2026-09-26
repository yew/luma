import { useEffect, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';

type Settings = { enabled: boolean; retention_days: number };
type Account = { id: number; login: string };
type AccountKey = { provider: string; host: string; account_id: string };
type Deletion = { kind: 'all' } | { kind: 'account'; key: AccountKey; login: string };
export function HistorySettings({ account }: { account: Account | null }) {
  const [settings, setSettings] = useState<Settings | null>(null);
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState<Deletion | null>(null);
  useEffect(() => {
    if (isTauri()) void invoke<Settings>('history_settings').then(setSettings).catch(() => setMessage('Unable to load history settings.'));
  }, []);
  async function update(next: Settings) {
    setBusy(true);
    try { setSettings(await invoke<Settings>('update_history_settings', { enabled: next.enabled, retentionDays: next.retention_days })); setMessage('History settings saved.'); }
    catch { setMessage('Unable to save history settings.'); }
    finally { setBusy(false); }
  }
  useEffect(() => setConfirmation(null), [account?.id]);
  const key: AccountKey | null = account ? { provider: 'github_copilot', host: 'api.github.com', account_id: String(account.id) } : null;
  async function clear(history: boolean, target: AccountKey | null) {
    setBusy(true);
    try {
      await invoke(history ? 'clear_usage_history' : 'clear_usage_cache', { account: target });
      setMessage(history ? 'Selected usage history deleted. Current usage cache is retained.' : 'Cached usage cleared. History is retained.');
      setConfirmation(null);
    } catch { setMessage('Unable to clear local data.'); }
    finally { setBusy(false); }
  }
  return <section className="settings" aria-label="Usage history settings">
    <h3>Usage history</h3>
    {settings ? <>
      <label><input type="checkbox" disabled={busy} checked={settings.enabled} onChange={event => void update({ ...settings, enabled: event.target.checked })}/> Collect usage history</label>
      <label>Retention <select disabled={busy} value={settings.retention_days} onChange={event => void update({ ...settings, retention_days: Number(event.target.value) })}>{[7, 30, 90, 180, 365].map(days => <option key={days} value={days}>{days} days</option>)}</select></label>
      <p>Only successful polls create history. Shorter retention removes older observations. Disabling collection keeps existing history until it expires or is deleted.</p>
      <button className="text-button" disabled={busy} onClick={() => void clear(false, null)}>Clear usage cache</button>
      <button className="text-button" disabled={busy || !account} onClick={() => { if (account && key) setConfirmation({ kind: 'account', key, login: account.login }); }}>Delete {account?.login ?? 'account'} history</button>
      <button className="text-button" disabled={busy} onClick={() => setConfirmation({ kind: 'all' })}>Delete all history</button>
      {confirmation && <div role="alert"><p>Delete {confirmation.kind === 'all' ? 'all accounts’' : confirmation.login + '’s'} saved usage observations? This cannot be undone.</p><button disabled={busy} onClick={() => void clear(true, confirmation.kind === 'all' ? null : confirmation.key)}>Delete history</button><button disabled={busy} onClick={() => setConfirmation(null)}>Cancel</button></div>}
    </> : <p>{isTauri() ? 'Loading history settings…' : 'History is available in the desktop app.'}</p>}
    {message && <p role="status">{message}</p>}
  </section>;
}
