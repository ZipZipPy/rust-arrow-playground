//! Dataset catalog: every relation the API serves, with its columns.
//!
//! This is the single source of truth for relation names, key columns,
//! column names, Postgres and Arrow types, nullability and UI groups. The
//! DDL, the SQL, the encoders, the seeder and `/api/info` are all derived
//! from it.

use std::hash::{Hash, Hasher};
use std::str::FromStr;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use serde::Serialize;
use tokio_postgres::types::Type;

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
pub static ORDERS_COLUMNS: [Column; 20] = [
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

/// Every column of the view `order_line_details`, in view order.
pub static ORDER_LINE_COLUMNS: [Column; 29] = [
    column("line_id", T::Int64, false, false, G::Numeric),
    column("order_id", T::Int64, false, false, G::Numeric),
    column("line_no", T::Int16, false, false, G::Numeric),
    column("created_at", T::Timestamp, false, false, G::Time),
    column("ship_date", T::Date, true, false, G::Time),
    column("payment_method", T::Text, false, true, G::Text),
    column("is_returned", T::Boolean, false, false, G::Numeric),
    column("customer_id", T::Int32, false, false, G::Numeric),
    column("customer_name", T::Text, false, false, G::Text),
    column("customer_segment", T::Text, false, true, G::Text),
    column("customer_since", T::Date, false, false, G::Time),
    column("customer_city", T::Text, false, true, G::Text),
    column("country_code", T::Text, false, true, G::Text),
    column("country_name", T::Text, false, true, G::Text),
    column("region", T::Text, false, true, G::Text),
    column("currency", T::Text, false, true, G::Text),
    column("product_id", T::Int32, false, false, G::Numeric),
    column("product_name", T::Text, false, true, G::Text),
    column("brand", T::Text, false, true, G::Text),
    column("category", T::Text, false, true, G::Text),
    column("department", T::Text, false, true, G::Text),
    column("quantity", T::Int16, false, false, G::Numeric),
    column("unit_price", T::Float64, false, false, G::Numeric),
    column("discount", T::Float32, false, false, G::Numeric),
    column("line_total", T::Float64, false, false, G::Numeric),
    column("vat_rate", T::Float32, false, false, G::Numeric),
    column("shipping_weight_kg", T::Float32, false, false, G::Numeric),
    column("latitude", T::Float64, false, false, G::Numeric),
    column("longitude", T::Float64, false, false, G::Numeric),
];

/// Every column of `daily_sales` and `daily_sales_live`, which share their
/// defining query.
pub static DAILY_SALES_COLUMNS: [Column; 12] = [
    column("id", T::Int64, false, false, G::Numeric),
    column("day", T::Date, false, false, G::Time),
    column("country_code", T::Text, false, true, G::Text),
    column("country_name", T::Text, false, true, G::Text),
    column("region", T::Text, false, true, G::Text),
    column("category", T::Text, false, true, G::Text),
    column("orders", T::Int64, false, false, G::Numeric),
    column("lines", T::Int64, false, false, G::Numeric),
    column("units", T::Int64, false, false, G::Numeric),
    column("revenue", T::Float64, false, false, G::Numeric),
    column("avg_discount", T::Float32, false, false, G::Numeric),
    column("returned_lines", T::Int64, false, false, G::Numeric),
];

/// What kind of relation a dataset reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    Table,
    View,
    MaterializedView,
}

/// Where `/api/info` takes a dataset's row estimate, largest key and size
/// from, without running the dataset's query.
#[derive(Debug)]
pub enum Stats {
    /// The relation itself: a table or a materialized view.
    Relation,
    /// A join view with one row per row of table `rows` (whose `key` is the
    /// view's key); its size is the sum over every table it reads.
    Join {
        rows: &'static str,
        key: &'static str,
        tables: &'static [&'static str],
    },
    /// A live view with the same rows as another dataset (its materialized
    /// twin); it has no storage of its own.
    Mirror(&'static Dataset),
}

/// One relation served by the API, with its column catalog.
#[derive(Debug)]
pub struct Dataset {
    /// The `dataset` parameter.
    pub name: &'static str,
    pub label: &'static str,
    pub kind: RelationKind,
    pub relation: &'static str,
    /// BIGINT column used for keyset pagination (`offset`).
    pub key: &'static str,
    /// One sentence for the UI.
    pub description: &'static str,
    pub columns: &'static [Column],
    pub stats: Stats,
}

