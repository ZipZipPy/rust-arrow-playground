// Plain-language explanations shown next to each metric.

export const GLOSSARY = {
  endToEnd:
    'Everything you wait for: from starting the request until the rows are decoded and read once in the browser. This is the number that matters to a user.',
  db: 'Time PostgreSQL took to run the query and hand the rows to the Rust server. It is the same work for every format.',
  encode:
    'Time the server spent turning rows into the response: JSON text, Arrow record batches, or a Parquet file (plus any IPC/Parquet compression).',
  network:
    'Time to first byte minus the server’s own work: the round trip to the server and any queuing on the way. Grows with distance to the server. It can also include HTTP compression, which runs after the server’s timers stop.',
  download:
    'From the first to the last byte of the body. Roughly bytes ÷ bandwidth, so smaller responses win on slow links.',
  decode:
    'Turning bytes into usable data in the browser. JSON must scan every character and build one object per row. Arrow IPC is already laid out like memory, so it is mostly wrapped as typed arrays. Parquet is decompressed and decoded in WebAssembly first.',
  use: 'One pass over a column (the sum of “total”) to prove the data is usable. Arrow reads one contiguous typed array; JSON visits one object per row.',
  other:
    'Whatever is left of the end-to-end time: opening the connection, browser queuing, server work outside the query and encoding, the browser undoing HTTP compression and handing the body to JavaScript, and scheduling between steps.',
  ttfb: 'Time to first byte: from sending the request to the first byte of the response. Includes the server’s query and encoding, because the server encodes before it starts sending.',
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
