import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { PlannedRequest } from './plan';
import { executePlan, type ExecuteOptions } from './runner';
import { buildRequest, DEFAULT_SETTINGS } from './settings';

const request = buildRequest({ ...DEFAULT_SETTINGS, cache: true }, 'arrow', 0, null);

function run(plan: PlannedRequest[], signal = new AbortController().signal): Promise<void> {
  const opts: ExecuteOptions = {
    base: '',
    target: 'same origin',
    region: null,
    kind: 'servers',
    batchId: 1,
    plan,
    columns: [],
    useScope: 'all',
    offsetMode: 'fixed',
    signal,
    onProgress: () => {},
    onMeasurement: () => {},
    onPreview: () => {},
  };
  return executePlan(opts);
}

describe('executePlan', () => {
  beforeEach(() => {
    vi.stubGlobal('requestAnimationFrame', (callback: () => void) => setTimeout(callback, 0));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('names the server when a request of a multi-server plan fails', async () => {
    const fetch = vi.fn(async () => Response.json({ error: 'too many concurrent large requests' }, { status: 503 }));
    vi.stubGlobal('fetch', fetch);
    const server = { base: 'https://playground-abc-uc.a.run.app', label: 'us-central1', region: 'us-central1' };
    await expect(run([{ request, role: 'prime', round: 0, server }])).rejects.toThrow(
      'us-central1 (playground-abc-uc.a.run.app): 503: too many concurrent large requests',
    );
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('leaves the error of a single-server plan as it is', async () => {
    vi.stubGlobal('fetch', async () => Response.json({ error: 'rows must be at most 1000000' }, { status: 400 }));
    await expect(run([{ request, role: 'prime', round: 0 }])).rejects.toThrow(/^400: rows must be at most 1000000$/);
  });

  it('does not wrap a cancellation', async () => {
    const controller = new AbortController();
    vi.stubGlobal('fetch', async (_url: string, init: RequestInit) => {
      controller.abort();
      throw init.signal?.reason;
    });
    const server = { base: 'https://playground-abc-uc.a.run.app', label: 'us-central1', region: null };
    await expect(run([{ request, role: 'prime', round: 0, server }], controller.signal)).rejects.toMatchObject({
      name: 'AbortError',
    });
  });
});
