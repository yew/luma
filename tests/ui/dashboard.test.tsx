import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Settings, defaultPreferences } from '../../src/Settings';
import { HistorySettings } from '../../src/HistorySettings';
import { App } from '../../src/App';
import { GitHubUsage, type Auth } from '../../src/GitHubUsage';

const bridge = vi.hoisted(() => ({ invoke: vi.fn(), native: true, listener: null as null | ((event: { payload: Auth }) => void), codexListener: null as null | ((event: { payload: any }) => void), unlisten: vi.fn() }));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ startDragging: vi.fn().mockResolvedValue(undefined) }) }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: bridge.invoke, isTauri: () => bridge.native }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async (_name, callback) => { if (_name === 'github-status') bridge.listener = callback; if (_name === 'codex-status') bridge.codexListener = callback; return bridge.unlisten; }) }));
const disconnected = (revision = 1): Auth => ({ revision, state: 'disconnected', next_refresh_at: null, account: null, snapshot: null, error: null });
const connected = (revision = 2): Auth => ({ ...disconnected(revision), state: 'connected', account: { id: 42, login: 'account-a' }, snapshot: {
  account_id: 42, login: 'account-a', plan: 'enterprise', reset_at: null, fetched_at: Date.now(), used_percent: 36.1171,
  premium: { entitlement: 2000000, credits_used: 722342, quota_remaining: 1277551.1, percent_remaining: 63.8, overage_permitted: true, unlimited: false },
} });
async function flush() { await act(async () => { await Promise.resolve(); }); }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; }

