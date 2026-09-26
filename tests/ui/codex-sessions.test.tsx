import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CodexSessions, CodexSettings, type CodexSession, type CodexStatus } from '../../src/CodexSessions';

const bridge = vi.hoisted(() => ({ invoke: vi.fn(), native: true }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: bridge.invoke, isTauri: () => bridge.native }));
const now = Date.UTC(2026, 8, 26, 12);
function session(id: string, status: CodexSession['status'], last_activity = now - 60000): CodexSession {
  return { session_id: id, turn_id: 'turn-' + id, title: 'Session ' + id, project: '/workspace/' + id, status, detail: 'Recorded ' + id + ' event', last_activity };
}
function status(patch: Partial<CodexStatus> = {}): CodexStatus {
  return { enabled: false, installed: false, sessions: [], error: null, ...patch };
}
async function flush() { await act(async () => { await Promise.resolve(); }); }
beforeEach(() => { bridge.invoke.mockReset(); bridge.native = true; });
afterEach(cleanup);

describe('Codex session metadata', () => {
  it('removes a card when source reconciliation removes it from the snapshot', () => {
    const status = { enabled:true, installed:true, error:null, sessions:[{ session_id:'deleted',turn_id:'turn',title:'Deleted conversation',project:null,status:'stopped' as const,detail:'Turn stopped',last_activity:1000 }] };
    const view=render(<CodexSessions status={status} hideTitles={false} hidePaths={false} now={2000}/>);
    expect(screen.getByText('Deleted conversation')).toBeTruthy();
    view.rerender(<CodexSessions status={{...status,sessions:[]}} hideTitles={false} hidePaths={false} now={3000}/>);
    expect(screen.queryByText('Deleted conversation')).toBeNull();
  });
  it('orders waiting, running, stopped/canceled, unknown, then most recent without mutating source', () => {
    const sessions = [session('unknown', 'unknown', now), session('old-running', 'running', now - 120000), session('stopped', 'stopped'), session('waiting', 'waiting'), session('new-running', 'running'), session('canceled', 'canceled', now)];
    render(<CodexSessions status={status({ enabled: true, installed: true, sessions })} hideTitles={false} hidePaths={false} now={now}/>);
    const rows = screen.getAllByRole('listitem');
    expect(rows.map(row => within(row).getByRole('heading').textContent)).toEqual(['Session waiting', 'Session new-running', 'Session old-running', 'Session canceled', 'Session stopped', 'Session unknown']);
    expect(sessions[0].session_id).toBe('unknown');
    expect(screen.getByText('Turn stopped')).toBeTruthy();
    expect(screen.queryByText('Completed')).toBeNull();
    expect(screen.getByText('A stopped turn does not confirm success.')).toBeTruthy();
    expect(screen.getByText('Request observed; waiting state is provisional until a follow-up event.')).toBeTruthy();
    expect(screen.getByText('Recorded waiting event')).toBeTruthy();
    expect(rows[0].querySelector('time')?.getAttribute('datetime')).toBe(new Date(now - 60000).toISOString());
    expect(rows[0].querySelector('time')?.textContent).toBe('1m ago');
    expect(rows[0].textContent).toContain('Codex hook');
  });
  it('hides titles and paths from the DOM while preserving status and time', () => {
    const view = render(<CodexSessions status={status({ enabled: true, installed: true, sessions: [session('private-title', 'running')] })} hideTitles={true} hidePaths={true} now={now}/>);
    expect(screen.getByRole('heading').textContent).toBe('Codex session');
    expect(screen.getByText('Hidden project')).toBeTruthy();
    expect(view.container.querySelector('h3')?.outerHTML).not.toContain('private-title');
    expect(view.container.innerHTML).not.toContain('/workspace/private-title');
    expect(screen.getByText('Running')).toBeTruthy();
    expect(view.container.querySelector('time')).toBeTruthy();
  });
  it('distinguishes missing installation, pending events, disabled collection, and backend errors', () => {
    const view = render(<CodexSessions status={status({ enabled: true, installed: true })} hideTitles={false} hidePaths={false} now={now}/>);
    expect(screen.getByText('Waiting for first event')).toBeTruthy();
    view.rerender(<CodexSessions status={status({ enabled: true, error: 'Hooks file changed.' })} hideTitles={false} hidePaths={false} now={now}/>);
    expect(screen.getByText('Hook installation missing')).toBeTruthy();
    expect(screen.getByRole('alert').textContent).toBe('Hooks file changed.');
    view.rerender(<CodexSessions status={status({ sessions: [session('old', 'stopped')] })} hideTitles={false} hidePaths={false} now={now}/>);
    expect(screen.getByText('Monitoring disabled')).toBeTruthy();
    expect(screen.getByText('Saved activity · collection is off')).toBeTruthy();
    expect(screen.getByText('Session old')).toBeTruthy();
  });
});

