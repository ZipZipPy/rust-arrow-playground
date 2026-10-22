import { useCallback, useEffect, useState } from 'react';
import { fetchInfo } from '../lib/api';
import { errorMessage } from '../lib/errors';
import type { ApiInfo } from '../lib/types';

export interface ServerInfoState {
  info: ApiInfo | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/** Loads `/api/info` for the current API base; reloads when the base changes. */
export function useServerInfo(base: string): ServerInfoState {
  const [info, setInfo] = useState<ApiInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    setError(null);
    fetchInfo(base, controller.signal)
      .then((next) => {
        setInfo(next);
        setLoading(false);
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setInfo(null);
        setError(errorMessage(e));
        setLoading(false);
      });
    return () => controller.abort();
  }, [base, generation]);

  const reload = useCallback(() => setGeneration((g) => g + 1), []);
  return { info, error, loading, reload };
}
