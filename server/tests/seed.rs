//! Seeding against a real database. Needs `TEST_DATABASE_URL`.

mod common;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use playground::config::{SeedConfig, SeedMode, ServerInfo};
use playground::schema::DATASETS;
use playground::seed::{self, SeedOutcome};
use playground::server::{self, AppState, Settings};
use playground::{db, ddl};
use serde_json::Value;
use tokio_postgres::Client;
use tower::ServiceExt;

use common::{DB_LOCK, count};

const SCHEMA: &str = "playground_seed_tests";

/// Every seeded relation with its key, in creation order.
const RELATIONS: [(&str, &str); 8] = [
    ("countries", "code"),
    ("cities", "id"),
    ("categories", "id"),
    ("products", "id"),
    ("customers", "id"),
    ("orders", "id"),
    ("order_items", "id"),
    ("daily_sales", "id"),
];

/// md5 over the rows of `relation` that match `filter`, in key order.
async fn checksum(client: &Client, relation: &str, filter: &str) -> String {
    let key = RELATIONS
        .iter()
        .find(|(name, _)| *name == relation)
        .unwrap()
        .1;
    client
        .query_one(
            &format!(
                "SELECT md5(string_agg(t::text, '|' ORDER BY {key})) FROM {relation} t {filter}"
            ),
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

async fn checksums(client: &Client) -> Vec<(&'static str, String)> {
    let mut sums = Vec::new();
    for (relation, _) in RELATIONS {
        sums.push((relation, checksum(client, relation, "").await));
    }
    sums
}

async fn status(client: &Client) -> db::SeedStatus {
    db::seed_status(client)
        .await
        .unwrap()
        .expect("a seed_status row")
}

fn lines(outcome: SeedOutcome) -> u64 {
    match outcome {
        SeedOutcome::Seeded { lines, .. } => lines,
        SeedOutcome::Skipped => panic!("expected a seed, got {outcome:?}"),
    }
}

fn if_empty(database: &tokio_postgres::Config, rows: u64) -> SeedConfig {
    SeedConfig {
        if_empty: true,
        ..common::seed_config(database, rows, 2)
    }
}

/// The checks every complete seed must pass: the normalized tables agree
/// with the flat `orders` rows and with each other, the views read them,
/// and `seed_status` counts what is there.
async fn assert_consistent(client: &Client) {
    let orders = count(client, "SELECT count(*) FROM orders").await;
    let lines = count(client, "SELECT count(*) FROM order_items").await;
    assert_eq!(
        count(client, "SELECT coalesce(max(id), 0) FROM orders").await,
        orders
    );
    assert_eq!(
        count(client, "SELECT count(*) FROM order_line_details").await,
        lines,
        "the join loses no line"
    );
    let customers = count(client, "SELECT count(*) FROM customers").await;
    assert_eq!(customers, (orders / 10).max(1000));
    assert_eq!(
        count(client, "SELECT max(id)::bigint FROM customers").await,
        customers
    );
    assert_eq!(
        count(
            client,
            "SELECT count(*) FROM orders o \
             WHERE NOT EXISTS (SELECT 1 FROM customers c WHERE c.id = o.customer_id)"
        )
        .await,
        0,
        "every order's customer exists"
    );

    // Line ids are order_id * 10 + line_no, lines 1..k without gaps, each
    // with another product.
    assert_eq!(
        count(
            client,
            "SELECT count(*) FROM order_items \
             WHERE id <> order_id * 10 + line_no OR line_no NOT BETWEEN 1 AND 5"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            client,
            "SELECT count(*) FROM order_items i WHERE line_no > 1 \
             AND NOT EXISTS (SELECT 1 FROM order_items p WHERE p.id = i.id - 1)"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            client,
            "SELECT count(*) FROM (SELECT order_id, product_id FROM order_items \
             GROUP BY 1, 2 HAVING count(*) > 1) d"
        )
        .await,
        0
    );
    if orders > 0 {
        let average = lines as f64 / orders as f64;
        assert!((2.2..2.8).contains(&average), "{average} lines per order");
    }

    // Line 1 is the order's main line: the view over the normalized tables
    // reads back exactly the flat columns.
    assert_eq!(
        count(
            client,
            "SELECT count(*) FROM orders o \
             JOIN order_line_details d ON d.line_id = o.id * 10 + 1 \
             JOIN customers c ON c.id = o.customer_id \
             WHERE (d.order_id, d.line_no, d.created_at, d.payment_method, d.is_returned, \
                    d.customer_id, d.customer_name, c.email, d.country_code, d.customer_city, \
                    d.category, d.product_name, d.quantity, d.unit_price, d.discount) \
                 = (o.id, 1::smallint, o.created_at, o.payment_method, o.is_returned, \
                    o.customer_id, o.customer_name, o.email, o.country, o.city, \
                    o.category, o.product, o.quantity, o.unit_price, o.discount) \
               AND d.ship_date IS NOT DISTINCT FROM o.ship_date"
        )
        .await,
        orders,
        "line 1 of every order equals the flat order"
    );

    // The materialized view and the live view share one SQL string.
    for (left, right) in [
        ("daily_sales", "daily_sales_live"),
        ("daily_sales_live", "daily_sales"),
    ] {
        assert_eq!(
            count(
                client,
                &format!(
                    "SELECT count(*) FROM (SELECT * FROM {left} EXCEPT SELECT * FROM {right}) d"
                )
            )
            .await,
            0,
            "{left} EXCEPT {right}"
        );
    }
    assert_eq!(
        count(
            client,
            "SELECT coalesce(sum(lines), 0)::bigint FROM daily_sales"
        )
        .await,
        lines
    );
    assert_eq!(
        count(
            client,
            "SELECT coalesce(sum(orders), 0)::bigint FROM daily_sales"
        )
        .await,
        orders
    );

    let status = status(client).await;
    assert!(status.is_complete(), "{status:?}");
    assert_eq!(status.schema_version, ddl::SCHEMA_VERSION);
    assert_eq!((status.orders, status.order_items), (orders, lines));
    assert!(status.finished_at.is_some());
}

#[tokio::test]
async fn seeds_are_consistent_and_independent_of_the_worker_count() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();

    let single = common::seed_config(&database, 2500, 1);
    let outcome = seed::run(&single).await.unwrap();
    assert!(matches!(outcome, SeedOutcome::Seeded { rows: 2500, .. }));
    assert_eq!(
        lines(outcome) as i64,
        count(&client, "SELECT count(*) FROM order_items").await
    );
    assert_consistent(&client).await;
    assert_eq!(status(&client).await.seed, 11);
    let first = checksums(&client).await;

    let parallel = common::seed_config(&database, 2500, 4);
    seed::run(&parallel).await.unwrap();
    assert_eq!(
        checksums(&client).await,
        first,
        "worker count must not matter"
    );

    let indexes: Vec<String> = client
        .query(
            "SELECT indexname::text FROM pg_indexes WHERE schemaname = current_schema() ORDER BY 1",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        indexes,
        [
            "categories_name_key",
            "categories_pkey",
            "cities_pkey",
            "countries_pkey",
            "customers_pkey",
            "daily_sales_id_idx",
            "order_items_order_id_idx",
            "order_items_pkey",
            "orders_created_at_idx",
            "orders_pkey",
            "products_pkey",
            "seed_status_pkey",
        ]
    );
    let foreign_keys: Vec<String> = client
        .query(
            "SELECT conname::text FROM pg_constraint \
             WHERE contype = 'f' AND connamespace = current_schema()::regnamespace ORDER BY 1",
            &[],
        )
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(
        foreign_keys,
        [
            "cities_country_code_fkey",
            "customers_city_id_fkey",
            "order_items_order_id_fkey",
            "order_items_product_id_fkey",
            "orders_customer_id_fkey",
            "products_category_id_fkey",
        ]
    );

    // VACUUM (ANALYZE) ran, so /api/info's estimates are exact.
    for dataset in DATASETS {
        let stats = db::dataset_stats(&client, dataset).await.unwrap();
        let rows = count(
            &client,
            &format!("SELECT count(*) FROM {}", dataset.relation),
        )
        .await;
        assert_eq!(stats.row_estimate, Some(rows), "{}", dataset.name);
    }
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM pg_stat_user_tables \
             WHERE schemaname = current_schema() AND relname <> 'seed_status' \
             AND last_vacuum IS NULL"
        )
        .await,
        0,
        "every table was vacuumed"
    );
}

