//! HTTP compression of data responses, done by the handler itself.
//!
//! A streaming middleware compresses while hyper sends the body, after the
//! server timers have stopped, so its CPU time would show up in the client's
//! "download" time. Here the body is compressed in the same blocking task as
//! the encoder and timed as `compress`.

use std::io::Write;

use serde::{Serialize, Serializer};

use crate::encode::{CancelFlag, EncodeError};

/// Only bodies larger than 1 KiB are worth compressing.
pub const MIN_COMPRESSED_BYTES: usize = 1024 + 1;
/// Input fed to the compressor between two looks at the cancellation flag.
const CHUNK: usize = 1024 * 1024;
const ZSTD_LEVEL: i32 = 3;
const BROTLI_QUALITY: u32 = 4;
const BROTLI_WINDOW_BITS: u32 = 22;
const BROTLI_BUFFER: usize = 64 * 1024;
const GZIP_LEVEL: u32 = 6;

/// A `Content-Encoding` the server can produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpEncoding {
    Zstd,
    Brotli,
    Gzip,
}

impl HttpEncoding {
    /// In order of preference when the client weighs them equally.
    pub const ALL: &'static [Self] = &[Self::Zstd, Self::Brotli, Self::Gzip];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Zstd => "zstd",
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    fn matches(self, coding: &str) -> bool {
        coding.eq_ignore_ascii_case(self.as_str())
            || (self == Self::Gzip && coding.eq_ignore_ascii_case("x-gzip"))
    }

    /// Picks the encoding from `Accept-Encoding` values: the highest q-value
    /// wins, `*` covers codings not named, `q=0` refuses, and ties go to the
    /// earlier entry of [`Self::ALL`]. `None` when the client accepts none.
    pub fn negotiate<'a>(accept_encoding: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut named: Vec<(Self, f32)> = Vec::new();
        let mut wildcard = None;
        for item in accept_encoding
            .into_iter()
            .flat_map(|value| value.split(','))
        {
            let mut parts = item.split(';').map(str::trim);
            let coding = parts.next().unwrap_or_default();
            let Some(q) = quality(parts) else { continue };
            if coding == "*" {
                wildcard = Some(q);
            } else if let Some(encoding) = Self::ALL.iter().find(|e| e.matches(coding)) {
                named.push((*encoding, q));
            }
        }
        let weight = |encoding: Self| {
            named
                .iter()
                .find(|(named, _)| *named == encoding)
                .map(|&(_, q)| q)
                .or(wildcard)
                .unwrap_or(0.0)
        };
        let mut best: Option<(Self, f32)> = None;
        for &encoding in Self::ALL {
            let q = weight(encoding);
            if q > 0.0 && best.is_none_or(|(_, best_q)| q > best_q) {
                best = Some((encoding, q));
            }
        }
        best.map(|(encoding, _)| encoding)
    }

    /// Compresses `input` (zstd level 3, brotli quality 4, gzip level 6),
    /// stopping early if `cancel` is set.
    pub fn compress(self, input: &[u8], cancel: &CancelFlag) -> Result<Vec<u8>, EncodeError> {
        // Most payloads shrink at least twice, so this rarely regrows.
        let out = Vec::with_capacity(input.len() / 2);
        let compressed = match self {
            Self::Zstd => {
                let mut encoder = zstd::stream::write::Encoder::new(out, ZSTD_LEVEL)?;
                encoder.set_pledged_src_size(Some(input.len() as u64))?;
                feed(&mut encoder, input, cancel)?;
                encoder.finish()?
            }
            Self::Brotli => {
                let mut encoder = brotli::CompressorWriter::new(
                    out,
                    BROTLI_BUFFER,
                    BROTLI_QUALITY,
                    BROTLI_WINDOW_BITS,
                );
                feed(&mut encoder, input, cancel)?;
                encoder.flush()?;
                encoder.into_inner()
            }
            Self::Gzip => {
                let mut encoder =
                    flate2::write::GzEncoder::new(out, flate2::Compression::new(GZIP_LEVEL));
                feed(&mut encoder, input, cancel)?;
                encoder.finish()?
            }
        };
        Ok(compressed)
    }
}

