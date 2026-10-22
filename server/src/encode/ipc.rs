//! Arrow IPC stream (served by the API) and file (written by `export`).

use std::io::Write;

use arrow::datatypes::Schema;
use arrow::error::ArrowError;
use arrow::ipc::CompressionType;
use arrow::ipc::writer::{FileWriter, IpcWriteOptions, StreamWriter};
use arrow::record_batch::RecordBatch;

use super::IpcCompression;

/// Writes schema, batches and end-of-stream marker in the IPC stream format.
pub fn write_stream<W: Write>(
    out: W,
    schema: &Schema,
    batches: &[RecordBatch],
    compression: IpcCompression,
) -> Result<W, ArrowError> {
    let mut writer = StreamWriter::try_new_with_options(out, schema, options(compression)?)?;
    for batch in batches {
        writer.write(batch)?;
    }
    // Writes the end-of-stream marker and flushes.
    writer.into_inner()
}

/// Writes the IPC file format (`.arrow`), readable by DuckDB, pyarrow, polars.
pub fn write_file<W: Write>(
    out: W,
    schema: &Schema,
    batches: &[RecordBatch],
    compression: IpcCompression,
) -> Result<W, ArrowError> {
    let mut writer = FileWriter::try_new_with_options(out, schema, options(compression)?)?;
    for batch in batches {
        writer.write(batch)?;
    }
    // Writes the footer and flushes.
    writer.into_inner()
}

fn options(compression: IpcCompression) -> Result<IpcWriteOptions, ArrowError> {
    let codec = match compression {
        IpcCompression::None => None,
        IpcCompression::Lz4 => Some(CompressionType::LZ4_FRAME),
        IpcCompression::Zstd => Some(CompressionType::ZSTD),
    };
    IpcWriteOptions::default().try_with_compression(codec)
}
