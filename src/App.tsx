import { useState } from 'react';
import { isTauri, invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';

const samples = [
  { title: 'Review usage adapter', project: 'luma', status: 'Waiting for input', symbol: '◷' },
  { title: 'Build floating window', project: 'luma', status: 'Running', symbol: '●' },
  { title: 'Validate authentication', project: 'luma', status: 'Completed', symbol: '✓' },
];

export function App() {
  const [demo, setDemo] = useState(false);
  const [collapsed, setCollapsed] = useState(false);
  const [settings, setSettings] = useState(false);
  const [pinned, setPinned] = useState(true);
  const [hideTitles, setHideTitles] = useState(false);
  const [message, setMessage] = useState('');
  async function togglePin() {
    try { await invoke('set_pinned', { pinned: !pinned }); setPinned(!pinned); }
    catch { setMessage('Unable to change window pinning.'); }
  }
  return <main className="dashboard">
    <header>
      <div className="brand" data-tauri-drag-region onPointerDown={(event) => {
        if (isTauri() && event.button === 0) void getCurrentWindow().startDragging().catch(() => setMessage('Unable to drag the window.'));
      }}><span className="logo">✦</span><h1>Luma</h1><span className="mode">{demo ? 'DEMO' : 'LOCAL'}</span></div>
      <nav aria-label="Window controls">
        <button aria-label="Always on top" aria-pressed={pinned} title={isTauri() ? 'Always on top' : 'Available in the desktop app'} disabled={!isTauri()} onClick={togglePin}>♧</button>
        <button aria-label={collapsed ? 'Expand dashboard' : 'Collapse dashboard'} aria-expanded={!collapsed} onClick={() => setCollapsed(!collapsed)}>{collapsed ? '+' : '−'}</button>
        <button aria-label="Settings" aria-pressed={settings} onClick={() => setSettings(!settings)}>⚙</button>
      </nav>
    </header>
    {message && <p className="notice" role="status">{message}</p>}
    {settings && <section className="settings" aria-label="Settings">
      <label><input type="checkbox" checked={demo} onChange={e => setDemo(e.target.checked)} /> Show demo data</label>
      <label><input type="checkbox" checked={hideTitles} onChange={e => setHideTitles(e.target.checked)} /> Hide session titles</label>
      <p>Preview settings apply to this window only. Live connections are not configured.</p>
    </section>}
    {demo && <p className="demo-note">Demo data · not live usage or session activity</p>}
    {collapsed ? <section className="summary" aria-label="Activity summary">
      <span><b>{demo ? '1' : '—'}</b> waiting</span><span><b>{demo ? '1' : '—'}</b> running</span><span><b>{demo ? '36.1%' : '—'}</b> used</span>
    </section> : <>
      <section aria-labelledby="usage-title">
        <div className="section-heading"><h2 id="usage-title">AI USAGE</h2><span>{demo ? '1 provider' : 'Not connected'}</span></div>
        <article className="usage-card">
          <div className="provider-row"><div className="provider-icon">⌘</div><div><h3>GitHub Copilot</h3><span className="muted">{demo ? 'Enterprise · premium quota' : 'Premium quota'}</span></div><span className="chip">{demo ? 'Demo' : 'Offline'}</span></div>
          {demo ? <>
            <div className="amount"><strong>36.1<span>%</span></strong><span className="muted">used</span></div>
            <progress value={36.1171} max={100} aria-label="Premium quota used" />
            <div className="quota-detail"><span>722,342 / 2,000,000 credits</span><span>63.8% remaining*</span></div>
            <p className="footnote">Resets Oct 1, 2026 · *Reported by provider</p>
          </> : <div className="empty"><p>Your usage, at a glance.</p><p className="muted">Connect GitHub when native sign-in is available.</p></div>}
        </article>
      </section>
      <section aria-labelledby="sessions-title">
        <div className="section-heading"><h2 id="sessions-title">CONVERSATIONS</h2><span>Codex</span></div>
        {demo ? <ul className="sessions">{samples.map((s, index) => <li key={s.title}>
          <span className={`status-icon status-${index}`} aria-hidden="true">{s.symbol}</span><div className="session-info"><h3>{hideTitles ? `Session ${index + 1}` : s.title}</h3><p>{hideTitles ? 'Hidden project' : s.project} <span>· {s.status}</span></p></div>
        </li>)}</ul> : <div className="empty session-empty"><span className="empty-icon">◎</span><p>No live session connection</p><p className="muted">Waiting-state detection is under validation.</p></div>}
      </section>
    </>}
    <footer><span><i />{demo ? 'Demo preview' : 'Connections unavailable'}</span><button className="text-button" onClick={() => setDemo(!demo)}>{demo ? 'Exit demo' : 'Preview demo'}</button></footer>
  </main>;
}