impl Serialize for HttpEncoding {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The q-value among an `Accept-Encoding` item's parameters (1 if absent);
/// `None` for a malformed one, which drops the item.
fn quality<'a>(parameters: impl Iterator<Item = &'a str>) -> Option<f32> {
    let mut q = 1.0;
    for parameter in parameters {
        if let Some((name, value)) = parameter.split_once('=')
            && name.trim().eq_ignore_ascii_case("q")
        {
            q = value
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|q| (0.0..=1.0).contains(q))?;
        }
    }
    Some(q)
}

fn feed(encoder: &mut impl Write, input: &[u8], cancel: &CancelFlag) -> Result<(), EncodeError> {
    for chunk in input.chunks(CHUNK) {
        cancel.check()?;
        encoder.write_all(chunk)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn negotiate(header: &str) -> Option<&'static str> {
        HttpEncoding::negotiate([header]).map(HttpEncoding::as_str)
    }

    #[test]
    fn negotiation_respects_q_values_and_server_preference() {
        assert_eq!(negotiate("gzip"), Some("gzip"));
        assert_eq!(negotiate("br"), Some("br"));
        assert_eq!(negotiate("zstd"), Some("zstd"));
        // Chrome and Firefox send these; equal weights prefer zstd, then br.
        assert_eq!(negotiate("gzip, deflate, br, zstd"), Some("zstd"));
        assert_eq!(negotiate("gzip, deflate, br"), Some("br"));
        assert_eq!(negotiate("gzip;q=1, zstd;q=0.5"), Some("gzip"));
        assert_eq!(negotiate("br;q=0.9, gzip;q=0.9, zstd;q=0"), Some("br"));
        assert_eq!(negotiate("GZIP; Q=0.4, BR;q=0.3"), Some("gzip"));
        assert_eq!(negotiate("x-gzip"), Some("gzip"));
        assert_eq!(negotiate("*"), Some("zstd"));
        assert_eq!(negotiate("zstd;q=0, *;q=0.5"), Some("br"));
        assert_eq!(negotiate("identity"), None);
        assert_eq!(negotiate("deflate, compress"), None);
        assert_eq!(negotiate("*;q=0"), None);
        assert_eq!(negotiate(""), None);
        assert_eq!(
            negotiate("gzip;q=2"),
            None,
            "malformed q-values drop the item"
        );
        assert_eq!(
            HttpEncoding::negotiate(["gzip;q=0.5", "br;q=0.8"]).map(HttpEncoding::as_str),
            Some("br"),
            "several header lines"
        );
    }

    #[test]
    fn every_encoding_round_trips() {
        let input: Vec<u8> = (0..120_000u32)
            .flat_map(|i| format!("{{\"id\":{i},\"country\":\"TH\"}},").into_bytes())
            .collect();
        let cancel = CancelFlag::default();
        for &encoding in HttpEncoding::ALL {
            let compressed = encoding.compress(&input, &cancel).unwrap();
            assert!(compressed.len() < input.len() / 4, "{encoding:?}");
            let mut output = Vec::new();
            match encoding {
                HttpEncoding::Zstd => {
                    output = zstd::decode_all(&compressed[..]).unwrap();
                }
                HttpEncoding::Brotli => {
                    brotli::Decompressor::new(&compressed[..], 4096)
                        .read_to_end(&mut output)
                        .unwrap();
                }
                HttpEncoding::Gzip => {
                    flate2::read::GzDecoder::new(&compressed[..])
                        .read_to_end(&mut output)
                        .unwrap();
                }
            }
            assert_eq!(output, input, "{encoding:?}");
        }
    }

    #[test]
    fn compression_stops_when_cancelled() {
        let cancel = CancelFlag::default();
        cancel.cancel();
        let err = HttpEncoding::Gzip
            .compress(&[0; 4096], &cancel)
            .unwrap_err();
        assert!(matches!(err, EncodeError::Cancelled(_)));
    }
}
