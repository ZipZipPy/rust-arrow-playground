//! Column catalog of the `orders` table.
//!
//! This is the single source of truth for column names, Postgres and Arrow
//! types, nullability and UI groups. The DDL, the SQL, the encoders, the
//! seeder and `/api/info` are all derived from it.

use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::Serialize;
use tokio_postgres::types::Type;

pub const TABLE: &str = "orders";

/// Storage type of a column, which fixes its Postgres, Arrow and JSON form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnType {
    Int64,
    Int32,
    Int16,
    Float64,
    Float32,
    Boolean,
    Text,
    /// `TIMESTAMPTZ`, microsecond precision, always UTC.
    Timestamp,
    Date,
}

impl ColumnType {
    /// Type name used in `CREATE TABLE`.
    pub const fn sql(self) -> &'static str {
        match self {
            Self::Int64 => "BIGINT",
            Self::Int32 => "INTEGER",
            Self::Int16 => "SMALLINT",
            Self::Float64 => "DOUBLE PRECISION",
            Self::Float32 => "REAL",
            Self::Boolean => "BOOLEAN",
            Self::Text => "TEXT",
            Self::Timestamp => "TIMESTAMPTZ",
            Self::Date => "DATE",
        }
    }

    /// `pg_type.typname`, as reported by `/api/info`.
    pub const fn pg_name(self) -> &'static str {
        match self {
            Self::Int64 => "int8",
            Self::Int32 => "int4",
            Self::Int16 => "int2",
            Self::Float64 => "float8",
            Self::Float32 => "float4",
            Self::Boolean => "bool",
            Self::Text => "text",
            Self::Timestamp => "timestamptz",
            Self::Date => "date",
        }
    }

    pub fn pg_type(self) -> Type {
        match self {
            Self::Int64 => Type::INT8,
            Self::Int32 => Type::INT4,
            Self::Int16 => Type::INT2,
            Self::Float64 => Type::FLOAT8,
            Self::Float32 => Type::FLOAT4,
            Self::Boolean => Type::BOOL,
            Self::Text => Type::TEXT,
            Self::Timestamp => Type::TIMESTAMPTZ,
            Self::Date => Type::DATE,
        }
    }

    pub fn arrow_type(self) -> DataType {
        match self {
            Self::Int64 => DataType::Int64,
            Self::Int32 => DataType::Int32,
            Self::Int16 => DataType::Int16,
            Self::Float64 => DataType::Float64,
            Self::Float32 => DataType::Float32,
            Self::Boolean => DataType::Boolean,
            Self::Text => DataType::Utf8,
            Self::Timestamp => DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            Self::Date => DataType::Date32,
        }
    }

    /// Arrow type as written in the design document and `/api/info`.
    pub const fn arrow_name(self) -> &'static str {
        match self {
            Self::Int64 => "Int64",
            Self::Int32 => "Int32",
            Self::Int16 => "Int16",
            Self::Float64 => "Float64",
            Self::Float32 => "Float32",
            Self::Boolean => "Boolean",
            Self::Text => "Utf8",
            Self::Timestamp => "Timestamp(Microsecond, \"UTC\")",
            Self::Date => "Date32",
        }
    }

    /// Rough size of one JSON value, used to pre-size output buffers.
    pub const fn json_size_hint(self) -> usize {
        match self {
            Self::Int64 => 8,
            Self::Int32 => 6,
            Self::Int16 => 2,
            Self::Float64 => 10,
            Self::Float32 => 4,
            Self::Boolean => 5,
            Self::Text => 16,
            Self::Timestamp => 29,
            Self::Date => 12,
        }
    }
}

/// Column group used by the UI's column presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Numeric,
    Text,
    Time,
}

#[derive(Debug, PartialEq, Eq, Hash)]
pub struct Column {
    pub name: &'static str,
    pub ty: ColumnType,
    pub nullable: bool,
    /// Low-cardinality text column that may be dictionary encoded in Arrow.
    pub dictionary: bool,
    pub group: Group,
}

impl Column {
    /// The name as a quoted SQL identifier. Catalog names are plain
    /// lowercase identifiers, so no escaping is needed.
    pub fn quoted(&self) -> String {
        format!("\"{}\"", self.name)
    }

    pub fn arrow_field(&self, dictionary: bool) -> Field {
        let data_type = if dictionary && self.dictionary {
            DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        } else {
            self.ty.arrow_type()
        };
        Field::new(self.name, data_type, self.nullable)
    }
}

const fn column(
    name: &'static str,
    ty: ColumnType,
    nullable: bool,
    dictionary: bool,
    group: Group,
) -> Column {
    Column {
        name,
        ty,
        nullable,
        dictionary,
        group,
    }
}

use ColumnType as T;
use Group as G;

