import { createReadStream } from 'node:fs';
import { createInterface } from 'node:readline';
import { normalizeEvent, reduceEvent } from './codex-events.mjs';

// Explicit input only: do not crawl conversation directories or print contents.
const path = process.argv[2];
if (!path) {
  console.error('Usage: node tools/inspect-codex-events.mjs <session.jsonl>');
  process.exitCode = 2;
} else {
  const counts = { started: 0, completed: 0, failed: 0, malformed: 0 };
  let state = null;
  try {
    for await (const line of createInterface({ input: createReadStream(path), crlfDelay: Infinity })) {
      let record;
      try { record = JSON.parse(line); } catch { counts.malformed++; continue; }
      const event = normalizeEvent(record);
      if (!event) continue;
      counts[event.status === 'running' ? 'started' : event.status]++;
      state = reduceEvent(state, event);
    }
    console.log(JSON.stringify({ counts, lastRecordedStatus: state?.status ?? 'unknown',
      liveStatus: 'unknown', waitingDetection: 'not_available_from_this_probe',
      note: 'Historical evidence only. File silence cannot establish current running/waiting state.' }, null, 2));
  } catch {
    console.error('Unable to inspect the selected session file. No transcript was emitted.');
    process.exitCode = 1;
  }
}
