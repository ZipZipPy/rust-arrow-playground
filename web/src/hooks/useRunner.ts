import { useCallback, useRef, useState } from 'react';
import { errorMessage } from '../lib/errors';
import type { Progress } from '../lib/runner';

export type ActivityKind = 'run' | 'compare' | 'bench';

export interface Activity {
  kind: ActivityKind;
  progress: Progress | null;
}

export type Task = (signal: AbortSignal, report: (progress: Progress) => void) => Promise<void>;

export interface Runner {
  activity: Activity | null;
  error: string | null;
  start: (kind: ActivityKind, task: Task) => void;
  cancel: () => void;
  dismissError: () => void;
}

/** Runs one cancellable task at a time and tracks its progress and failure. */
export function useRunner(): Runner {
  const [activity, setActivity] = useState<Activity | null>(null);
  const [error, setError] = useState<string | null>(null);
  const controllerRef = useRef<AbortController | null>(null);

  const start = useCallback((kind: ActivityKind, task: Task) => {
    if (controllerRef.current) return;
    const controller = new AbortController();
    controllerRef.current = controller;
    setError(null);
    setActivity({ kind, progress: null });
    task(controller.signal, (progress) => setActivity({ kind, progress }))
      .catch((e: unknown) => {
        if (!controller.signal.aborted) setError(errorMessage(e));
      })
      .finally(() => {
        controllerRef.current = null;
        setActivity(null);
      });
  }, []);

  const cancel = useCallback(() => controllerRef.current?.abort(), []);
  const dismissError = useCallback(() => setError(null), []);

  return { activity, error, start, cancel, dismissError };
}
