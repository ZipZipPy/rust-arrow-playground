//! DDL of every relation: the normalized tables the seeder fills, `orders`
//! (generated from the catalog), the join view, and the daily aggregate as a
//! materialized view and a live view built from one SQL string.

use crate::schema::{Dataset, ORDERS_COLUMNS, RelationKind};

/// Bumped whenever the seeded schema changes; `seed --if-empty` replaces
/// data seeded under any other version. Version 1 had `orders` only.
pub const SCHEMA_VERSION: i32 = 2;

/// The 7-table join behind `order_line_details` and the daily aggregate.
macro_rules! order_lines_join {
    () => {
        "FROM order_items oi
JOIN orders o      ON o.id = oi.order_id
JOIN customers c   ON c.id = o.customer_id
JOIN cities ci     ON ci.id = c.city_id
JOIN countries co  ON co.code = ci.country_code
JOIN products p    ON p.id = oi.product_id
JOIN categories ca ON ca.id = p.category_id"
    };
}

/// One row per order line. Every column has an explicit type, so the
/// catalog can be checked against what Postgres reports.
pub const ORDER_LINE_DETAILS_SQL: &str = concat!(
    "SELECT oi.id AS line_id,
       oi.order_id,
       oi.line_no,
       o.created_at,
       o.ship_date,
       o.payment_method,
       o.is_returned,
       c.id AS customer_id,
       c.name AS customer_name,
       c.segment AS customer_segment,
       c.signup_date AS customer_since,
       ci.name AS customer_city,
       co.code AS country_code,
       co.name AS country_name,
       co.region,
       co.currency,
       p.id AS product_id,
       p.name AS product_name,
       p.brand,
       ca.name AS category,
       ca.department,
       oi.quantity,
       oi.unit_price,
       oi.discount,
       round((oi.quantity * oi.unit_price * (1 - oi.discount))::numeric, 2)::float8 AS line_total,
       co.vat_rate,
       (p.weight_kg * oi.quantity)::real AS shipping_weight_kg,
       ci.latitude,
       ci.longitude
",
    order_lines_join!()
);

/// Revenue per day, customer country and category. Both `daily_sales`
/// (materialized) and `daily_sales_live` are this exact string, so they
/// cannot drift apart.
pub const DAILY_SALES_SQL: &str = concat!(
    "SELECT row_number() OVER (ORDER BY day, country_code, category)::bigint AS id, *
FROM (
  SELECT (o.created_at AT TIME ZONE 'UTC')::date AS day,
         co.code AS country_code, co.name AS country_name, co.region,
         ca.name AS category,
         count(*) FILTER (WHERE oi.line_no = 1)::bigint AS orders,
         count(*)::bigint AS lines,
         sum(oi.quantity)::bigint AS units,
         round(sum(oi.quantity * oi.unit_price * (1 - oi.discount))::numeric, 2)::float8 AS revenue,
         avg(oi.discount)::real AS avg_discount,
         count(*) FILTER (WHERE o.is_returned)::bigint AS returned_lines
  ",
    order_lines_join!(),
    "
  GROUP BY 1, 2, 3, 4, 5
) s"
);

/// Column definitions of a table.
#[derive(Debug)]
pub enum Columns {
    Sql(&'static str),
    /// Generated from a dataset catalog.
    Catalog(&'static [crate::schema::Column]),
}

/// An index the seeder builds after loading; a unique one becomes the
/// table's primary key.
#[derive(Debug)]
pub struct Index {
    pub name: &'static str,
    pub table: &'static str,
    pub columns: &'static str,
    pub primary_key: bool,
}

impl Index {
    pub fn create_sql(&self) -> String {
        let unique = if self.primary_key { "UNIQUE " } else { "" };
        format!(
            "CREATE {unique}INDEX {} ON {} ({})",
            self.name, self.table, self.columns
        )
    }

    /// Turns the unique index into the primary key, a catalog-only change.
    pub fn primary_key_sql(&self) -> Option<String> {
        self.primary_key.then(|| {
            format!(
                "ALTER TABLE {} ADD CONSTRAINT {} PRIMARY KEY USING INDEX {}",
                self.table, self.name, self.name
            )
        })
    }
}

#[derive(Debug)]
pub struct Table {
    pub name: &'static str,
    pub columns: Columns,
    /// Built after loading; small dimension tables declare their keys
    /// inline instead.
    pub indexes: &'static [Index],
    /// `ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY`, added after the
    /// indexes.
    pub foreign_keys: &'static [&'static str],
}

impl Table {
    /// `CREATE TABLE` with the inline constraints only.
    pub fn create_sql(&self) -> String {
        let columns = match self.columns {
            Columns::Sql(sql) => sql.to_owned(),
            Columns::Catalog(columns) => columns
                .iter()
                .map(|column| {
                    let not_null = if column.nullable { "" } else { " NOT NULL" };
                    format!("{} {}{not_null}", column.quoted(), column.ty.sql())
                })
                .collect::<Vec<_>>()
                .join(",\n  "),
        };
        format!("CREATE TABLE {} (\n  {columns}\n)", self.name)
    }

