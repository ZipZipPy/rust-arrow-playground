//! Arrow IPC stream (served by the API) and file (written by `export`).

use std::io::Write;

use arrow::datatypes::Schema;
use arrow::error::ArrowError;
use arrow::ipc::CompressionType;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions, StreamWriter};
use arrow::record_batch::RecordBatch;

use super::{CancelFlag, EncodeError, IpcCompression};

/// Flatbuffer metadata and padding allowance per record batch and column.
const METADATA_BYTES_PER_BATCH: usize = 512;
const METADATA_BYTES_PER_COLUMN: usize = 64;

/// Uncompressed IPC body size of `batches`: the bytes their (sliced) buffers
/// cover plus some metadata. Shared dictionaries are counted once per batch,
/// a small overestimate.
pub fn size_hint(batches: &[RecordBatch]) -> usize {
    batches
        .iter()
        .map(|batch| {
            let buffers: usize = batch
                .columns()
                .iter()
                .map(|array| array.to_data().get_slice_memory_size().unwrap_or(0))
                .sum();
            buffers + METADATA_BYTES_PER_BATCH + batch.num_columns() * METADATA_BYTES_PER_COLUMN
        })
        .sum()
}

/// Writes schema, batches and end-of-stream marker in the IPC stream format.
pub fn write_stream<W: Write>(
    out: W,
    schema: &Schema,
    batches: &[RecordBatch],
    compression: IpcCompression,
    cancel: &CancelFlag,
) -> Result<W, EncodeError> {
    let mut writer = StreamWriter::try_new_with_options(out, schema, options(compression)?)?;
    for batch in batches {
        cancel.check()?;
        writer.write(batch)?;
    }
    // Writes the end-of-stream marker and flushes.
    Ok(writer.into_inner()?)
}

/// Writes the IPC file format (`.arrow`), readable by DuckDB, pyarrow, polars.
pub fn write_file<W: Write>(
    out: W,
    schema: &Schema,
    batches: &[RecordBatch],
    compression: IpcCompression,
    cancel: &CancelFlag,
) -> Result<W, EncodeError> {
    let mut writer = FileWriter::try_new_with_options(out, schema, options(compression)?)?;
    for batch in batches {
        cancel.check()?;
        writer.write(batch)?;
    }
    // Writes the footer and flushes.
    Ok(writer.into_inner()?)
}

fn options(compression: IpcCompression) -> Result<IpcWriteOptions, ArrowError> {
    let codec = match compression {
        IpcCompression::None => None,
        IpcCompression::Lz4 => Some(CompressionType::LZ4_FRAME),
        IpcCompression::Zstd => Some(CompressionType::ZSTD),
    };
    IpcWriteOptions::default().try_with_compression(codec)
}
