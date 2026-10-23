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

/** The answer to one `/api/info` request, tagged with the request it answers. */
export interface InfoAnswer {
  base: string;
  generation: number;
  info: ApiInfo | null;
  error: string | null;
}

/**
 * Loads `/api/info` for the current API base; reloads when the base changes.
 * Info belongs to the base it came from: right after a switch, `info` is null
 * (so nothing runs against the new server with the old one's limits, columns
 * or key range) until the new server answers.
 */
export function useServerInfo(base: string): ServerInfoState {
  const [answer, setAnswer] = useState<InfoAnswer | null>(null);
  const [generation, setGeneration] = useState(0);

  useEffect(() => {
    const controller = new AbortController();
    fetchInfo(base, controller.signal)
      .then((info) => {
        if (!controller.signal.aborted) setAnswer({ base, generation, info, error: null });
      })
      .catch((e: unknown) => {
        if (!controller.signal.aborted) setAnswer({ base, generation, info: null, error: errorMessage(e) });
      });
    return () => controller.abort();
  }, [base, generation]);

  const reload = useCallback(() => setGeneration((g) => g + 1), []);
  return { ...infoFor(answer, base, generation), reload };
}

/** What the latest answer means for the current request (`base`, `generation`). */
export function infoFor(answer: InfoAnswer | null, base: string, generation: number): Omit<ServerInfoState, 'reload'> {
  const sameBase = answer?.base === base;
  const current = sameBase && answer.generation === generation;
  return {
    // A reload of the same base keeps showing what it had until the answer arrives.
    info: sameBase ? answer.info : null,
    error: current ? answer.error : null,
    loading: !current,
  };
}
