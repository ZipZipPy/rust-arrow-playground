import { describe, expect, it } from 'vitest';
import { makeInfo } from '../test/serverInfo';
import { infoFor } from './useServerInfo';

const info = makeInfo();

describe('infoFor', () => {
  it('is loading until the first answer', () => {
    expect(infoFor(null, '', 0)).toEqual({ info: null, error: null, loading: true });
  });

  it('shows the answer for the current base', () => {
    expect(infoFor({ base: '', generation: 0, info, error: null }, '', 0)).toEqual({ info, error: null, loading: false });
  });

  it('drops the old server’s info as soon as the base changes', () => {
    const old = { base: '', generation: 0, info, error: null };
    expect(infoFor(old, 'https://svc.run.app', 0)).toEqual({ info: null, error: null, loading: true });
  });

  it('keeps the info of the same base while it reloads, and hides a stale error', () => {
    expect(infoFor({ base: '', generation: 0, info, error: null }, '', 1)).toEqual({ info, error: null, loading: true });
    expect(infoFor({ base: '', generation: 0, info: null, error: 'down' }, '', 1)).toEqual({ info: null, error: null, loading: true });
    expect(infoFor({ base: '', generation: 1, info: null, error: 'down' }, '', 1)).toEqual({ info: null, error: 'down', loading: false });
  });
});
