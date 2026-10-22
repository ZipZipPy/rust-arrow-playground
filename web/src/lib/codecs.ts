import { type Codec, CompressionType, compressionRegistry } from 'apache-arrow';
import { decompress as zstdDecompress } from 'fzstd';
import { decompressLz4Frame } from './lz4';

// apache-arrow reads the 8-byte uncompressed-length prefix of every compressed
// IPC buffer itself: it passes only the bytes after the prefix to `decode()`,
// and for a length of -1 (buffer stored uncompressed) it never calls the codec.
// So each decoder below receives exactly one complete LZ4 frame / zstd frame.

/**
 * Arrow views decompressed buffers as typed arrays in place and throws when,
 * say, a BigInt64Array would start at an unaligned offset. fzstd returns a view
 * at byte offset 12 of its working buffer, so realign such results.
 */
function aligned(bytes: Uint8Array): Uint8Array {
  return bytes.byteOffset % 8 === 0 ? bytes : bytes.slice();
}

export const lz4FrameCodec: Codec = { decode: (bytes) => aligned(decompressLz4Frame(bytes)) };
export const zstdCodec: Codec = { decode: (bytes) => aligned(zstdDecompress(bytes)) };

let registered = false;

/** Registers browser-side decoders for Arrow IPC body compression. Idempotent. */
export function registerIpcCodecs(): void {
  if (registered) return;
  compressionRegistry.set(CompressionType.LZ4_FRAME, lz4FrameCodec);
  compressionRegistry.set(CompressionType.ZSTD, zstdCodec);
  registered = true;
}