#[tokio::test]
async fn append_adds_new_orders_lines_and_customers() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();
    seed::run(&common::seed_config(&database, 10_000, 3))
        .await
        .unwrap();
    let before = status(&client).await;
    let orders = checksum(&client, "orders", "").await;
    let order_items = checksum(&client, "order_items", "").await;
    let customers = checksum(&client, "customers", "").await;
    let days = count(&client, "SELECT count(*) FROM daily_sales").await;

    let append = SeedConfig {
        mode: SeedMode::Append,
        ..common::seed_config(&database, 2000, 2)
    };
    let outcome = seed::run(&append).await.unwrap();
    assert!(matches!(outcome, SeedOutcome::Seeded { rows: 2000, .. }));
    assert_consistent(&client).await;
    assert_eq!(count(&client, "SELECT count(*) FROM orders").await, 12_000);
    assert_eq!(
        count(
            &client,
            "SELECT count(*) FROM order_items WHERE order_id > 10000"
        )
        .await,
        lines(outcome) as i64
    );
    assert_eq!(count(&client, "SELECT count(*) FROM customers").await, 1200);
    assert!(
        count(
            &client,
            "SELECT count(*) FROM orders WHERE customer_id > 1000"
        )
        .await
            > 0,
        "new orders reference the new customers"
    );
    assert!(
        count(&client, "SELECT count(*) FROM daily_sales").await > days,
        "the materialized view was refreshed"
    );

    assert_eq!(
        checksum(&client, "orders", "WHERE id <= 10000").await,
        orders
    );
    assert_eq!(
        checksum(&client, "order_items", "WHERE order_id <= 10000").await,
        order_items
    );
    assert_eq!(
        checksum(&client, "customers", "WHERE id <= 1000").await,
        customers,
        "appending leaves existing rows alone"
    );
    let repeated = count(
        &client,
        "SELECT count(*) FROM orders a JOIN orders b ON b.id = a.id + 10000 \
         WHERE a.created_at = b.created_at",
    )
    .await;
    assert_eq!(
        repeated, 0,
        "appended rows are new data, not a copy of the first rows"
    );

    let after = status(&client).await;
    assert_ne!(
        after.generation(),
        before.generation(),
        "running servers drop their row caches"
    );
    assert!(after.started_at > before.started_at);
    assert!(after.finished_at.unwrap() > after.started_at);
}