    /// Everything [`Self::create_sql`] leaves out: indexes, primary key and
    /// foreign keys, in an order that works on an empty table.
    pub fn constraints_sql(&self) -> Vec<String> {
        let indexes = self.indexes.iter().map(Index::create_sql);
        let primary_keys = self.indexes.iter().filter_map(Index::primary_key_sql);
        let foreign_keys = self.foreign_keys.iter().map(|&fk| fk.to_owned());
        indexes.chain(primary_keys).chain(foreign_keys).collect()
    }
}

/// Every table, parents before children.
pub static TABLES: [Table; 7] = [
    Table {
        name: "countries",
        columns: Columns::Sql(
            "code TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  region TEXT NOT NULL,
  currency TEXT NOT NULL,
  vat_rate REAL NOT NULL",
        ),
        indexes: &[],
        foreign_keys: &[],
    },
    Table {
        name: "cities",
        columns: Columns::Sql(
            "id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  country_code TEXT NOT NULL REFERENCES countries,
  latitude DOUBLE PRECISION NOT NULL,
  longitude DOUBLE PRECISION NOT NULL,
  population INTEGER NOT NULL",
        ),
        indexes: &[],
        foreign_keys: &[],
    },
    Table {
        name: "categories",
        columns: Columns::Sql(
            "id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  department TEXT NOT NULL",
        ),
        indexes: &[],
        foreign_keys: &[],
    },
    Table {
        name: "products",
        columns: Columns::Sql(
            "id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  category_id INTEGER NOT NULL REFERENCES categories,
  brand TEXT NOT NULL,
  base_price DOUBLE PRECISION NOT NULL,
  weight_kg REAL NOT NULL,
  launched_on DATE NOT NULL",
        ),
        indexes: &[],
        foreign_keys: &[],
    },
    Table {
        name: "customers",
        columns: Columns::Sql(
            "id INTEGER NOT NULL,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  segment TEXT NOT NULL,
  signup_date DATE NOT NULL,
  city_id INTEGER NOT NULL,
  birth_year SMALLINT,
  is_active BOOLEAN NOT NULL",
        ),
        indexes: &[Index {
            name: "customers_pkey",
            table: "customers",
            columns: "id",
            primary_key: true,
        }],
        foreign_keys: &[
            "ALTER TABLE customers ADD CONSTRAINT customers_city_id_fkey \
             FOREIGN KEY (city_id) REFERENCES cities (id)",
        ],
    },
    Table {
        name: "orders",
        columns: Columns::Catalog(&ORDERS_COLUMNS),
        indexes: &[
            Index {
                name: "orders_pkey",
                table: "orders",
                columns: "id",
                primary_key: true,
            },
            Index {
                name: "orders_created_at_idx",
                table: "orders",
                columns: "created_at",
                primary_key: false,
            },
        ],
        foreign_keys: &["ALTER TABLE orders ADD CONSTRAINT orders_customer_id_fkey \
             FOREIGN KEY (customer_id) REFERENCES customers (id)"],
    },
    Table {
        name: "order_items",
        columns: Columns::Sql(
            "id BIGINT NOT NULL,
  order_id BIGINT NOT NULL,
  line_no SMALLINT NOT NULL,
  product_id INTEGER NOT NULL,
  quantity SMALLINT NOT NULL,
  unit_price DOUBLE PRECISION NOT NULL,
  discount REAL NOT NULL",
        ),
        indexes: &[
            Index {
                name: "order_items_pkey",
                table: "order_items",
                columns: "id",
                primary_key: true,
            },
            Index {
                name: "order_items_order_id_idx",
                table: "order_items",
                columns: "order_id",
                primary_key: false,
            },
        ],
        foreign_keys: &[
            "ALTER TABLE order_items ADD CONSTRAINT order_items_order_id_fkey \
             FOREIGN KEY (order_id) REFERENCES orders (id)",
            "ALTER TABLE order_items ADD CONSTRAINT order_items_product_id_fkey \
             FOREIGN KEY (product_id) REFERENCES products (id)",
        ],
    },
];

pub fn table(name: &str) -> &'static Table {
    TABLES
        .iter()
        .find(|table| table.name == name)
        .expect("a known table")
}

/// A view or materialized view over the tables.
#[derive(Debug)]
pub struct View {
    pub name: &'static str,
    pub kind: RelationKind,
    pub select: &'static str,
    /// Indexes of a materialized view, built after it is filled.
    pub indexes: &'static [&'static str],
}

