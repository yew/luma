import { useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import './CodexSessions.css';

export type CodexSession = {
  session_id: string;
  turn_id: string | null;
  title: string;
  project: string | null;
  status: 'running' | 'waiting' | 'stopped' | 'canceled' | 'unknown';
  detail: string;
  last_activity: number;
};

export type CodexStatus = {
  revision?: number;
  enabled: boolean;
  installed: boolean;
  sessions: CodexSession[];
  error: string | null;
};

const states = {
  waiting: { label: 'Waiting', symbol: '◷', order: 0 },
  running: { label: 'Running', symbol: '●', order: 1 },
  stopped: { label: 'Turn stopped', symbol: '■', order: 2 },
  canceled: { label: 'Canceled', symbol: '○', order: 2 },
  unknown: { label: 'Unknown', symbol: '?', order: 3 },
};

function eventTime(timestamp: number, now: number) {
  const seconds = Math.max(0, Math.floor((now - timestamp) / 1000));
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return Math.floor(seconds / 60) + 'm ago';
  if (seconds < 86400) return Math.floor(seconds / 3600) + 'h ago';
  return Math.floor(seconds / 86400) + 'd ago';
}

function connectionLabel(status: CodexStatus) {
  if (!status.enabled) return 'Monitoring disabled';
  if (!status.installed) return 'Hook installation missing';
  return status.sessions.length ? 'Codex hooks connected' : 'Waiting for first event';
}

export function CodexSessions({ status, hideTitles, hidePaths, now }: {
  status: CodexStatus | null;
  hideTitles: boolean;
  hidePaths: boolean;
  now: number;
}) {
  if (!status) return <div className="empty session-empty"><p>{isTauri() ? 'Loading Codex monitoring…' : 'Codex monitoring is available in the desktop app.'}</p></div>;
  const sessions = [...status.sessions].sort((left, right) => states[left.status].order - states[right.status].order || right.last_activity - left.last_activity || left.session_id.localeCompare(right.session_id));
  return <div className="codex-monitor">
    {status.error && <p className="notice codex-error" role="alert">{status.error}</p>}
    {(!status.enabled || !status.installed || !sessions.length) && <div className="empty session-empty">
      <p>{connectionLabel(status)}</p>
      <p className="muted">{!status.enabled ? 'Enable Codex hooks in Settings to collect session activity.' : !status.installed ? 'Repair the Codex hooks in Settings to resume collection.' : 'Trust the configuration with /hooks in Codex CLI. Desktop sessions may need to be reopened after setup.'}</p>
    </div>}
    {sessions.length > 0 && <>
      {!status.enabled && <p className="codex-retained">Saved activity · collection is off</p>}
      <ul className="sessions codex-sessions" aria-label="Codex sessions">{sessions.map(session => {
        const state = states[session.status];
        const validTime = Number.isFinite(session.last_activity) && !Number.isNaN(new Date(session.last_activity).getTime());
        return <li key={session.session_id}>
          <span className={'status-icon codex-' + session.status} aria-hidden="true">{state.symbol}</span>
          <div className="session-info">
            <h3>{hideTitles ? 'Codex session' : session.title || 'Codex session'}</h3>
            <p className="codex-state">{state.label}</p>
            <p className="codex-project">{hidePaths ? 'Hidden project' : session.project || 'Project unavailable'}</p>
            <p className="codex-detail">{session.detail || 'No further event details.'}</p>
            {session.status === 'waiting' && <p className="codex-detail">Request observed; waiting state is provisional until a follow-up event.</p>}
            {session.status === 'stopped' && <p className="codex-detail">A stopped turn does not confirm success.</p>}
            <p className="codex-source">Codex hook · {validTime ? <time dateTime={new Date(session.last_activity).toISOString()} title={new Date(session.last_activity).toLocaleString()}>{eventTime(session.last_activity, now)}</time> : 'Event time unavailable'}</p>
          </div>
        </li>;
      })}</ul>
    </>}
  </div>;
}

type CodexCommand = 'codex_enable' | 'codex_disable' | 'codex_clear_sessions' | 'codex_status';

export function CodexSettings({ status, onChange }: {
  status: CodexStatus | null;
  onChange: (status: CodexStatus) => void;
}) {
  const [pending, setPending] = useState<CodexCommand | null>(null);
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [confirmClear, setConfirmClear] = useState(false);
  const native = isTauri();
  async function command(action: CodexCommand) {
    if (pending || !native) return;
    setPending(action); setError(''); setMessage('');
    try {
      const next = await invoke<CodexStatus>(action);
      onChange(next);
      if (action === 'codex_clear_sessions') { setConfirmClear(false); setMessage('Saved Codex sessions cleared.'); }
      if (action === 'codex_disable') setMessage('Collection disabled. Saved sessions are retained.');
      if (action === 'codex_enable' && next.enabled && next.installed) setMessage('Hooks installed. Trust the configuration with /hooks in Codex CLI.');
    } catch (failure) {
      setError(typeof failure === 'string' ? failure : 'Unable to update Codex monitoring. Try again.');
    } finally { setPending(null); }
  }
  return <section className="settings codex-settings" aria-label="Codex monitoring settings">
    <h3>Codex monitoring</h3>
    <p>{native ? status ? connectionLabel(status) : 'Loading Codex monitoring…' : 'Hook monitoring is available in the desktop app.'}</p>
    <p>Enable hooks to collect session status, project names, and event times on this device. Luma adds its handlers to <code>~/.codex/hooks.json</code> and preserves existing handlers.</p>
    <p>In Codex CLI, use <code>/hooks</code> to trust the configuration. Desktop sessions may need to be reopened after setup. Approval requests indicate possible waiting; a stopped turn does not confirm success.</p>
    <div className="codex-actions">
      <button className="primary-button" disabled={!native || !!pending || !status} onClick={() => void command(status?.enabled && status.installed ? 'codex_disable' : 'codex_enable')}>
        {pending === 'codex_enable' ? 'Installing…' : pending === 'codex_disable' ? 'Disabling…' : status?.enabled && status.installed ? 'Disable monitoring' : status?.enabled ? 'Repair hooks' : 'Enable Codex hooks'}
      </button>
      {status?.enabled && !status.installed && <button className="text-button" disabled={!native || !!pending} onClick={() => void command('codex_disable')}>Disable monitoring</button>}
      <button className="text-button" disabled={!native || !!pending} onClick={() => void command('codex_status')}>Refresh hook status</button>
      <button className="text-button" disabled={!native || !!pending || !status?.sessions.length} onClick={() => setConfirmClear(true)}>Clear saved sessions</button>
    </div>
    <p>Disabling stops collection and removes only Luma’s handlers. Clearing saved sessions is separate and leaves monitoring settings unchanged.</p>
    {confirmClear && <div className="codex-confirmation" role="group" aria-label="Confirm session deletion">
      <p>Delete all saved Codex session activity? This cannot be undone. Future events will still be collected while monitoring is enabled.</p>
      <div className="codex-actions"><button className="primary-button" disabled={!!pending} onClick={() => void command('codex_clear_sessions')}>{pending === 'codex_clear_sessions' ? 'Clearing…' : 'Delete saved sessions'}</button><button className="text-button" disabled={!!pending} onClick={() => setConfirmClear(false)}>Cancel</button></div>
    </div>}
    {(error || status?.error) && <p className="codex-error" role="alert">{error || status?.error}</p>}
    {message && <p role="status">{message}</p>}
  </section>;
}
