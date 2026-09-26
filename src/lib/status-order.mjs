// Reject command responses arriving after newer backend events.
export function createStatusGate() {
  let revision = -1;
  return status => {
    if (status.revision < revision) return false;
    revision = status.revision;
    return true;
  };
}