/// Datasets are equal when their names are, which is what the row cache key
/// needs.
impl PartialEq for Dataset {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Eq for Dataset {}

impl Hash for Dataset {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

impl Dataset {
    pub fn column(&self, name: &str) -> Option<&'static Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn key_column(&self) -> &'static Column {
        self.column(self.key)
            .expect("the key is a column of its dataset")
    }

    /// Number of columns that may be dictionary encoded.
    pub fn dictionary_columns(&self) -> usize {
        self.columns
            .iter()
            .filter(|column| column.dictionary)
            .count()
    }
}

pub static ORDERS: Dataset = Dataset {
    name: "orders",
    label: "Orders (one wide table)",
    kind: RelationKind::Table,
    relation: "orders",
    key: "id",
    description: "One row per order from a single 20-column table: a primary-key range scan, \
                  so the database part is as cheap as it gets.",
    columns: &ORDERS_COLUMNS,
    stats: Stats::Relation,
};

pub static ORDER_LINES: Dataset = Dataset {
    name: "order_lines",
    label: "Order lines (7-table join view)",
    kind: RelationKind::View,
    relation: "order_line_details",
    key: "line_id",
    description: "One row per order line: every request joins order_items with orders, \
                  customers, cities, countries, products and categories.",
    columns: &ORDER_LINE_COLUMNS,
    stats: Stats::Join {
        rows: "order_items",
        key: "id",
        tables: &[
            "order_items",
            "orders",
            "customers",
            "cities",
            "countries",
            "products",
            "categories",
        ],
    },
};

pub static DAILY_SALES: Dataset = Dataset {
    name: "daily_sales",
    label: "Daily sales (materialized view)",
    kind: RelationKind::MaterializedView,
    relation: "daily_sales",
    key: "id",
    description: "Revenue per day, country and category, computed once by the seeder and \
                  stored: reads a small precomputed table.",
    columns: &DAILY_SALES_COLUMNS,
    stats: Stats::Relation,
};

pub static DAILY_SALES_LIVE: Dataset = Dataset {
    name: "daily_sales_live",
    label: "Daily sales, live (aggregate view)",
    kind: RelationKind::View,
    relation: "daily_sales_live",
    key: "id",
    description: "The same daily aggregate computed on every request: the database joins \
                  and groups every order line before the first row comes back.",
    columns: &DAILY_SALES_COLUMNS,
    stats: Stats::Mirror(&DAILY_SALES),
};

/// Every dataset, in the order the UI lists them.
pub static DATASETS: [&Dataset; 4] = [&ORDERS, &ORDER_LINES, &DAILY_SALES, &DAILY_SALES_LIVE];

pub const DEFAULT_DATASET: &Dataset = &ORDERS;

/// Looks a dataset up by name, ignoring ASCII case like other keywords.
pub fn dataset(name: &str) -> Option<&'static Dataset> {
    DATASETS
        .iter()
        .copied()
        .find(|dataset| dataset.name.eq_ignore_ascii_case(name))
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown dataset \"{name}\" (expected one of: {expected})")]
pub struct UnknownDataset {
    pub name: String,
    expected: String,
}

