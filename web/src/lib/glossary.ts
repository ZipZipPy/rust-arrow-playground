// Plain-language explanations shown next to each metric.

export const GLOSSARY = {
  endToEnd:
    'Everything you wait for: from starting the request until the data is decoded and the Touch data step has read it in the browser. This is the number that matters to a user.',
  db: 'Time PostgreSQL took to run the query and hand the rows to the Rust server. It is the same work for every format, and it grows with the dataset: a join view or a live aggregate costs far more than a plain table.',
  encode:
    'Time the server spent turning rows into the response: JSON text, Arrow record batches, or a Parquet file (plus any IPC/Parquet compression).',
  compress:
    'Time the server spent compressing the response for HTTP (zstd, br or gzip) before sending it. Zero when HTTP compression is off, and when the server skips it: bodies under 1 KiB, Arrow IPC with zstd, and Parquet with zstd, gzip or brotli are already compressed.',
  network:
    'Time to first byte minus the server’s own work (query, encoding, compression): the round trip to the server and any queuing on the way. Grows with distance to the server.',
  download:
    'From the first to the last byte of the body: the transfer itself, roughly wire bytes ÷ bandwidth, so smaller responses win on slow links. When the body is HTTP-compressed, the browser also decompresses it here while it streams in.',
  decode:
    'Turning bytes into data in the browser. JSON must scan every character and build every value: one object per row, with its strings and numbers. Arrow IPC already has the in-memory layout, so decoding only wraps the bytes as typed arrays and builds no value yet. Parquet is decompressed and decoded into Arrow in WebAssembly first.',
  use: 'The Touch data step after decoding. “Every value” reads every value of every selected column once, as the JavaScript value an app works with (strings decoded, 64-bit integers and timestamps turned into numbers): Arrow and Parquet build those values here, while JSON built them during Decode. “One column” sums one numeric column, the column scan where Arrow never builds rows or strings.',
  browser:
    'Decode + Use: all the browser’s work to turn the body into values. Compare this rather than Decode alone, because Arrow moves the work of building values from Decode into Use.',
  other:
    'Whatever is left of the end-to-end time: opening the connection, browser queuing, server work outside the query, encoding and compression, handing the body to JavaScript, and scheduling between steps.',
  ttfb: 'Time to first byte: from sending the request to the first byte of the response. Includes the server’s query, encoding and HTTP compression, because the server finishes all three before it starts sending.',
  wire: 'Bytes that actually crossed the network, after HTTP compression (gzip, br or zstd). Less is better on slow or metered links.',
  payload:
    'Size of the response as the server produced it, before HTTP compression. This is what the browser has to decode.',
  rowsPerSecond: 'Rows divided by the end-to-end time.',
  megabytesPerSecond: 'Payload megabytes divided by the end-to-end time: how fast usable data arrives.',
  speedup: 'Median end-to-end time of JSON divided by this format’s. 3× means it finished in a third of the time.',
  sizeRatio: 'How the median wire size compares with JSON’s.',
  rtt: 'Round-trip time: the median of 5 requests to /api/ping, which returns an empty response. A floor for any request to this server.',
  timingSource:
    'Timings come from the browser’s Resource Timing API. When it is unavailable (for example a cross-origin server without Timing-Allow-Origin) the app falls back to measuring around fetch(), which is less precise.',
} as const;

export type GlossaryKey = keyof typeof GLOSSARY;
