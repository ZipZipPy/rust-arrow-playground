// localStorage is a per-browser convenience here. It can be missing or throw
// (private windows, blocked site data, quota), so every access is guarded and
// the app falls back to defaults.

export const STORAGE_KEYS = {
  apiBase: 'rap.apiBase',
  settings: 'rap.settings.v1',
  history: 'rap.history.v1',
  theme: 'rap.theme',
} as const;

export function readStored<T>(key: string, parse: (raw: unknown) => T, fallback: T): T {
  try {
    const text = globalThis.localStorage?.getItem(key);
    if (text === null || text === undefined) return fallback;
    return parse(JSON.parse(text));
  } catch {
    return fallback;
  }
}

export function writeStored(key: string, value: unknown): void {
  try {
    globalThis.localStorage?.setItem(key, JSON.stringify(value));
  } catch {
    // Storage unavailable or full: the setting simply won't survive a reload.
  }
}
