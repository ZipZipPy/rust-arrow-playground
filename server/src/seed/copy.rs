//! Binary `COPY ... FROM STDIN` of generated rows.

use std::pin::pin;

use tokio_postgres::Client;
use tokio_postgres::binary_copy::BinaryCopyInWriter;
use tokio_postgres::types::{ToSql, Type};

/// A generated row and the table it is loaded into.
pub trait CopyRow {
    const TABLE: &'static str;

    fn columns() -> Vec<&'static str>;

    /// Postgres types in [`Self::columns`] order.
    fn types() -> Vec<Type>;

    /// The row's values in [`Self::columns`] order.
    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send;
}

/// Loads `rows` into `R::TABLE` with one binary `COPY`, so rows are
/// generated while earlier ones are already on the wire. Returns the number
/// of rows copied.
pub async fn copy_in<R: CopyRow>(
    client: &Client,
    rows: impl IntoIterator<Item = R>,
) -> Result<u64, tokio_postgres::Error> {
    let sql = format!(
        "COPY {} ({}) FROM STDIN (FORMAT binary)",
        R::TABLE,
        R::columns().join(", ")
    );
    let sink = client.copy_in(&sql).await?;
    let types = R::types();
    let mut writer = pin!(BinaryCopyInWriter::new(sink, &types));
    for row in rows {
        writer.as_mut().write(row.values().as_ref()).await?;
    }
    writer.finish().await
}
