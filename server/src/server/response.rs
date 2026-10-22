//! Data response headers and large-body streaming.

use std::convert::Infallible;
use std::time::Duration;

use axum::body::Body;
use axum::http::header::{CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::IntoResponse;
use bytes::Bytes;
use futures_util::stream;

use super::compression::HttpEncoding;
use crate::encode::Format;

/// Bodies above this are streamed without `Content-Length`, which keeps
/// uncompressed responses under Cloud Run's 32 MiB HTTP/1 response limit.
pub const STREAM_THRESHOLD: usize = 16 * 1024 * 1024;
pub const STREAM_CHUNK: usize = 1024 * 1024;

pub const SERVER_TIMING: HeaderName = HeaderName::from_static("server-timing");
pub const TIMING_ALLOW_ORIGIN: HeaderName = HeaderName::from_static("timing-allow-origin");
pub const X_DB_MS: HeaderName = HeaderName::from_static("x-db-ms");
pub const X_ENCODE_MS: HeaderName = HeaderName::from_static("x-encode-ms");
pub const X_COMPRESS_MS: HeaderName = HeaderName::from_static("x-compress-ms");
pub const X_TOTAL_MS: HeaderName = HeaderName::from_static("x-total-ms");
pub const X_ROW_COUNT: HeaderName = HeaderName::from_static("x-row-count");
pub const X_COLUMN_COUNT: HeaderName = HeaderName::from_static("x-column-count");
pub const X_LAST_ID: HeaderName = HeaderName::from_static("x-last-id");
pub const X_BODY_BYTES: HeaderName = HeaderName::from_static("x-body-bytes");
pub const X_ENCODED_BYTES: HeaderName = HeaderName::from_static("x-encoded-bytes");
pub const X_FORMAT: HeaderName = HeaderName::from_static("x-format");
pub const X_DATASET: HeaderName = HeaderName::from_static("x-dataset");
pub const X_CACHE: HeaderName = HeaderName::from_static("x-cache");

/// Headers the browser may read cross-origin.
pub const EXPOSED_HEADERS: [HeaderName; 15] = [
    SERVER_TIMING,
    X_DB_MS,
    X_ENCODE_MS,
    X_COMPRESS_MS,
    X_TOTAL_MS,
    X_ROW_COUNT,
    X_COLUMN_COUNT,
    X_LAST_ID,
    X_BODY_BYTES,
    X_ENCODED_BYTES,
    X_FORMAT,
    X_DATASET,
    X_CACHE,
    CONTENT_ENCODING,
    VARY,
];

/// Small payloads go out in one piece with `Content-Length`; large ones as
/// 1 MiB chunks (chunked transfer encoding on HTTP/1.1).
pub fn payload_body(payload: Vec<u8>) -> Body {
    if payload.len() <= STREAM_THRESHOLD {
        return Body::from(payload);
    }
    let payload = Bytes::from(payload);
    let chunks = (0..payload.len()).step_by(STREAM_CHUNK).map(move |start| {
        let end = (start + STREAM_CHUNK).min(payload.len());
        Ok::<_, Infallible>(payload.slice(start..end))
    });
    Body::from_stream(stream::iter(chunks))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Hit,
    Miss,
    Off,
}

impl CacheStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Off => "off",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub db: Duration,
    pub encode: Duration,
    /// HTTP compression; zero when none was applied.
    pub compress: Duration,
    pub total: Duration,
}

/// Milliseconds with three decimals.
pub fn millis(duration: Duration) -> String {
    format!("{:.3}", duration.as_secs_f64() * 1000.0)
}

impl Timings {
    /// `Server-Timing`, `X-Db-Ms`, `X-Encode-Ms`, `X-Compress-Ms` and
    /// `X-Total-Ms`. `http_encoding` names the compression, if any.
    pub fn write_headers(
        &self,
        headers: &mut HeaderMap,
        encode_description: &str,
        http_encoding: Option<HttpEncoding>,
    ) {
        let (db, encode, compress, total) = (
            millis(self.db),
            millis(self.encode),
            millis(self.compress),
            millis(self.total),
        );
        let compression = http_encoding.map_or("identity", HttpEncoding::as_str);
        let server_timing = format!(
            "db;dur={db};desc=\"PostgreSQL\", encode;dur={encode};desc=\"{encode_description}\", \
             compress;dur={compress};desc=\"HTTP {compression}\", total;dur={total}"
        );
        headers.insert(SERVER_TIMING, header_value(server_timing));
        headers.insert(X_DB_MS, header_value(db));
        headers.insert(X_ENCODE_MS, header_value(encode));
        headers.insert(X_COMPRESS_MS, header_value(compress));
        headers.insert(X_TOTAL_MS, header_value(total));
    }
}

