// Initializes the same ESM build of parquet-wasm the browser uses, but from
// the .wasm bytes on disk instead of a fetched URL.
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import * as parquet from 'parquet-wasm/esm';

const require = createRequire(import.meta.url);
parquet.initSync({ module: readFileSync(require.resolve('parquet-wasm/esm/parquet_wasm_bg.wasm')) });

export { parquet };
