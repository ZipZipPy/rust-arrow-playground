import { useCallback, useEffect, useRef, useState } from 'react';
import { pingOnce } from '../lib/measure';
import { median } from '../lib/stats';

const PING_COUNT = 5;

export interface RttState {
  medianMs: number | null;
  samples: number[];
  measuring: boolean;
  error: string | null;
  measure: () => void;
}

/** Median of 5 sequential `/api/ping` round trips, measured once the server is reachable. */
export function useRtt(base: string, enabled: boolean): RttState {
  const [samples, setSamples] = useState<number[]>([]);
  const [measuring, setMeasuring] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const controllerRef = useRef<AbortController | null>(null);

  const measure = useCallback(() => {
    controllerRef.current?.abort();
    const controller = new AbortController();
    controllerRef.current = controller;
    setMeasuring(true);
    setError(null);
    (async () => {
      const next: number[] = [];
      for (let i = 0; i < PING_COUNT; i++) next.push(await pingOnce(base, controller.signal));
      return next;
    })()
      .then((next) => {
        if (controller.signal.aborted) return;
        setSamples(next);
        setMeasuring(false);
      })
      .catch((e: unknown) => {
        if (controller.signal.aborted) return;
        setSamples([]);
        setError(e instanceof Error ? e.message : String(e));
        setMeasuring(false);
      });
  }, [base]);

  useEffect(() => {
    setSamples([]);
    if (enabled) measure();
    return () => controllerRef.current?.abort();
  }, [enabled, measure]);

  return { medianMs: samples.length ? median(samples) : null, samples, measuring, error, measure };
}