/// Every column of `orders`, in table order.
pub static COLUMNS: [Column; 20] = [
    column("id", T::Int64, false, false, G::Numeric),
    column("created_at", T::Timestamp, false, false, G::Time),
    column("ship_date", T::Date, true, false, G::Time),
    column("customer_id", T::Int32, false, false, G::Numeric),
    column("customer_name", T::Text, false, false, G::Text),
    column("email", T::Text, false, false, G::Text),
    column("country", T::Text, false, true, G::Text),
    column("city", T::Text, false, true, G::Text),
    column("category", T::Text, false, true, G::Text),
    column("product", T::Text, false, true, G::Text),
    column("quantity", T::Int16, false, false, G::Numeric),
    column("unit_price", T::Float64, false, false, G::Numeric),
    column("discount", T::Float32, false, false, G::Numeric),
    column("total", T::Float64, false, false, G::Numeric),
    column("payment_method", T::Text, false, true, G::Text),
    column("is_returned", T::Boolean, false, false, G::Numeric),
    column("rating", T::Int16, true, false, G::Numeric),
    column("latitude", T::Float64, false, false, G::Numeric),
    column("longitude", T::Float64, false, false, G::Numeric),
    column("notes", T::Text, true, false, G::Text),
];

pub fn find(name: &str) -> Option<&'static Column> {
    COLUMNS.iter().find(|column| column.name == name)
}

pub fn arrow_schema(columns: &[&Column], dictionary: bool) -> SchemaRef {
    let fields: Vec<Field> = columns
        .iter()
        .map(|column| column.arrow_field(dictionary))
        .collect();
    Arc::new(Schema::new(fields))
}

/// A non-empty, duplicate-free subset of the catalog in request order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnSelection(Vec<&'static Column>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown column \"{name}\" (valid columns: {valid})")]
pub struct UnknownColumn {
    pub name: String,
    valid: String,
}

impl ColumnSelection {
    pub fn all() -> Self {
        Self(COLUMNS.iter().collect())
    }

    /// Parses a comma-separated list of column names. Whitespace around names
    /// and empty items are ignored, duplicates keep their first position, and
    /// a list without any names selects every column.
    pub fn parse(list: &str) -> Result<Self, UnknownColumn> {
        let mut selected: Vec<&'static Column> = Vec::new();
        for name in list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let column = find(name).ok_or_else(|| UnknownColumn {
                name: name.to_owned(),
                valid: COLUMNS
                    .iter()
                    .map(|c| c.name)
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;
            if !selected.contains(&column) {
                selected.push(column);
            }
        }
        Ok(if selected.is_empty() {
            Self::all()
        } else {
            Self(selected)
        })
    }

    pub fn columns(&self) -> &[&'static Column] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.0.iter().position(|column| column.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(selection: &ColumnSelection) -> Vec<&'static str> {
        selection.columns().iter().map(|c| c.name).collect()
    }

    #[test]
    fn catalog_names_are_unique_plain_identifiers() {
        for (i, column) in COLUMNS.iter().enumerate() {
            assert!(
                column
                    .name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b == b'_'),
                "{} must not need escaping",
                column.name
            );
            assert_eq!(find(column.name), Some(&COLUMNS[i]));
        }
    }

    #[test]
    fn dictionary_columns_are_text() {
        for column in COLUMNS.iter().filter(|c| c.dictionary) {
            assert_eq!(column.ty, ColumnType::Text, "{}", column.name);
        }
    }

    #[test]
    fn empty_list_selects_all_columns() {
        assert_eq!(ColumnSelection::parse("").unwrap(), ColumnSelection::all());
        assert_eq!(ColumnSelection::parse(" , ,").unwrap().len(), COLUMNS.len());
    }

    #[test]
    fn selection_keeps_order_and_drops_duplicates() {
        let selection = ColumnSelection::parse("total, id,total,country ,id").unwrap();
        assert_eq!(names(&selection), ["total", "id", "country"]);
        assert_eq!(selection.position("id"), Some(1));
        assert_eq!(selection.position("email"), None);
    }

    #[test]
    fn unknown_columns_are_rejected() {
        let err = ColumnSelection::parse("id,password").unwrap_err();
        assert_eq!(err.name, "password");
        assert!(err.to_string().contains("valid columns: id, created_at"));
        // Names are case sensitive, like the JSON keys.
        assert!(ColumnSelection::parse("ID").is_err());
    }

    #[test]
    fn dictionary_flag_only_changes_dict_columns() {
        let selection = ColumnSelection::parse("country,customer_name").unwrap();
        let schema = arrow_schema(selection.columns(), true);
        assert_eq!(
            schema.field(0).data_type(),
            &DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
        );
        assert_eq!(schema.field(1).data_type(), &DataType::Utf8);
        let plain = arrow_schema(selection.columns(), false);
        assert_eq!(plain.field(0).data_type(), &DataType::Utf8);
    }
}