#[tokio::test]
async fn if_empty_keeps_only_a_complete_seed_of_this_schema() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();

    // A version 1 database: `orders` only, and no seed_status. `serve`
    // starts first and adds the missing relations around it.
    client
        .batch_execute(&format!(
            "{}; ALTER TABLE orders ADD PRIMARY KEY (id); INSERT INTO orders \
             SELECT g, now(), NULL, 1, 'A B', 'a@b', 'TH', 'Bangkok', 'Books', 'Atlas', \
                    1, 1.0, 0, 1.0, 'card', false, NULL, 0, 0, NULL \
             FROM generate_series(1, 50) g",
            ddl::table("orders").create_sql()
        ))
        .await
        .unwrap();
    let created = db::create_missing_relations(&client).await.unwrap();
    assert_eq!(created.len(), 9, "everything but orders: {created:?}");
    assert!(!created.contains(&"orders"));
    assert_eq!(
        count(&client, "SELECT count(*) FROM order_line_details").await,
        0
    );
    assert!(db::seed_status(&client).await.unwrap().is_none());

    let outcome = seed::run(&if_empty(&database, 1500)).await.unwrap();
    assert!(
        matches!(outcome, SeedOutcome::Seeded { rows: 1500, .. }),
        "v1 schema"
    );
    assert_consistent(&client).await;
    let seeded = checksums(&client).await;

    assert_eq!(
        seed::run(&if_empty(&database, 999)).await.unwrap(),
        SeedOutcome::Skipped
    );
    assert_eq!(checksums(&client).await, seeded);

    // An interrupted seed leaves `loading` behind.
    client
        .execute("UPDATE seed_status SET state = 'loading'", &[])
        .await
        .unwrap();
    let append = SeedConfig {
        mode: SeedMode::Append,
        ..common::seed_config(&database, 100, 1)
    };
    let err = seed::run(&append).await.unwrap_err().to_string();
    assert!(err.contains("needs a complete seed"), "{err}");
    let outcome = seed::run(&if_empty(&database, 1200)).await.unwrap();
    assert!(
        matches!(outcome, SeedOutcome::Seeded { rows: 1200, .. }),
        "loading"
    );
    assert_consistent(&client).await;

    // A complete seed of an older schema version.
    client
        .execute("UPDATE seed_status SET schema_version = 1", &[])
        .await
        .unwrap();
    let outcome = seed::run(&if_empty(&database, 1100)).await.unwrap();
    assert!(
        matches!(outcome, SeedOutcome::Seeded { rows: 1100, .. }),
        "version 1"
    );
    assert_consistent(&client).await;

    // Replacing with no orders still leaves every dataset readable.
    seed::run(&common::seed_config(&database, 0, 1))
        .await
        .unwrap();
    assert_consistent(&client).await;
    assert_eq!(
        count(&client, "SELECT count(*) FROM daily_sales_live").await,
        0
    );
}