describe('Codex hook controls', () => {
  it('installs only on explicit enable and passes the returned status to its owner', async () => {
    const onChange = vi.fn();
    const active = status({ enabled: true, installed: true });
    bridge.invoke.mockResolvedValue(active);
    render(<CodexSettings status={status()} onChange={onChange}/>);
    expect(bridge.invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Enable Codex hooks' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledExactlyOnceWith('codex_enable');
    expect(onChange).toHaveBeenCalledWith(active);
    expect(screen.getByRole('status').textContent).toContain('/hooks in Codex CLI');
  });
  it('disables collection without clearing saved sessions', async () => {
    const saved = [session('retained', 'stopped')];
    const onChange = vi.fn();
    bridge.invoke.mockResolvedValue(status({ sessions: saved }));
    render(<CodexSettings status={status({ enabled: true, installed: true, sessions: saved })} onChange={onChange}/>);
    fireEvent.click(screen.getByRole('button', { name: 'Disable monitoring' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledExactlyOnceWith('codex_disable');
    expect(onChange).toHaveBeenCalledWith(status({ sessions: saved }));
    expect(screen.getByRole('status').textContent).toContain('Saved sessions are retained');
  });
  it('keeps failed setup visible without announcing success', async () => {
    bridge.invoke.mockRejectedValue('Unable to update hooks.json.');
    const onChange = vi.fn();
    render(<CodexSettings status={status()} onChange={onChange}/>);
    fireEvent.click(screen.getByRole('button', { name: 'Enable Codex hooks' })); await flush();
    expect(screen.getByRole('alert').textContent).toBe('Unable to update hooks.json.');
    expect(screen.queryByRole('status')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });
  it('requires separate deletion confirmation and can cancel it', async () => {
    const active = status({ enabled: true, installed: true, sessions: [session('saved', 'stopped')] });
    bridge.invoke.mockResolvedValue({ ...active, sessions: [] });
    render(<CodexSettings status={active} onChange={vi.fn()}/>);
    fireEvent.click(screen.getByRole('button', { name: 'Clear saved sessions' }));
    expect(bridge.invoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('group', { name: 'Confirm session deletion' })).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Clear saved sessions' }));
    fireEvent.click(screen.getByRole('button', { name: 'Delete saved sessions' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledExactlyOnceWith('codex_clear_sessions');
    expect(screen.getByRole('status').textContent).toBe('Saved Codex sessions cleared.');
  });
  it('offers repair and explicit status refresh for a missing hook installation', async () => {
    const onChange = vi.fn();
    const active = status({ enabled: true, installed: true });
    bridge.invoke.mockResolvedValue(active);
    render(<CodexSettings status={status({ enabled: true })} onChange={onChange}/>);
    expect(screen.getByRole('button', { name: 'Repair hooks' })).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Disable monitoring' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Refresh hook status' })); await flush();
    expect(bridge.invoke).toHaveBeenCalledExactlyOnceWith('codex_status');
    expect(onChange).toHaveBeenCalledWith(active);
  });
  it('does not call the native bridge in browser preview', () => {
    bridge.native = false;
    render(<CodexSettings status={status()} onChange={vi.fn()}/>);
    for (const button of screen.getAllByRole('button')) expect((button as HTMLButtonElement).disabled).toBe(true);
    expect(bridge.invoke).not.toHaveBeenCalled();
    expect(screen.getByText('Hook monitoring is available in the desktop app.')).toBeTruthy();
  });
});