fn header_value(text: String) -> HeaderValue {
    HeaderValue::try_from(text).expect("generated header values are visible ASCII")
}

/// A successful `/api/data` response.
#[derive(Debug)]
pub struct DataResponse {
    /// What goes on the wire: the payload, compressed if `content_encoding`.
    pub body: Vec<u8>,
    /// Size of the payload before HTTP compression.
    pub payload_bytes: usize,
    pub content_encoding: Option<HttpEncoding>,
    pub format: Format,
    pub dataset: &'static str,
    pub rows: usize,
    pub columns: usize,
    pub last_key: i64,
    pub cache: CacheStatus,
    pub timings: Timings,
}

impl IntoResponse for DataResponse {
    fn into_response(self) -> axum::response::Response {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static(self.format.content_type()),
        );
        if let Some(encoding) = self.content_encoding {
            headers.insert(
                CONTENT_ENCODING,
                HeaderValue::from_static(encoding.as_str()),
            );
        }
        headers.insert(VARY, HeaderValue::from_static("Accept-Encoding"));
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        self.timings.write_headers(
            &mut headers,
            &format!("{} encode", self.format),
            self.content_encoding,
        );
        headers.insert(X_ROW_COUNT, self.rows.into());
        headers.insert(X_COLUMN_COUNT, self.columns.into());
        headers.insert(X_LAST_ID, self.last_key.into());
        headers.insert(X_BODY_BYTES, self.payload_bytes.into());
        headers.insert(X_ENCODED_BYTES, self.body.len().into());
        headers.insert(X_FORMAT, HeaderValue::from_static(self.format.as_str()));
        headers.insert(X_DATASET, HeaderValue::from_static(self.dataset));
        headers.insert(X_CACHE, HeaderValue::from_static(self.cache.as_str()));
        (headers, payload_body(self.body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::HttpBody;
    use http_body_util::BodyExt;

    use super::*;

    #[test]
    fn small_bodies_keep_an_exact_length() {
        let body = payload_body(vec![7; STREAM_THRESHOLD]);
        assert_eq!(body.size_hint().exact(), Some(STREAM_THRESHOLD as u64));
    }

    #[tokio::test]
    async fn large_bodies_stream_in_one_mebibyte_chunks() {
        let len = STREAM_THRESHOLD + STREAM_CHUNK / 2;
        let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let mut body = payload_body(payload.clone());
        assert_eq!(body.size_hint().exact(), None, "no Content-Length");

        let mut received = Vec::new();
        let mut chunk_sizes = Vec::new();
        while let Some(frame) = body.frame().await {
            let data = frame.unwrap().into_data().unwrap();
            chunk_sizes.push(data.len());
            received.extend_from_slice(&data);
        }
        assert_eq!(received, payload);
        assert_eq!(chunk_sizes.len(), 17);
        assert!(chunk_sizes[..16].iter().all(|&size| size == STREAM_CHUNK));
        assert_eq!(chunk_sizes[16], STREAM_CHUNK / 2);
    }

    #[test]
    fn timing_headers_use_three_decimals() {
        let mut headers = HeaderMap::new();
        let timings = Timings {
            db: Duration::from_micros(12_345),
            encode: Duration::from_nanos(1_600),
            compress: Duration::from_micros(7_250),
            total: Duration::from_secs(2),
        };
        timings.write_headers(&mut headers, "arrow encode", Some(HttpEncoding::Brotli));
        assert_eq!(
            headers[SERVER_TIMING],
            "db;dur=12.345;desc=\"PostgreSQL\", encode;dur=0.002;desc=\"arrow encode\", \
             compress;dur=7.250;desc=\"HTTP br\", total;dur=2000.000"
        );
        assert_eq!(headers[X_DB_MS], "12.345");
        assert_eq!(headers[X_ENCODE_MS], "0.002");
        assert_eq!(headers[X_COMPRESS_MS], "7.250");
        assert_eq!(headers[X_TOTAL_MS], "2000.000");

        let uncompressed = Timings {
            compress: Duration::ZERO,
            ..timings
        };
        uncompressed.write_headers(&mut headers, "json encode", None);
        assert!(
            headers[SERVER_TIMING]
                .to_str()
                .unwrap()
                .contains("compress;dur=0.000;desc=\"HTTP identity\"")
        );
        assert_eq!(headers[X_COMPRESS_MS], "0.000");
    }
}
