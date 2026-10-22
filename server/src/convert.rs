//! tokio-postgres rows → Arrow record batches.
//!
//! Rows are walked once, row by row, appending each cell to its column's
//! typed builder. Rows live scattered across the heap, so one sequential pass
//! is several times faster than one pass per column.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanBuilder, Date32Builder, DictionaryArray, Float32Builder, Float64Builder,
    Int16Builder, Int32Builder, Int64Builder, StringBuilder, StringDictionaryBuilder,
    TimestampMicrosecondBuilder,
};
use arrow::datatypes::{Int32Type, SchemaRef};
use arrow::error::ArrowError;
use arrow::record_batch::RecordBatch;
use tokio_postgres::Row;

use crate::pg_time::{Date, Timestamp};
use crate::schema::{self, Column, ColumnType};

/// Average bytes per text value used to pre-size string buffers.
const TEXT_BYTES_HINT: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("reading a Postgres value failed: {0}")]
    Postgres(#[from] tokio_postgres::Error),
    #[error("building Arrow arrays failed: {0}")]
    Arrow(#[from] ArrowError),
}

/// Converts `rows` (whose first `columns.len()` fields are `columns`) into
/// record batches of at most `batch_size` rows.
///
/// With `dictionary`, the catalog's dict columns become
/// `Dictionary(Int32, Utf8)`. Each such column is encoded once over all rows
/// and every batch gets a slice of its keys with the same values array, so
/// writers emit each dictionary exactly once instead of replacing it per
/// batch (which the IPC file format forbids and some readers mishandle).
pub fn record_batches(
    rows: &[Row],
    columns: &[&'static Column],
    batch_size: usize,
    dictionary: bool,
) -> Result<(SchemaRef, Vec<RecordBatch>), ConvertError> {
    assert!(batch_size > 0, "batch_size must be positive");
    let schema = schema::arrow_schema(columns, dictionary);
    let shared: Vec<bool> = columns
        .iter()
        .map(|column| dictionary && column.dictionary)
        .collect();
    let dictionaries = shared_dictionaries(rows, &shared)?;

    let mut batches = Vec::with_capacity(rows.len().div_ceil(batch_size));
    for (chunk_index, chunk) in rows.chunks(batch_size).enumerate() {
        let mut builders: Vec<Option<ColumnBuilder>> = columns
            .iter()
            .zip(&shared)
            .map(|(column, &is_shared)| {
                (!is_shared).then(|| ColumnBuilder::new(column.ty, chunk.len()))
            })
            .collect();
        for row in chunk {
            for (idx, builder) in builders.iter_mut().enumerate() {
                if let Some(builder) = builder {
                    builder.append(row, idx)?;
                }
            }
        }

        let offset = chunk_index * batch_size;
        let arrays: Vec<ArrayRef> = builders
            .into_iter()
            .zip(&dictionaries)
            .map(|(builder, dictionary)| match (builder, dictionary) {
                (Some(builder), _) => builder.finish(),
                (None, Some(dictionary)) => Arc::new(dictionary.slice(offset, chunk.len())),
                (None, None) => unreachable!("every column has a builder or a dictionary"),
            })
            .collect();
        batches.push(RecordBatch::try_new(schema.clone(), arrays)?);
    }
    Ok((schema, batches))
}

/// For each column flagged in `shared`, one dictionary array over all rows.
fn shared_dictionaries(
    rows: &[Row],
    shared: &[bool],
) -> Result<Vec<Option<DictionaryArray<Int32Type>>>, ConvertError> {
    let mut builders: Vec<Option<StringDictionaryBuilder<Int32Type>>> = shared
        .iter()
        .map(|&is_shared| {
            is_shared.then(|| {
                StringDictionaryBuilder::with_capacity(rows.len(), 256, 256 * TEXT_BYTES_HINT)
            })
        })
        .collect();
    if builders.iter().all(Option::is_none) {
        return Ok(builders.into_iter().map(|_| None).collect());
    }
    for row in rows {
        for (idx, builder) in builders.iter_mut().enumerate() {
            let Some(builder) = builder else { continue };
            match row.try_get::<_, Option<&str>>(idx)? {
                Some(value) => {
                    builder.append(value)?;
                }
                None => builder.append_null(),
            }
        }
    }
    Ok(builders
        .iter_mut()
        .map(|builder| builder.as_mut().map(StringDictionaryBuilder::finish))
        .collect())
}

/// A typed Arrow builder for one catalog column type.
enum ColumnBuilder {
    Int64(Int64Builder),
    Int32(Int32Builder),
    Int16(Int16Builder),
    Float64(Float64Builder),
    Float32(Float32Builder),
    Boolean(BooleanBuilder),
    Text(StringBuilder),
    Timestamp(TimestampMicrosecondBuilder),
    Date(Date32Builder),
}

impl ColumnBuilder {
    fn new(ty: ColumnType, capacity: usize) -> Self {
        match ty {
            ColumnType::Int64 => Self::Int64(Int64Builder::with_capacity(capacity)),
            ColumnType::Int32 => Self::Int32(Int32Builder::with_capacity(capacity)),
            ColumnType::Int16 => Self::Int16(Int16Builder::with_capacity(capacity)),
            ColumnType::Float64 => Self::Float64(Float64Builder::with_capacity(capacity)),
            ColumnType::Float32 => Self::Float32(Float32Builder::with_capacity(capacity)),
            ColumnType::Boolean => Self::Boolean(BooleanBuilder::with_capacity(capacity)),
            ColumnType::Text => Self::Text(StringBuilder::with_capacity(
                capacity,
                capacity * TEXT_BYTES_HINT,
            )),
            ColumnType::Timestamp => Self::Timestamp(
                TimestampMicrosecondBuilder::with_capacity(capacity).with_timezone("UTC"),
            ),
            ColumnType::Date => Self::Date(Date32Builder::with_capacity(capacity)),
        }
    }

    fn append(&mut self, row: &Row, idx: usize) -> Result<(), tokio_postgres::Error> {
        match self {
            Self::Int64(b) => b.append_option(row.try_get::<_, Option<i64>>(idx)?),
            Self::Int32(b) => b.append_option(row.try_get::<_, Option<i32>>(idx)?),
            Self::Int16(b) => b.append_option(row.try_get::<_, Option<i16>>(idx)?),
            Self::Float64(b) => b.append_option(row.try_get::<_, Option<f64>>(idx)?),
            Self::Float32(b) => b.append_option(row.try_get::<_, Option<f32>>(idx)?),
            Self::Boolean(b) => b.append_option(row.try_get::<_, Option<bool>>(idx)?),
            Self::Text(b) => b.append_option(row.try_get::<_, Option<&str>>(idx)?),
            Self::Timestamp(b) => {
                b.append_option(row.try_get::<_, Option<Timestamp>>(idx)?.map(|t| t.0))
            }
            Self::Date(b) => b.append_option(row.try_get::<_, Option<Date>>(idx)?.map(|d| d.0)),
        }
        Ok(())
    }

    fn finish(self) -> ArrayRef {
        match self {
            Self::Int64(mut b) => Arc::new(b.finish()),
            Self::Int32(mut b) => Arc::new(b.finish()),
            Self::Int16(mut b) => Arc::new(b.finish()),
            Self::Float64(mut b) => Arc::new(b.finish()),
            Self::Float32(mut b) => Arc::new(b.finish()),
            Self::Boolean(mut b) => Arc::new(b.finish()),
            Self::Text(mut b) => Arc::new(b.finish()),
            Self::Timestamp(mut b) => Arc::new(b.finish()),
            Self::Date(mut b) => Arc::new(b.finish()),
        }
    }
}
