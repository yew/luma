import { useEffect, useState, type FormEvent } from 'react';
import { isTauri } from '@tauri-apps/api/core';
import './Settings.css';

export type Preferences = {
  refresh_interval_secs: number;
  launch_at_login: boolean;
  hide_titles: boolean;
  hide_paths: boolean;
  pinned: boolean;
  collapsed: boolean;
  proxy: { mode: 'environment' | 'direct' | 'http'; server: string };
};

export const defaultPreferences: Preferences = {
  refresh_interval_secs: 300,
  launch_at_login: false,
  hide_titles: false,
  hide_paths: false,
  pinned: true,
  collapsed: false,
  proxy: { mode: 'environment', server: '' },
};

type SettingsProps = {
  preferences: Preferences;
  onPreferencesChange: (patch: Partial<Preferences>) => Promise<void>;
  demo: boolean;
  onDemoChange: (demo: boolean) => void;
};

export function Settings({ preferences, onPreferencesChange, demo, onDemoChange }: SettingsProps) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const native = isTauri();
  const [proxy, setProxy] = useState(preferences.proxy);
  const [proxySaved, setProxySaved] = useState(false);
  useEffect(() => { setProxy(preferences.proxy); }, [preferences.proxy.mode, preferences.proxy.server]);
  async function saveProxy(event: FormEvent) {
    event.preventDefault();
    setBusy(true); setError(''); setProxySaved(false);
    try {
      const savedProxy = proxy.mode === 'http' ? proxy : { ...proxy, server: preferences.proxy.server };
      await onPreferencesChange({ proxy: savedProxy }); setProxySaved(true);
    }
    catch (failure) { setError(typeof failure === 'string' ? failure : 'Unable to save proxy settings.'); }
    finally { setBusy(false); }
  }
  async function change(patch: Partial<Preferences>) {
    setBusy(true);
    setError('');
    try { await onPreferencesChange(patch); }
    catch (failure) { setError(typeof failure === 'string' ? failure : 'Unable to save settings. Try again.'); }
    finally { setBusy(false); }
  }
  return <section className="settings preferences" aria-label="Dashboard settings">
    <h2>SETTINGS</h2>
    <label><input type="checkbox" checked={demo} onChange={event => onDemoChange(event.target.checked)} /> Show demo data</label>
    <fieldset disabled={busy || !native}>
      <label className="settings-select">Refresh usage
        <select value={preferences.refresh_interval_secs} onChange={event => void change({ refresh_interval_secs: Number(event.target.value) })}>
          {[60, 120, 300, 600, 900, 1800, 3600].includes(preferences.refresh_interval_secs) ? null : <option value={preferences.refresh_interval_secs}>{preferences.refresh_interval_secs} seconds</option>}
          <option value={60}>Every minute</option>
          <option value={120}>Every 2 minutes</option>
          <option value={300}>Every 5 minutes</option>
          <option value={600}>Every 10 minutes</option>
          <option value={900}>Every 15 minutes</option>
          <option value={1800}>Every 30 minutes</option>
          <option value={3600}>Every hour</option>
        </select>
      </label>
      <p>Provider limits and retry delays may postpone a refresh.</p>
      <label><input type="checkbox" checked={preferences.launch_at_login} onChange={event => void change({ launch_at_login: event.target.checked })} /> Launch Luma at login</label>
      <label><input type="checkbox" checked={preferences.pinned} onChange={event => void change({ pinned: event.target.checked })} /> Always on top</label>
      <label><input type="checkbox" checked={preferences.hide_titles} onChange={event => void change({ hide_titles: event.target.checked })} /> Hide session titles</label>
      <label><input type="checkbox" checked={preferences.hide_paths} onChange={event => void change({ hide_paths: event.target.checked })} /> Hide project paths</label>
    </fieldset>
    <form onSubmit={saveProxy} className="proxy-settings">
      <fieldset disabled={busy || !native}>
        <legend>Proxy</legend>
        <label className="settings-select">Connection
          <select value={proxy.mode} onChange={event => { setProxy({ ...proxy, mode: event.target.value as Preferences['proxy']['mode'] }); setProxySaved(false); }}>
            <option value="environment">Environment proxy</option>
            <option value="direct">Direct connection</option>
            <option value="http">HTTP proxy</option>
          </select>
        </label>
        {proxy.mode === 'http' && <label className="settings-select">Proxy server
          <input type="url" required spellCheck={false} autoComplete="off" maxLength={2048} placeholder="http://127.0.0.1:7890" value={proxy.server} onChange={event => { setProxy({ ...proxy, server: event.target.value }); setProxySaved(false); }}/>
        </label>}
        <p>Applies to GitHub sign-in and usage requests. HTTP proxy URLs must not include a username or password.</p>
        <button className="primary-button" type="submit">{busy ? 'Saving…' : 'Save proxy'}</button>
        {proxySaved && <p role="status">Proxy saved. New requests use this connection; no restart needed.</p>}
      </fieldset>
    </form>
    <p>{native ? 'Preferences stay on this device. GitHub credentials use system secure storage.' : 'Persistent preferences are available in the desktop app.'}</p>
    {error && <p className="settings-error" role="alert">{error}</p>}
  </section>;
}
