import { useSyncExternalStore } from "react";
import { loadRunnerCapacity, type RunnerCapacity } from "./api";

const POLL_MS = 5000;

/** The host's room as the runner last reported it, shared by every view:
 *  one poll runs while anything on screen listens. Null while no runner
 *  reports, so a queued run's wait reason (which only a polling runner keeps
 *  fresh) can be hidden. A failed poll keeps the last answer. */
let current: RunnerCapacity | null = null;
const listeners = new Set<() => void>();
let timer: number | null = null;

export function publishRunnerCapacity(next: RunnerCapacity | null): void {
  if (next === current) return;
  current = next;
  for (const listener of listeners) listener();
}

function refresh(): void {
  Promise.resolve()
    .then(() => loadRunnerCapacity())
    .then(publishRunnerCapacity, () => {});
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (timer === null) {
    refresh();
    timer = window.setInterval(() => {
      if (document.visibilityState === "visible") refresh();
    }, POLL_MS);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer !== null) {
      window.clearInterval(timer);
      timer = null;
    }
  };
}

export function useRunnerCapacity(): RunnerCapacity | null {
  return useSyncExternalStore(subscribe, () => current);
}

/** Whether a runner reports its room, for `waitingFor`. */
export function useRunnerOnline(): boolean {
  return useRunnerCapacity() !== null;
}
