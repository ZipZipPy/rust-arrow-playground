// LZ4 frame decompressor (https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md).
// Arrow IPC body compression "LZ4_FRAME" wraps each buffer in one LZ4 frame.
// Checksums are skipped: the payload already traveled over TCP/TLS.

const FRAME_MAGIC = 0x184d2204;
const SKIPPABLE_MAGIC = 0x184d2a50;
const SKIPPABLE_MASK = 0xfffffff0;
const UNCOMPRESSED_BLOCK = 0x80000000;
const MIN_MATCH = 4;

const FLG_DICT_ID = 0x01;
const FLG_CONTENT_CHECKSUM = 0x04;
const FLG_CONTENT_SIZE = 0x08;
const FLG_BLOCK_CHECKSUM = 0x10;

const BLOCK_MAX_SIZES: Record<number, number> = { 4: 64 << 10, 5: 256 << 10, 6: 1 << 20, 7: 4 << 20 };

interface FrameHeader {
  blockChecksum: boolean;
  contentChecksum: boolean;
  contentSize: number | null;
  blockMaxSize: number;
  /** Offset of the first block header. */
  dataStart: number;
}

function readU32(src: Uint8Array, at: number): number {
  if (at + 4 > src.length) throw new Error('LZ4: unexpected end of input');
  return (src[at]! | (src[at + 1]! << 8) | (src[at + 2]! << 16) | (src[at + 3]! << 24)) >>> 0;
}

function readFrameHeader(src: Uint8Array, at: number): FrameHeader {
  const flg = src[at + 4];
  const bd = src[at + 5];
  if (flg === undefined || bd === undefined) throw new Error('LZ4: truncated frame header');
  if (flg >> 6 !== 1) throw new Error(`LZ4: unsupported frame version ${flg >> 6}`);
  const blockMaxSize = BLOCK_MAX_SIZES[(bd >> 4) & 0x7];
  if (blockMaxSize === undefined) throw new Error('LZ4: invalid block maximum size');
  let pos = at + 6;
  let contentSize: number | null = null;
  if (flg & FLG_CONTENT_SIZE) {
    contentSize = readU32(src, pos) + readU32(src, pos + 4) * 2 ** 32;
    pos += 8;
  }
  if (flg & FLG_DICT_ID) pos += 4;
  pos += 1; // header checksum
  return {
    blockChecksum: (flg & FLG_BLOCK_CHECKSUM) !== 0,
    contentChecksum: (flg & FLG_CONTENT_CHECKSUM) !== 0,
    contentSize,
    blockMaxSize,
    dataStart: pos,
  };
}

/**
 * Walks every frame without decoding to find an upper bound for the output, so
 * the output can be allocated once. Exact when frames declare their content size.
 */
function outputBound(src: Uint8Array): number {
  let bound = 0;
  let pos = 0;
  while (pos < src.length) {
    const magic = readU32(src, pos);
    if ((magic & SKIPPABLE_MASK) === SKIPPABLE_MAGIC) {
      pos += 8 + readU32(src, pos + 4);
      continue;
    }
    if (magic !== FRAME_MAGIC) throw new Error('LZ4: bad frame magic number');
    const header = readFrameHeader(src, pos);
    pos = header.dataStart;
    let frameBound = 0;
    for (;;) {
      const word = readU32(src, pos);
      pos += 4;
      if (word === 0) break;
      const size = word & ~UNCOMPRESSED_BLOCK;
      frameBound += word & UNCOMPRESSED_BLOCK ? size : header.blockMaxSize;
      pos += size + (header.blockChecksum ? 4 : 0);
    }
    if (header.contentChecksum) pos += 4;
    bound += header.contentSize ?? frameBound;
  }
  return bound;
}

/** Decodes one LZ4 block from `src[start, end)` into `dst` at `out`; returns the new output position. */
export function decompressBlock(src: Uint8Array, start: number, end: number, dst: Uint8Array, out: number): number {
  let pos = start;
  while (pos < end) {
    const token = src[pos++]!;

    let literals = token >>> 4;
    if (literals === 15) {
      let b: number;
      do {
        b = src[pos++]!;
        literals += b;
      } while (b === 255 && pos < end);
    }
    if (pos + literals > end || out + literals > dst.length) throw new Error('LZ4: literal run out of bounds');
    if (literals < 32) {
      for (let i = 0; i < literals; i++) dst[out++] = src[pos++]!;
    } else {
      dst.set(src.subarray(pos, pos + literals), out);
      pos += literals;
      out += literals;
    }
    // The last sequence of a block carries literals only.
    if (pos >= end) break;

    const offset = src[pos]! | (src[pos + 1]! << 8);
    pos += 2;
    if (offset === 0 || offset > out) throw new Error('LZ4: invalid match offset');

    let length = token & 15;
    if (length === 15) {
      let b: number;
      do {
        b = src[pos++]!;
        length += b;
      } while (b === 255 && pos < end);
    }
    length += MIN_MATCH;
    if (out + length > dst.length) throw new Error('LZ4: match out of bounds');

    let from = out - offset;
    if (offset >= length && length >= 32) {
      dst.copyWithin(out, from, from + length);
      out += length;
    } else {
      // Overlapping matches repeat the last `offset` bytes; copy byte by byte.
      for (let i = 0; i < length; i++) dst[out++] = dst[from++]!;
    }
  }
  return out;
}

/** Decompresses one or more concatenated LZ4 frames. */
export function decompressLz4Frame(src: Uint8Array): Uint8Array {
  const dst = new Uint8Array(outputBound(src));
  let out = 0;
  let pos = 0;
  while (pos < src.length) {
    const magic = readU32(src, pos);
    if ((magic & SKIPPABLE_MASK) === SKIPPABLE_MAGIC) {
      pos += 8 + readU32(src, pos + 4);
      continue;
    }
    const header = readFrameHeader(src, pos);
    pos = header.dataStart;
    for (;;) {
      const word = readU32(src, pos);
      pos += 4;
      if (word === 0) break;
      const size = word & ~UNCOMPRESSED_BLOCK;
      if (pos + size > src.length) throw new Error('LZ4: truncated block');
      if (word & UNCOMPRESSED_BLOCK) {
        dst.set(src.subarray(pos, pos + size), out);
        out += size;
      } else {
        out = decompressBlock(src, pos, pos + size, dst, out);
      }
      pos += size + (header.blockChecksum ? 4 : 0);
    }
    if (header.contentChecksum) pos += 4;
  }
  return out === dst.length ? dst : dst.subarray(0, out);
}
