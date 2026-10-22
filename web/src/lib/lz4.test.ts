import lz4js from 'lz4js';
import { describe, expect, it } from 'vitest';
import { decompressLz4Frame } from './lz4';

function pseudoRandomBytes(length: number, seed = 1): Uint8Array {
  const out = new Uint8Array(length);
  let x = seed;
  for (let i = 0; i < length; i++) {
    x = (x * 1_103_515_245 + 12_345) >>> 0;
    out[i] = x >>> 24;
  }
  return out;
}

function repetitiveBytes(length: number): Uint8Array {
  const text = 'customer_name,country,city,category,product;'.repeat(Math.ceil(length / 44));
  return new TextEncoder().encode(text).subarray(0, length);
}

/** Builds a frame by hand so header flags lz4js never writes can be covered. */
function frame(flg: number, blocks: Uint8Array[], extraHeader: number[] = [], trailer: number[] = []): Uint8Array {
  const parts: number[] = [0x04, 0x22, 0x4d, 0x18, flg, 0x40, ...extraHeader, 0x00];
  for (const block of blocks) parts.push(...block);
  parts.push(0, 0, 0, 0, ...trailer);
  return Uint8Array.from(parts);
}

function u32(n: number): number[] {
  return [n & 0xff, (n >>> 8) & 0xff, (n >>> 16) & 0xff, (n >>> 24) & 0xff];
}

const ascii = (s: string) => [...s].map((c) => c.charCodeAt(0));

/** Element-wise comparison; `toEqual` on multi-megabyte arrays is very slow. */
function expectSameBytes(actual: Uint8Array, expected: Uint8Array) {
  expect(actual.length).toBe(expected.length);
  const mismatch = actual.findIndex((byte, i) => byte !== expected[i]);
  expect(mismatch).toBe(-1);
}

const decodeText = (bytes: Uint8Array) => new TextDecoder().decode(bytes);

describe('decompressLz4Frame', () => {
  it.each([
    ['empty', new Uint8Array(0)],
    ['tiny', Uint8Array.from([1, 2, 3])],
    ['repetitive', repetitiveBytes(200_000)],
    ['random (stored uncompressed)', pseudoRandomBytes(100_000)],
    ['multi-block (> 4 MiB)', repetitiveBytes(9_000_000)],
  ])('round-trips %s data compressed by lz4js', (_, input) => {
    const compressed = Uint8Array.from(lz4js.compress(input));
    expectSameBytes(decompressLz4Frame(compressed), input);
  });

  it('handles overlapping matches (run-length style)', () => {
    const input = new Uint8Array(10_000).fill(7);
    const compressed = Uint8Array.from(lz4js.compress(input));
    expect(compressed.length).toBeLessThan(200);
    expectSameBytes(decompressLz4Frame(compressed), input);
  });

  it('lets a block reference data from the previous block (linked blocks)', () => {
    // FLG 0x40: version 1, linked blocks, no checksums.
    const block1 = [...u32(9), 0x80, ...ascii('abcdefgh')];
    // 0 literals + match (offset 8, length 8), then 1 literal.
    const block2 = [...u32(5), 0x04, 0x08, 0x00, 0x10, ...ascii('z')];
    const bytes = frame(0x40, [Uint8Array.from(block1), Uint8Array.from(block2)]);
    expect(decodeText(decompressLz4Frame(bytes))).toBe('abcdefghabcdefghz');
  });

  it('skips content size, dictionary id, block and content checksums', () => {
    // FLG: version | block checksum | content size | content checksum | dict id
    const flg = 0x40 | 0x10 | 0x08 | 0x04 | 0x01;
    const contentSize = [...u32(5), 0, 0, 0, 0];
    const dictId = u32(0xdeadbeef);
    const block = Uint8Array.from([...u32(0x80000000 | 5), ...ascii('hello'), ...u32(0x12345678)]);
    const bytes = frame(flg, [block], [...contentSize, ...dictId], u32(0xcafef00d));
    expect(decodeText(decompressLz4Frame(bytes))).toBe('hello');
  });

  it('decodes concatenated frames and skips skippable frames', () => {
    const a = Uint8Array.from(lz4js.compress(new TextEncoder().encode('first ')));
    const skippable = Uint8Array.from([0x5a, 0x2a, 0x4d, 0x18, ...u32(3), 1, 2, 3]);
    const b = Uint8Array.from(lz4js.compress(new TextEncoder().encode('second')));
    const joined = new Uint8Array([...a, ...skippable, ...b]);
    expect(decodeText(decompressLz4Frame(joined))).toBe('first second');
  });

  it('rejects bad input', () => {
    expect(() => decompressLz4Frame(Uint8Array.from([1, 2, 3, 4, 5, 6, 7]))).toThrow(/magic/);
    const truncated = Uint8Array.from(lz4js.compress(repetitiveBytes(1000))).subarray(0, 20);
    expect(() => decompressLz4Frame(truncated)).toThrow();
    // A match pointing before the start of the output.
    const badOffset = frame(0x40, [Uint8Array.from([...u32(4), 0x10, 0x61, 0x09, 0x00])]);
    expect(() => decompressLz4Frame(badOffset)).toThrow(/offset/);
  });
});
