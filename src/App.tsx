import { useEffect, useState } from 'react';
import { GitHubUsage, type Auth } from './GitHubUsage';
import { Settings, defaultPreferences, type Preferences } from './Settings';
import { HistorySettings } from './HistorySettings';
import { listen } from '@tauri-apps/api/event';
import { isTauri, invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

const samples = [
  { title: 'Review usage adapter', project: 'luma', status: 'Waiting for input', symbol: '◷' },
  { title: 'Build floating window', project: 'luma', status: 'Running', symbol: '●' },
  { title: 'Validate authentication', project: 'luma', status: 'Completed', symbol: '✓' },
];

export function App() {
  const [demo, setDemo] = useState(false);
  const [preferences, setPreferences] = useState<Preferences>(defaultPreferences);
  const [auth, setAuth] = useState<Auth | null>(null);
  const { collapsed, pinned, hide_titles: hideTitles, hide_paths: hidePaths } = preferences;
  const [settings, setSettings] = useState(false);

  const [message, setMessage] = useState('');
  useEffect(() => {
    if (!isTauri()) return;
    void invoke<Preferences>('get_preferences').then(setPreferences).catch(() => setMessage('Unable to load saved preferences.'));
    const errors = listen<string>('preferences-error', event => setMessage(event.payload));
    return () => { void errors.then(unlisten => unlisten()).catch(() => {}); };
  }, []);
  async function updatePreferences(patch: Partial<Preferences>) {
    if (isTauri()) setPreferences(await invoke<Preferences>('update_preferences', { patch }));
    else setPreferences(previous => ({ ...previous, ...patch }));
  }
  const change = (patch: Partial<Preferences>) => void updatePreferences(patch).catch(() => setMessage('Unable to save preferences.'));
  return <main className="dashboard">
    <header>
      <div className="brand" data-tauri-drag-region onPointerDown={(event) => {
        if (isTauri() && event.button === 0) void getCurrentWindow().startDragging().catch(() => setMessage('Unable to drag the window.'));
      }}><span className="logo">✦</span><h1>Luma</h1><span className="mode">{demo ? 'DEMO' : 'LOCAL'}</span></div>
      <nav aria-label="Window controls">
        <button aria-label="Always on top" aria-pressed={pinned} title={isTauri() ? 'Always on top' : 'Available in the desktop app'} disabled={!isTauri()} onClick={() => change({ pinned: !pinned })}>♧</button>
        <button aria-label={collapsed ? 'Expand dashboard' : 'Collapse dashboard'} aria-expanded={!collapsed} onClick={() => change({ collapsed: !collapsed })}>{collapsed ? '+' : '−'}</button>
        <button aria-label="Settings" aria-pressed={settings} onClick={() => setSettings(!settings)}>⚙</button>
      </nav>
    </header>
    {message && <p className="notice" role="status">{message}</p>}
    {settings && <><Settings preferences={preferences} onPreferencesChange={updatePreferences} demo={demo} onDemoChange={setDemo}/><HistorySettings account={auth?.account ?? null}/></>}
    {demo && <p className="demo-note">Demo data · not live usage or session activity</p>}
    {collapsed ? <section className="summary" aria-label="Activity summary">
      <span><b>{demo ? '1' : '—'}</b> waiting</span><span><b>{demo ? '1' : '—'}</b> running</span><span><b>{demo ? '36.1%' : auth?.snapshot?.used_percent != null ? auth.snapshot.used_percent.toFixed(1) + '%' : '—'}</b> used</span>
    </section> : null}
    <div hidden={collapsed}>
      <section aria-labelledby="usage-title">
        <div className="section-heading"><h2 id="usage-title">AI USAGE</h2><span>{demo ? '1 provider' : 'GitHub'}</span></div>
        <div hidden={demo}><GitHubUsage onStatus={setAuth} /></div>
        {demo && <article className="usage-card">
          <div className="provider-row"><div className="provider-icon">⌘</div><div><h3>GitHub Copilot</h3><span className="muted">{demo ? 'Enterprise · premium quota' : 'Premium quota'}</span></div><span className="chip">{demo ? 'Demo' : 'Offline'}</span></div>
          {demo ? <>
            <div className="amount"><strong>36.1<span>%</span></strong><span className="muted">used</span></div>
            <progress value={36.1171} max={100} aria-label="Premium quota used" />
            <div className="quota-detail"><span>722,342 / 2,000,000 credits</span><span>63.8% remaining*</span></div>
            <p className="footnote">Resets Oct 1, 2026 · *Reported by provider</p>
          </> : <div className="empty"><p>Your usage, at a glance.</p><p className="muted">Connect GitHub when native sign-in is available.</p></div>}
        </article>}
      </section>
      <section aria-labelledby="sessions-title">
        <div className="section-heading"><h2 id="sessions-title">CONVERSATIONS</h2><span>Codex</span></div>
        {demo ? <ul className="sessions">{samples.map((s, index) => <li key={s.title}>
          <span className={`status-icon status-${index}`} aria-hidden="true">{s.symbol}</span><div className="session-info"><h3>{hideTitles ? `Session ${index + 1}` : s.title}</h3><p>{hidePaths ? 'Hidden project' : s.project} <span>· {s.status}</span></p></div>
        </li>)}</ul> : <div className="empty session-empty"><span className="empty-icon">◎</span><p>No live session connection</p><p className="muted">Waiting-state detection is under validation.</p></div>}
      </section>
    </div>
    <footer><span><i />{demo ? 'Demo preview' : 'Local dashboard'}</span><button className="text-button" onClick={() => setDemo(!demo)}>{demo ? 'Exit demo' : 'Preview demo'}</button></footer>
  </main>;
}
