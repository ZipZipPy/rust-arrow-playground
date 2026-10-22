import { describe, expect, it } from 'vitest';
import {
  describeSizeRatio,
  describeSpeedup,
  formatBytes,
  formatCompact,
  formatInt,
  formatMegabytesPerSecond,
  formatMs,
  formatPercent,
  formatRowsPerSecond,
  formatTimes,
} from './format';

describe('formatMs', () => {
  it.each([
    [0.4234, '0.42 ms'],
    [4.256, '4.26 ms'],
    [42.26, '42.3 ms'],
    [123.4, '123 ms'],
    [1234, '1.23 s'],
    [12_345, '12.3 s'],
    [null, '–'],
    [Number.NaN, '–'],
  ])('%s → %s', (input, expected) => {
    expect(formatMs(input)).toBe(expected);
  });
});

describe('formatBytes', () => {
  it.each([
    [0, '0 B'],
    [999, '999 B'],
    [1000, '1.00 kB'],
    [12_345, '12.3 kB'],
    [456_789, '457 kB'],
    [4_560_000, '4.56 MB'],
    [2_500_000_000, '2.50 GB'],
    [null, '–'],
  ])('%s → %s', (input, expected) => {
    expect(formatBytes(input)).toBe(expected);
  });
});

describe('counts and rates', () => {
  it('formats integers with separators', () => {
    expect(formatInt(1_234_567)).toBe('1,234,567');
    expect(formatInt(undefined)).toBe('–');
  });

  it('formats compact counts', () => {
    expect(formatCompact(950)).toBe('950');
    expect(formatCompact(12_500)).toBe('13k');
    expect(formatCompact(1_500)).toBe('1.5k');
    expect(formatCompact(1_000_000)).toBe('1M');
    expect(formatCompact(2_345_678)).toBe('2.3M');
    expect(formatCompact(3_400_000_000)).toBe('3.4B');
  });

  it('formats rows/s and MB/s', () => {
    expect(formatRowsPerSecond(100_000, 50)).toBe('2M rows/s');
    expect(formatRowsPerSecond(100, 0)).toBe('–');
    expect(formatMegabytesPerSecond(45_200_000, 1000)).toBe('45.2 MB/s');
    expect(formatMegabytesPerSecond(500_000_000, 1000)).toBe('500 MB/s');
    expect(formatMegabytesPerSecond(null, 10)).toBe('–');
  });

  it('formats percentages', () => {
    expect(formatPercent(0.5)).toBe('50%');
    expect(formatPercent(0.042)).toBe('4.2%');
  });
});

describe('ratios', () => {
  it('formats multipliers', () => {
    expect(formatTimes(3.24)).toBe('3.2×');
    expect(formatTimes(12.4)).toBe('12×');
    expect(formatTimes(0.456)).toBe('0.46×');
  });

  it('describes speed-ups in both directions', () => {
    expect(describeSpeedup(3.2)).toBe('3.2× faster');
    expect(describeSpeedup(0.5)).toBe('2.0× slower');
    expect(describeSpeedup(null)).toBe('–');
  });

  it('describes size ratios in both directions', () => {
    expect(describeSizeRatio(0.25)).toBe('4.0× smaller');
    expect(describeSizeRatio(1.5)).toBe('1.5× larger');
    expect(describeSizeRatio(0)).toBe('–');
  });
});