impl View {
    /// `CREATE [MATERIALIZED] VIEW`, filled at once and then indexed.
    pub fn create_sql(&self) -> Vec<String> {
        let create = match self.kind {
            RelationKind::MaterializedView => format!(
                "CREATE MATERIALIZED VIEW {} AS\n{}\nWITH DATA",
                self.name, self.select
            ),
            RelationKind::View | RelationKind::Table => {
                format!("CREATE VIEW {} AS\n{}", self.name, self.select)
            }
        };
        std::iter::once(create)
            .chain(self.indexes.iter().map(|&index| index.to_owned()))
            .collect()
    }
}

/// Every view, in creation order.
pub static VIEWS: [View; 3] = [
    View {
        name: "order_line_details",
        kind: RelationKind::View,
        select: ORDER_LINE_DETAILS_SQL,
        indexes: &[],
    },
    View {
        name: "daily_sales_live",
        kind: RelationKind::View,
        select: DAILY_SALES_SQL,
        indexes: &[],
    },
    View {
        name: "daily_sales",
        kind: RelationKind::MaterializedView,
        select: DAILY_SALES_SQL,
        indexes: &["CREATE UNIQUE INDEX daily_sales_id_idx ON daily_sales (id)"],
    },
];

/// The completion marker; see `db::SeedStatus`.
pub const CREATE_SEED_STATUS_SQL: &str = "CREATE TABLE IF NOT EXISTS seed_status (
  id BOOLEAN PRIMARY KEY DEFAULT true CHECK (id),
  schema_version INTEGER NOT NULL,
  state TEXT NOT NULL,
  orders BIGINT NOT NULL,
  order_items BIGINT NOT NULL,
  seed BIGINT NOT NULL,
  started_at TIMESTAMPTZ NOT NULL,
  finished_at TIMESTAMPTZ
)";

/// Drops every relation of every dataset (but not `seed_status`).
pub const DROP_ALL_SQL: &str = "DROP MATERIALIZED VIEW IF EXISTS daily_sales CASCADE;
DROP VIEW IF EXISTS daily_sales_live, order_line_details CASCADE;
DROP TABLE IF EXISTS order_items, orders, customers, products, categories, cities, countries CASCADE";

/// What the UI's "Show SQL" displays: the defining query of a view, or the
/// `CREATE TABLE` statement and indexes of a table.
pub fn defining_sql(dataset: &Dataset) -> String {
    if let Some(view) = VIEWS.iter().find(|view| view.name == dataset.relation) {
        return view.select.to_owned();
    }
    let table = table(dataset.relation);
    let mut statements = vec![table.create_sql()];
    statements.extend(table.indexes.iter().map(Index::create_sql));
    statements.join(";\n") + ";"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{DATASETS, ORDERS};

    #[test]
    fn orders_ddl_follows_the_catalog() {
        let ddl = table("orders").create_sql();
        assert!(ddl.starts_with("CREATE TABLE orders ("));
        assert!(ddl.contains("\"id\" BIGINT NOT NULL"));
        assert!(ddl.contains("\"ship_date\" DATE,"));
        assert!(ddl.contains("\"discount\" REAL NOT NULL"));
        assert!(!ddl.contains("PRIMARY KEY"), "added after loading");
        let constraints = table("orders").constraints_sql();
        assert_eq!(
            constraints,
            [
                "CREATE UNIQUE INDEX orders_pkey ON orders (id)",
                "CREATE INDEX orders_created_at_idx ON orders (created_at)",
                "ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_pkey",
                "ALTER TABLE orders ADD CONSTRAINT orders_customer_id_fkey \
                 FOREIGN KEY (customer_id) REFERENCES customers (id)",
            ]
        );
    }

    #[test]
    fn every_dataset_has_a_relation_and_sql() {
        for dataset in DATASETS {
            let sql = defining_sql(dataset);
            match dataset.kind {
                RelationKind::Table => assert!(sql.starts_with("CREATE TABLE"), "{sql}"),
                _ => assert!(sql.starts_with("SELECT"), "{sql}"),
            }
        }
        assert!(defining_sql(&ORDERS).contains("CREATE UNIQUE INDEX orders_pkey"));
    }

    #[test]
    fn the_daily_aggregate_is_one_string() {
        let live = VIEWS.iter().find(|v| v.name == "daily_sales_live").unwrap();
        let stored = VIEWS.iter().find(|v| v.name == "daily_sales").unwrap();
        assert!(std::ptr::eq(live.select, stored.select));
        assert!(stored.create_sql()[0].ends_with("WITH DATA"));
        assert!(DAILY_SALES_SQL.contains("JOIN categories ca ON ca.id = p.category_id"));
        assert!(ORDER_LINE_DETAILS_SQL.ends_with("JOIN categories ca ON ca.id = p.category_id"));
    }

    #[test]
    fn tables_are_listed_parents_first() {
        let position = |name: &str| TABLES.iter().position(|t| t.name == name).unwrap();
        for table in &TABLES {
            for fk in table.foreign_keys {
                let parent = fk
                    .split("REFERENCES ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap();
                assert!(position(parent) < position(table.name), "{fk}");
            }
        }
    }
}
