import wasmUrl from 'parquet-wasm/esm/parquet_wasm_bg.wasm?url';
import type { ParquetReader } from './decode';

let loading: Promise<ParquetReader> | null = null;

/**
 * Loads and instantiates parquet-wasm once. Call it before any timed decode so
 * downloading and compiling the ~5 MB module never counts as decode time.
 */
export function loadParquet(): Promise<ParquetReader> {
  loading ??= (async () => {
    const parquet = await import('parquet-wasm/esm');
    await parquet.default({ module_or_path: wasmUrl });
    return parquet;
  })().catch((error: unknown) => {
    loading = null; // let a later call retry
    throw error;
  });
  return loading;
}
