/** P0 diagnostic only: never retain transcript bodies or tool arguments. */
export function normalizeEvent(record) {
  if (record?.type !== 'event_msg' || !record.payload) return null;
  const p = record.payload;
  if (!['task_started', 'task_complete'].includes(p.type)) return null;
  if (typeof p.turn_id !== 'string' || !p.turn_id) return null;
  const timestamp = Date.parse(record.timestamp);
  if (!Number.isFinite(timestamp)) return null;
  let status = 'running';
  if (p.type === 'task_complete') {
    // This installed version omits error on successful task_complete events.
    status = p.error == null ? 'completed' : 'failed';
  }
  return { turnId: p.turn_id, timestamp, type: p.type, status };
}

export function reduceEvent(state, event) {
  if (!event) return state;
  // Consume a single file in append order; equal timestamps preserve that order.
  if (state && event.timestamp < state.timestamp) return state;
  if (event.type === 'task_started') {
    if (state?.turnId === event.turnId) return state;
    return { turnId: event.turnId, timestamp: event.timestamp, status: 'running' };
  }
  // A late completion from an older run must not overwrite a newer run.
  if (state && state.turnId !== event.turnId) return state;
  return { turnId: event.turnId, timestamp: event.timestamp, status: event.status };
}