impl FromStr for &'static Dataset {
    type Err = UnknownDataset;

    fn from_str(name: &str) -> Result<Self, UnknownDataset> {
        dataset(name).ok_or_else(|| UnknownDataset {
            name: name.to_owned(),
            expected: DATASETS
                .iter()
                .map(|d| d.name)
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

pub fn arrow_schema(columns: &[&Column], dictionary: bool) -> SchemaRef {
    let fields: Vec<Field> = columns
        .iter()
        .map(|column| column.arrow_field(dictionary))
        .collect();
    Arc::new(Schema::new(fields))
}

/// A non-empty, duplicate-free subset of one dataset's columns in request
/// order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnSelection(Vec<&'static Column>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown column \"{name}\" in dataset {dataset} (valid columns: {valid})")]
pub struct UnknownColumn {
    pub name: String,
    dataset: &'static str,
    valid: String,
}

impl ColumnSelection {
    pub fn all(dataset: &Dataset) -> Self {
        Self(dataset.columns.iter().collect())
    }

    /// Parses a comma-separated list of `dataset`'s column names. Whitespace
    /// around names and empty items are ignored, duplicates keep their first
    /// position, and a list without any names selects every column.
    pub fn parse(dataset: &Dataset, list: &str) -> Result<Self, UnknownColumn> {
        let mut selected: Vec<&'static Column> = Vec::new();
        for name in list
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            let column = dataset.column(name).ok_or_else(|| UnknownColumn {
                name: name.to_owned(),
                dataset: dataset.name,
                valid: dataset
                    .columns
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
            Self::all(dataset)
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
        for dataset in DATASETS {
            for (i, column) in dataset.columns.iter().enumerate() {
                assert!(
                    column
                        .name
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b == b'_'),
                    "{} must not need escaping",
                    column.name
                );
                assert_eq!(
                    dataset.column(column.name),
                    Some(&dataset.columns[i]),
                    "{}.{} is unique",
                    dataset.name,
                    column.name
                );
            }
        }
    }

    #[test]
    fn keys_are_non_null_bigints() {
        for dataset in DATASETS {
            let key = dataset.key_column();
            assert_eq!(key.ty, ColumnType::Int64, "{}", dataset.name);
            assert!(!key.nullable, "{}", dataset.name);
        }
    }

    #[test]
    fn datasets_match_the_design() {
        let names: Vec<&str> = DATASETS.iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            ["orders", "order_lines", "daily_sales", "daily_sales_live"]
        );
        assert_eq!(ORDERS.columns.len(), 20);
        assert_eq!(ORDER_LINES.columns.len(), 29);
        assert_eq!(DAILY_SALES.columns.len(), 12);
        assert_eq!(dataset("Order_Lines"), Some(&ORDER_LINES));
        assert_eq!(dataset("orders; --"), None);
        let err = "nope".parse::<&Dataset>().unwrap_err();
        assert_eq!(
            err.to_string(),
            "unknown dataset \"nope\" (expected one of: orders, order_lines, daily_sales, daily_sales_live)"
        );
    }

    #[test]
    fn dictionary_columns_are_text() {
        for dataset in DATASETS {
            for column in dataset.columns.iter().filter(|c| c.dictionary) {
                assert_eq!(column.ty, ColumnType::Text, "{}", column.name);
            }
        }
        assert_eq!(ORDERS.dictionary_columns(), 5);
    }

    #[test]
    fn empty_list_selects_all_columns() {
        assert_eq!(
            ColumnSelection::parse(&ORDERS, "").unwrap(),
            ColumnSelection::all(&ORDERS)
        );
        assert_eq!(
            ColumnSelection::parse(&ORDER_LINES, " , ,").unwrap().len(),
            ORDER_LINE_COLUMNS.len()
        );
    }

    #[test]
    fn selection_keeps_order_and_drops_duplicates() {
        let selection = ColumnSelection::parse(&ORDERS, "total, id,total,country ,id").unwrap();
        assert_eq!(names(&selection), ["total", "id", "country"]);
        assert_eq!(selection.position("id"), Some(1));
        assert_eq!(selection.position("email"), None);
    }

    #[test]
    fn unknown_columns_are_rejected_per_dataset() {
        let err = ColumnSelection::parse(&ORDERS, "id,password").unwrap_err();
        assert_eq!(err.name, "password");
        assert!(
            err.to_string()
                .contains("in dataset orders (valid columns: id, created_at")
        );
        // Names are case sensitive, like the JSON keys.
        assert!(ColumnSelection::parse(&ORDERS, "ID").is_err());
        // Columns of one dataset are not columns of another.
        assert!(ColumnSelection::parse(&ORDERS, "line_id").is_err());
        assert!(ColumnSelection::parse(&DAILY_SALES, "email").is_err());
        assert!(ColumnSelection::parse(&ORDER_LINES, "line_id,brand").is_ok());
    }

    #[test]
    fn dictionary_flag_only_changes_dict_columns() {
        let selection = ColumnSelection::parse(&ORDERS, "country,customer_name").unwrap();
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