#[tokio::test]
async fn seeders_take_turns() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();
    let waiting = async || {
        count(
            &client,
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted",
        )
        .await
    };
    let wait_for = async |expected: i64| {
        let deadline = Instant::now() + Duration::from_secs(10);
        while waiting().await != expected {
            assert!(
                Instant::now() < deadline,
                "expected {expected} waiting seeders"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };

    // While another session holds the lock, seeders wait without touching
    // anything, then run one after the other.
    let holder = db::connect(&database).await.unwrap();
    db::lock_seeding(&holder).await.unwrap();
    let spawn_seeder = || {
        let config = if_empty(&database, 1200);
        tokio::spawn(async move { seed::run(&config).await.unwrap() })
    };
    let first = spawn_seeder();
    wait_for(1).await;
    let second = spawn_seeder();
    wait_for(2).await;
    assert!(!first.is_finished() && !second.is_finished());
    assert!(db::seed_status(&client).await.unwrap().is_none());
    assert!(!db::relation_exists(&client, "orders").await.unwrap());
    assert!(
        db::create_missing_relations(&client)
            .await
            .unwrap()
            .is_empty(),
        "serve leaves the relations to the seeder"
    );

    db::unlock_seeding(&holder).await.unwrap();
    let mut outcomes = [first.await.unwrap(), second.await.unwrap()];
    outcomes.sort_by_key(|outcome| matches!(outcome, SeedOutcome::Seeded { .. }));
    assert_eq!(
        outcomes[0],
        SeedOutcome::Skipped,
        "the second saw the first's seed"
    );
    assert!(matches!(
        outcomes[1],
        SeedOutcome::Seeded { rows: 1200, .. }
    ));
    assert_consistent(&client).await;

    // Two replacing seeders at once: both run, one after the other.
    let replace = common::seed_config(&database, 1300, 2);
    let (a, b) = tokio::join!(seed::run(&replace), seed::run(&replace));
    assert!(matches!(a.unwrap(), SeedOutcome::Seeded { rows: 1300, .. }));
    assert!(matches!(b.unwrap(), SeedOutcome::Seeded { rows: 1300, .. }));
    assert_consistent(&client).await;
    assert_eq!(waiting().await, 0);
}

#[tokio::test]
async fn serve_answers_before_the_first_seed() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();
    let created = db::create_missing_relations(&client).await.unwrap();
    assert_eq!(
        created,
        [
            "countries",
            "cities",
            "categories",
            "products",
            "customers",
            "orders",
            "order_items",
            "order_line_details",
            "daily_sales_live",
            "daily_sales",
        ]
    );
    assert!(
        db::create_missing_relations(&client)
            .await
            .unwrap()
            .is_empty()
    );

    // Empty relations with the catalog's columns and types.
    for dataset in DATASETS {
        let statement = client
            .prepare(&format!("SELECT * FROM {}", dataset.relation))
            .await
            .unwrap();
        let actual: Vec<(&str, String)> = statement
            .columns()
            .iter()
            .map(|c| (c.name(), c.type_().name().to_owned()))
            .collect();
        let expected: Vec<(&str, String)> = dataset
            .columns
            .iter()
            .map(|c| (c.name, c.ty.pg_name().to_owned()))
            .collect();
        assert_eq!(actual, expected, "{}", dataset.name);
    }

    let router = router(&database);
    let (status, info) = get(&router, "/api/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["seed"], Value::Null);
    for dataset in info["datasets"].as_array().unwrap() {
        assert_eq!(dataset["row_estimate"], 0, "{}", dataset["name"]);
        assert_eq!(dataset["max_key"], 0, "{}", dataset["name"]);
    }
    for dataset in DATASETS {
        let (status, rows) = get(
            &router,
            &format!("/api/data?dataset={}&cache=true", dataset.name),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}", dataset.name);
        assert_eq!(rows, Value::Array(Vec::new()));
    }
}

/// An interrupted seed can leave tables without their keys yet, so some
/// relations cannot be created; `serve` still starts and `/api/info` still
/// answers.
#[tokio::test]
async fn serve_answers_after_an_interrupted_seed() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::fresh_database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();
    // As steps 1 to 3 leave it: customers loaded, its primary key not built.
    client
        .batch_execute(&ddl::table("customers").create_sql())
        .await
        .unwrap();
    let created = db::create_missing_relations(&client).await.unwrap();
    assert_eq!(created, ["countries", "cities", "categories", "products"]);

    let router = router(&database);
    let (status, info) = get(&router, "/api/info").await;
    assert_eq!(status, StatusCode::OK);
    for dataset in info["datasets"].as_array().unwrap() {
        assert_eq!(dataset["row_estimate"], Value::Null, "{}", dataset["name"]);
        assert_eq!(dataset["bytes"], Value::Null, "{}", dataset["name"]);
    }
    let (status, error) = get(&router, "/api/data?dataset=order_lines").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(error["error"].is_string());
}

fn router(database: &tokio_postgres::Config) -> axum::Router {
    let pool = db::create_pool(database.clone(), 2).unwrap();
    let state = AppState::new(
        pool,
        Settings {
            max_rows: 1000,
            cache_entries: 2,
            cache_max_rows: 1000,
            seed_check_interval: server::SEED_CHECK_INTERVAL,
            server_info: ServerInfo::from_env(),
        },
    );
    server::router(state, std::path::Path::new("/nonexistent/dist"))
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap())
}
