//! JSON array of row objects, serialized straight from Postgres rows.
//!
//! No `serde_json::Value` and no per-row maps: each cell is read from the
//! row's wire buffer and handed to the serializer, which is what a carefully
//! written REST endpoint would do. This keeps the JSON baseline honest.

use std::io::Write;

use serde::ser::{Error as _, SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};
use tokio_postgres::Row;

use crate::pg_time::{DATE_LEN, Date, RFC3339_LEN, Timestamp};
use crate::schema::{Column, ColumnType};

/// Serializes `rows` as `[{"col": value, ...}, ...]` into `out`.
pub fn write<W: Write>(
    mut out: W,
    rows: &[Row],
    columns: &[&'static Column],
) -> serde_json::Result<W> {
    serde_json::to_writer(&mut out, &JsonRows { rows, columns })?;
    Ok(out)
}

/// Approximate encoded size of one row, for pre-sizing buffers.
pub fn row_size_hint(columns: &[&Column]) -> usize {
    let cells: usize = columns
        .iter()
        // `"name":value,` around each value.
        .map(|column| column.name.len() + 4 + column.ty.json_size_hint())
        .sum();
    cells + 2
}

struct JsonRows<'a> {
    rows: &'a [Row],
    columns: &'a [&'static Column],
}

impl Serialize for JsonRows<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.rows.len()))?;
        for row in self.rows {
            seq.serialize_element(&JsonRow {
                row,
                columns: self.columns,
            })?;
        }
        seq.end()
    }
}

struct JsonRow<'a> {
    row: &'a Row,
    columns: &'a [&'static Column],
}

impl Serialize for JsonRow<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.columns.len()))?;
        for (idx, column) in self.columns.iter().enumerate() {
            let cell = Cell::read(self.row, idx, column.ty).map_err(S::Error::custom)?;
            map.serialize_entry(column.name, &cell)?;
        }
        map.end()
    }
}

/// One decoded value, borrowing text from the row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cell<'a> {
    Null,
    Int(i64),
    Float64(f64),
    Float32(f32),
    Bool(bool),
    Text(&'a str),
    Timestamp(Timestamp),
    Date(Date),
}

impl<'a> Cell<'a> {
    fn read(row: &'a Row, idx: usize, ty: ColumnType) -> Result<Self, tokio_postgres::Error> {
        fn get<'r, T: tokio_postgres::types::FromSql<'r>>(
            row: &'r Row,
            idx: usize,
            cell: impl FnOnce(T) -> Cell<'r>,
        ) -> Result<Cell<'r>, tokio_postgres::Error> {
            Ok(row.try_get::<_, Option<T>>(idx)?.map_or(Cell::Null, cell))
        }

        match ty {
            ColumnType::Int64 => get(row, idx, Cell::Int),
            ColumnType::Int32 => get(row, idx, |v: i32| Cell::Int(v.into())),
            ColumnType::Int16 => get(row, idx, |v: i16| Cell::Int(v.into())),
            ColumnType::Float64 => get(row, idx, Cell::Float64),
            ColumnType::Float32 => get(row, idx, Cell::Float32),
            ColumnType::Boolean => get(row, idx, Cell::Bool),
            ColumnType::Text => get(row, idx, Cell::Text),
            ColumnType::Timestamp => get(row, idx, Cell::Timestamp),
            ColumnType::Date => get(row, idx, Cell::Date),
        }
    }
}

impl Serialize for Cell<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match *self {
            Cell::Null => serializer.serialize_none(),
            Cell::Int(value) => serializer.serialize_i64(value),
            Cell::Float64(value) => serializer.serialize_f64(value),
            // Serialized as f32 so 0.05 prints as 0.05, not 0.05000000074505806.
            Cell::Float32(value) => serializer.serialize_f32(value),
            Cell::Bool(value) => serializer.serialize_bool(value),
            Cell::Text(value) => serializer.serialize_str(value),
            Cell::Timestamp(value) => {
                let mut buf = [0; RFC3339_LEN];
                serializer.serialize_str(value.write_rfc3339(&mut buf).map_err(S::Error::custom)?)
            }
            Cell::Date(value) => {
                let mut buf = [0; DATE_LEN];
                serializer.serialize_str(value.write_iso(&mut buf).map_err(S::Error::custom)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(cell: Cell<'_>) -> String {
        serde_json::to_string(&cell).unwrap()
    }

    #[test]
    fn nulls_are_null() {
        assert_eq!(json(Cell::Null), "null");
    }

    #[test]
    fn timestamps_are_rfc3339_utc_with_microseconds() {
        assert_eq!(
            json(Cell::Timestamp(Timestamp(1_735_732_800_123_456))),
            "\"2025-01-01T12:00:00.123456Z\""
        );
        assert_eq!(
            json(Cell::Timestamp(Timestamp(1_735_732_800_000_000))),
            "\"2025-01-01T12:00:00.000000Z\""
        );
    }

    #[test]
    fn dates_are_iso() {
        assert_eq!(json(Cell::Date(Date(20_091))), "\"2025-01-03\"");
    }

    #[test]
    fn floats_print_their_shortest_form() {
        assert_eq!(json(Cell::Float32(0.05)), "0.05");
        assert_eq!(json(Cell::Float32(0.1)), "0.1");
        assert_eq!(json(Cell::Float32(0.0)), "0.0");
        assert_eq!(json(Cell::Float64(19.99)), "19.99");
        assert_eq!(json(Cell::Float64(-0.12)), "-0.12");
    }

    #[test]
    fn integers_booleans_and_text() {
        assert_eq!(json(Cell::Int(-42)), "-42");
        assert_eq!(json(Cell::Bool(false)), "false");
        assert_eq!(
            json(Cell::Text("Zoë \"Z\" O'Brien")),
            r#""Zoë \"Z\" O'Brien""#
        );
    }

    #[test]
    fn out_of_range_timestamps_fail_instead_of_printing_garbage() {
        let err = serde_json::to_string(&Cell::Timestamp(Timestamp(i64::MAX))).unwrap_err();
        assert!(err.to_string().contains("outside the years"));
    }
}
