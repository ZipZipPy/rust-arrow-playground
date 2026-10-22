//! Seeding against a real database. Needs `TEST_DATABASE_URL`.

mod common;

use playground::config::SeedMode;
use playground::db;
use playground::seed::{self, SeedOutcome};

use common::DB_LOCK;

const SCHEMA: &str = "playground_seed_tests";

async fn checksum(client: &tokio_postgres::Client, filter: &str) -> String {
    client
        .query_one(
            &format!("SELECT md5(string_agg(o::text, '|' ORDER BY id)) FROM orders o {filter}"),
            &[],
        )
        .await
        .unwrap()
        .get(0)
}

async fn count(client: &tokio_postgres::Client, sql: &str) -> i64 {
    client.query_one(sql, &[]).await.unwrap().get(0)
}

#[tokio::test]
async fn seeding_is_deterministic_and_appendable() {
    let _lock = DB_LOCK.lock().await;
    let Some(database) = common::database(SCHEMA).await else {
        return;
    };
    let client = db::connect(&database).await.unwrap();

    let single = common::seed_config(&database, 2500, 1);
    let outcome = seed::run(&single).await.unwrap();
    assert!(matches!(outcome, SeedOutcome::Seeded { rows: 2500, .. }));
    let first = checksum(&client, "").await;

    let parallel = common::seed_config(&database, 2500, 4);
    seed::run(&parallel).await.unwrap();
    assert_eq!(
        checksum(&client, "").await,
        first,
        "worker count must not matter"
    );

    let indexes: Vec<String> = client
        .query("SELECT indexname::text FROM pg_indexes WHERE tablename = 'orders' AND schemaname = current_schema() ORDER BY 1", &[])
        .await
        .unwrap()
        .iter()
        .map(|row| row.get(0))
        .collect();
    assert_eq!(indexes, ["orders_created_at_idx", "orders_pkey"]);
    let stats = db::table_stats(&client).await.unwrap();
    assert_eq!((stats.row_estimate, stats.max_id), (2500, 2500), "analyzed");

    let mut if_empty = common::seed_config(&database, 999, 2);
    if_empty.if_empty = true;
    assert_eq!(seed::run(&if_empty).await.unwrap(), SeedOutcome::Skipped);
    assert_eq!(count(&client, "SELECT count(*) FROM orders").await, 2500);

    let mut append = common::seed_config(&database, 600, 2);
    append.mode = SeedMode::Append;
    seed::run(&append).await.unwrap();
    assert_eq!(count(&client, "SELECT count(*) FROM orders").await, 3100);
    assert_eq!(
        count(&client, "SELECT count(DISTINCT id) FROM orders").await,
        3100
    );
    assert_eq!(count(&client, "SELECT max(id) FROM orders").await, 3100);
    assert_eq!(
        checksum(&client, "WHERE id <= 2500").await,
        first,
        "appending leaves existing rows alone"
    );
    let repeated = count(
        &client,
        "SELECT count(*) FROM orders a JOIN orders b ON b.id = a.id + 2500 \
         WHERE a.created_at = b.created_at",
    )
    .await;
    assert_eq!(
        repeated, 0,
        "appended rows are new data, not a copy of the first rows"
    );

    let mut replace_into_existing = common::seed_config(&database, 0, 1);
    replace_into_existing.if_empty = false;
    seed::run(&replace_into_existing).await.unwrap();
    assert_eq!(count(&client, "SELECT count(*) FROM orders").await, 0);
    assert!(!db::table_has_rows(&client).await.unwrap());
}