beforeEach(() => {
  vi.useFakeTimers(); bridge.native = true; bridge.listener = null;
  bridge.invoke.mockReset();
  bridge.invoke.mockImplementation(async command => {
    if (command === 'github_status') return disconnected();
    if (command === 'history_settings') return { enabled: true, retention_days: 90 };
    return undefined;
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe('history privacy controls', () => {
  it('cancels account deletion confirmation on disconnect and never turns it into global deletion', async () => {
    const view = render(<HistorySettings account={{ id: 42, login: 'account-a' }}/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Delete account-a history' }));
    expect(screen.getByRole('button', { name: 'Delete history' })).toBeTruthy();
    view.rerender(<HistorySettings account={null}/>); await flush();
    expect(screen.queryByRole('button', { name: 'Delete history' })).toBeNull();
    expect(bridge.invoke.mock.calls.some(([name]) => name === 'clear_usage_history')).toBe(false);
  });
  it('deletes only the confirmed account and keeps cache clearing independent', async () => {
    render(<HistorySettings account={{ id: 42, login: 'account-a' }}/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Delete account-a history' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete history' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledWith('clear_usage_history', { account: { provider: 'github_copilot', host: 'api.github.com', account_id: '42' } });
    fireEvent.click(screen.getByRole('button', { name: 'Clear usage cache' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledWith('clear_usage_cache', { account: null });
  });
});

describe('proxy settings', () => {
  it('saves only on explicit submit and Direct discards an invalid unsaved URL', async () => {
    const save = vi.fn().mockResolvedValue(undefined);
    render(<Settings preferences={defaultPreferences} onPreferencesChange={save} demo={false} onDemoChange={() => {}}/>);
    fireEvent.change(screen.getByLabelText('Connection'), { target: { value: 'http' } });
    fireEvent.change(screen.getByLabelText('Proxy server'), { target: { value: 'invalid-address' } });
    expect(save).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText('Connection'), { target: { value: 'direct' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save proxy' })); await flush();
    expect(save).toHaveBeenCalledWith({ proxy: { mode: 'direct', server: '' } });
  });
  it('reports save failure and does not claim success', async () => {
    render(<Settings preferences={defaultPreferences} onPreferencesChange={async () => { throw 'Unable to save local preferences.'; }} demo={false} onDemoChange={() => {}}/>);
    fireEvent.click(screen.getByRole('button', { name: 'Save proxy' })); await flush();
    expect(screen.getByRole('alert').textContent).toContain('Unable to save');
    expect(screen.queryByText(/Proxy saved/)).toBeNull();
  });
});

describe('GitHub connection lifecycle', () => {
  it('restores current backend status when the component remounts', async () => {
    let current = disconnected();
    bridge.invoke.mockImplementation(async command => command === 'github_status' ? current : undefined);
    const first = render(<GitHubUsage/>); await flush();
    first.unmount(); current = connected();
    render(<GitHubUsage/>); await flush();
    expect(screen.getByText(/account-a/)).toBeTruthy();
    expect(screen.getByRole('progressbar').getAttribute('value')).toBe('36.1171');
  });
  it('ignores late focus snapshots after a newer disconnect event', async () => {
    bridge.invoke.mockResolvedValue(connected());
    render(<GitHubUsage/>); await flush();
    const old = deferred<Auth>(); bridge.invoke.mockImplementation(() => old.promise);
    fireEvent.focus(window);
    act(() => bridge.listener?.({ payload: disconnected(10) }));
    await act(async () => old.resolve(connected(3)));
    expect(screen.queryByText(/account-a/)).toBeNull();
    expect(screen.getByRole('button', { name: 'Connect GitHub' })).toBeTruthy();
  });
  it('canceling sign-in prevents late poll results and further polling', async () => {
    const pending = deferred<unknown>();
    bridge.invoke.mockImplementation(async command => {
      if (command === 'github_status') return disconnected();
      if (command === 'github_begin') return { user_code: 'TEST-CODE', verification_uri: 'https://github.com/login/device', expires_in: 600, interval: 5 };
      if (command === 'github_poll') return pending.promise;
    });
    render(<GitHubUsage/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Connect GitHub' })); await flush();
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    fireEvent.click(screen.getByRole('button', { name: 'Cancel sign-in' })); await flush();
    await act(async () => pending.resolve({ state: 'connected', retry_after: null, status: connected(20) }));
    await act(async () => vi.advanceTimersByTimeAsync(30000));
    expect(screen.queryByText(/account-a/)).toBeNull();
    expect(bridge.invoke.mock.calls.filter(([name]) => name === 'github_poll')).toHaveLength(1);
    expect(bridge.invoke).toHaveBeenCalledWith('github_cancel');
  });
  it('retries secure-storage restoration before fetching usage', async () => {
    let statusCalls = 0;
    bridge.invoke.mockImplementation(async command => {
      if (command === 'github_status') return ++statusCalls === 1 ? { ...disconnected(), state: 'error', error: { code: 'secure_storage', message: 'Unlock credential storage.' } } : connected(2);
    });
    render(<GitHubUsage/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Retry connection' })); await flush();
    expect(screen.getByText(/account-a/)).toBeTruthy();
    expect(bridge.invoke.mock.calls.some(([name]) => name === 'github_refresh')).toBe(false);
  });
  it('retries usage for a restored legacy token whose first identity fetch failed', async () => {
    const offline = { ...disconnected(), state: 'error', error: { code: 'network', message: 'Offline' } };
    let fetched = false;
    bridge.invoke.mockImplementation(async command => {
      if (command === 'github_status') return fetched ? connected(5) : offline;
      if (command === 'github_refresh') { fetched = true; return connected(5).snapshot; }
    });
    render(<GitHubUsage/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Retry connection' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledWith('github_refresh');
    expect(screen.getByText(/account-a/)).toBeTruthy();
  });
  it('subscription failure still loads the backend and retries on focus', async () => {
    const events = await import('@tauri-apps/api/event');
    vi.mocked(events.listen).mockRejectedValueOnce(new Error('IPC listener unavailable'));
    bridge.invoke.mockResolvedValue(connected());
    render(<GitHubUsage/>); await flush();
    expect(screen.getByText(/account-a/)).toBeTruthy();
    fireEvent.focus(window); await flush();
    expect(vi.mocked(events.listen).mock.calls.length).toBeGreaterThanOrEqual(2);
  });
  it('browser preview shows disconnected instead of an endless loading state', async () => {
    bridge.native = false;
    render(<GitHubUsage/>); await flush();
    expect(screen.queryByText('Loading GitHub connection…')).toBeNull();
    expect((screen.getByRole('button', { name: 'Connect GitHub' }) as HTMLButtonElement).disabled).toBe(true);
    expect(bridge.invoke).not.toHaveBeenCalled();
  });
  it('preserves usage on a failed refresh and switches to the new account deadline', async () => {
    let current = connected();
    bridge.invoke.mockImplementation(async command => {
      if (command === 'github_status') return current;
      if (command === 'github_refresh') { current = { ...connected(5), next_refresh_at: Date.now() + 5000, error: { code: 'network', message: 'Offline', retry_after: 5 } }; throw current.error; }
    });
    render(<GitHubUsage/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh' })); await flush();
    expect(screen.getByRole('progressbar')).toBeTruthy();
    expect(screen.getByText('Stale', { exact: false })).toBeTruthy();
    expect((screen.getByRole('button', { name: /Retry in/ }) as HTMLButtonElement).disabled).toBe(true);
    act(() => bridge.listener?.({ payload: { ...connected(6), account: { id: 43, login: 'account-b' }, next_refresh_at: Date.now() + 30000 } }));
    await act(async () => vi.advanceTimersByTimeAsync(31000));
    expect((screen.getByRole('button', { name: 'Refresh' }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe('dashboard preference ordering', () => {
  it('a failed initial edit restores saved preferences instead of leaving defaults', async () => {
    const initial = deferred<unknown>(); let reads = 0;
    bridge.invoke.mockImplementation(async command => {
      if (command === 'get_preferences') return ++reads === 1 ? initial.promise : { ...defaultPreferences, pinned: false };
      if (command === 'update_preferences') throw 'Store unavailable';
      if (command === 'github_status') return disconnected();
      if (command === 'history_settings') return { enabled: true, retention_days: 90 };
    });
    render(<App/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Always on top' })); await flush();
    await act(async () => initial.resolve({ ...defaultPreferences, pinned: false }));
    expect(screen.getByRole('button', { name: 'Always on top' }).getAttribute('aria-pressed')).toBe('false');
    expect(reads).toBe(2);
  });
  it('a slow initial preferences read cannot undo a user edit', async () => {
    const initial = deferred<unknown>();
    bridge.invoke.mockImplementation(async (command, args) => {
      if (command === 'get_preferences') return initial.promise;
      if (command === 'update_preferences') return { ...defaultPreferences, ...args.patch };
      if (command === 'github_status') return disconnected();
      if (command === 'history_settings') return { enabled: true, retention_days: 90 };
    });
    render(<App/>); await flush();
    fireEvent.click(screen.getByRole('button', { name: 'Always on top' })); await flush();
    expect(screen.getByRole('button', { name: 'Always on top' }).getAttribute('aria-pressed')).toBe('false');
    await act(async () => initial.resolve(defaultPreferences));
    expect(screen.getByRole('button', { name: 'Always on top' }).getAttribute('aria-pressed')).toBe('false');
  });
});

describe('Codex snapshot synchronization', () => {
  it('rejects a late old snapshot after a newer hook event', async () => {
    const initial = deferred<unknown>();
    bridge.invoke.mockImplementation(async command => {
      if (command === 'get_preferences') return defaultPreferences;
      if (command === 'github_status') return disconnected();
      if (command === 'codex_status') return initial.promise;
    });
    render(<App/>); await flush();
    act(() => bridge.codexListener?.({ payload: { revision: 5, enabled: true, installed: true, error: null, sessions: [{session_id:'new',turn_id:'turn',title:'Fresh session',project:'project',status:'running',detail:'Observed hook',last_activity:Date.now()}] } }));
    await act(async () => initial.resolve({ revision: 1, enabled: false, installed: false, error: null, sessions: [] }));
    expect(screen.getByText('Fresh session')).toBeTruthy();
  });
});
