// Human-friendly number formatting. Sizes use decimal units (1 kB = 1000 B)
// so bytes and MB/s line up with what network tools report.

const DASH = '–';

function trimmed(value: number, digits: number): string {
  return value.toLocaleString('en-US', { maximumFractionDigits: digits, minimumFractionDigits: 0 });
}

function fixed(value: number, digits: number): string {
  return value.toLocaleString('en-US', { maximumFractionDigits: digits, minimumFractionDigits: digits });
}

function isMissing(value: number | null | undefined): value is null | undefined {
  return value === null || value === undefined || !Number.isFinite(value);
}

/** 0.42 ms · 4.2 ms · 123 ms · 1.23 s · 12.3 s */
export function formatMs(ms: number | null | undefined): string {
  if (isMissing(ms)) return DASH;
  const abs = Math.abs(ms);
  if (abs >= 10_000) return `${fixed(ms / 1000, 1)} s`;
  if (abs >= 1000) return `${fixed(ms / 1000, 2)} s`;
  if (abs >= 100) return `${fixed(ms, 0)} ms`;
  if (abs >= 10) return `${fixed(ms, 1)} ms`;
  return `${fixed(ms, 2)} ms`;
}

const BYTE_UNITS = ['B', 'kB', 'MB', 'GB', 'TB'];

/** 512 B · 12.3 kB · 4.56 MB */
export function formatBytes(bytes: number | null | undefined): string {
  if (isMissing(bytes)) return DASH;
  let value = bytes;
  let unit = 0;
  while (Math.abs(value) >= 1000 && unit < BYTE_UNITS.length - 1) {
    value /= 1000;
    unit += 1;
  }
  if (unit === 0) return `${fixed(value, 0)} B`;
  const digits = Math.abs(value) >= 100 ? 0 : Math.abs(value) >= 10 ? 1 : 2;
  return `${fixed(value, digits)} ${BYTE_UNITS[unit]}`;
}

/** 1,234,567 */
export function formatInt(value: number | null | undefined): string {
  if (isMissing(value)) return DASH;
  return Math.round(value).toLocaleString('en-US');
}

/** 950 · 12.5k · 1.2M · 3.4B */
export function formatCompact(value: number | null | undefined): string {
  if (isMissing(value)) return DASH;
  const abs = Math.abs(value);
  if (abs >= 1e9) return `${trimmed(value / 1e9, abs >= 1e10 ? 0 : 1)}B`;
  if (abs >= 1e6) return `${trimmed(value / 1e6, abs >= 1e7 ? 0 : 1)}M`;
  if (abs >= 1e3) return `${trimmed(value / 1e3, abs >= 1e4 ? 0 : 1)}k`;
  return trimmed(value, 0);
}

/** 3.2× · 12× · 0.45× */
export function formatTimes(value: number | null | undefined): string {
  if (isMissing(value)) return DASH;
  const digits = value >= 10 ? 0 : value >= 1 ? 1 : 2;
  return `${fixed(value, digits)}×`;
}

/** "3.2× faster" / "1.5× slower" for a speed-up factor relative to a baseline. */
export function describeSpeedup(factor: number | null | undefined): string {
  if (isMissing(factor)) return DASH;
  if (factor >= 1) return `${formatTimes(factor)} faster`;
  return `${formatTimes(1 / factor)} slower`;
}

/** "4.1× smaller" / "1.3× larger" for a size ratio (value / baseline). */
export function describeSizeRatio(ratio: number | null | undefined): string {
  if (isMissing(ratio) || ratio <= 0) return DASH;
  if (ratio <= 1) return `${formatTimes(1 / ratio)} smaller`;
  return `${formatTimes(ratio)} larger`;
}

/** 2.3M rows/s */
export function formatRowsPerSecond(rows: number, ms: number): string {
  if (!(ms > 0)) return DASH;
  return `${formatCompact((rows / ms) * 1000)} rows/s`;
}

/** 45.2 MB/s */
export function formatMegabytesPerSecond(bytes: number | null, ms: number): string {
  if (bytes === null || !(ms > 0)) return DASH;
  const mbps = bytes / 1e6 / (ms / 1000);
  return `${trimmed(mbps, mbps >= 100 ? 0 : 1)} MB/s`;
}

export function formatPercent(fraction: number | null | undefined): string {
  if (isMissing(fraction)) return DASH;
  return `${trimmed(fraction * 100, fraction < 0.1 ? 1 : 0)}%`;
}
