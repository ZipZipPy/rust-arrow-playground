//! Deterministic mock orders.
//!
//! Every chunk of rows draws from its own RNG seeded with `(seed, first id
//! of the chunk)`, and customer attributes come from an RNG seeded with
//! `(seed, customer_id)`. Output therefore depends only on the seed, the
//! total row count and the chunk layout, never on which worker generated a
//! chunk or in which order.

use rand::{Rng, RngExt, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use tokio_postgres::types::ToSql;

use super::chunks::Chunk;
use super::data::{
    CATEGORIES, COUNTRIES, City, Country, DISCOUNTS, EMAIL_DOMAINS, FIRST_NAMES, HOUR_WEIGHTS,
    LAST_NAMES, NOTES, PAYMENT_METHODS, QUANTITY_WEIGHTS, RATING_WEIGHTS,
};
use crate::pg_time::{Date, MICROS_PER_DAY, Timestamp};
use crate::schema::COLUMNS;

/// 2024-01-01T00:00:00Z: orders span the two calendar years 2024 and 2025.
const WINDOW_START: Timestamp = Timestamp(1_704_067_200_000_000);
const WINDOW_DAYS: i64 = 731;
const MICROS_PER_HOUR: i64 = 3_600_000_000;

const MIN_CUSTOMERS: u64 = 1000;
const NOT_SHIPPED_PROBABILITY: f64 = 0.15;
const RETURNED_PROBABILITY: f64 = 0.05;
const UNRATED_PROBABILITY: f64 = 0.40;
const NOTE_PROBABILITY: f64 = 0.15;
const PRICE_NOISE: f64 = 0.05;
/// About 5 km, so orders scatter around the city centre.
const COORDINATE_JITTER: f64 = 0.05;

/// Domain separation so chunk and customer RNG streams never coincide.
const CHUNK_STREAM: u64 = 0x6368_756e_6b73_0001;
const CUSTOMER_STREAM: u64 = 0x6375_7374_6f6d_0002;

/// One generated row, in catalog column order.
#[derive(Debug, Clone, PartialEq)]
pub struct Order {
    pub id: i64,
    pub created_at: Timestamp,
    pub ship_date: Option<Date>,
    pub customer_id: i32,
    pub customer_name: String,
    pub email: String,
    pub country: &'static str,
    pub city: &'static str,
    pub category: &'static str,
    pub product: &'static str,
    pub quantity: i16,
    pub unit_price: f64,
    pub discount: f32,
    pub total: f64,
    pub payment_method: &'static str,
    pub is_returned: bool,
    pub rating: Option<i16>,
    pub latitude: f64,
    pub longitude: f64,
    pub notes: Option<&'static str>,
}

impl Order {
    /// COPY values in catalog column order.
    pub fn values(&self) -> [&(dyn ToSql + Sync); COLUMNS.len()] {
        [
            &self.id,
            &self.created_at,
            &self.ship_date,
            &self.customer_id,
            &self.customer_name,
            &self.email,
            &self.country,
            &self.city,
            &self.category,
            &self.product,
            &self.quantity,
            &self.unit_price,
            &self.discount,
            &self.total,
            &self.payment_method,
            &self.is_returned,
            &self.rating,
            &self.latitude,
            &self.longitude,
            &self.notes,
        ]
    }
}

/// Picks indexes with probability proportional to their weight.
#[derive(Debug, Clone)]
pub struct WeightedIndex {
    cumulative: Vec<u32>,
}

impl WeightedIndex {
    pub fn new(weights: impl IntoIterator<Item = u32>) -> Self {
        let cumulative: Vec<u32> = weights
            .into_iter()
            .scan(0u32, |total, weight| {
                *total += weight;
                Some(*total)
            })
            .collect();
        assert!(
            cumulative.last().is_some_and(|&total| total > 0),
            "weights must not all be zero"
        );
        Self { cumulative }
    }

    pub fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> usize {
        let total = *self.cumulative.last().expect("checked in new");
        let point = rng.random_range(0..total);
        self.cumulative.partition_point(|&end| end <= point)
    }
}

struct Customer {
    name: String,
    email: String,
    country: &'static Country,
    city: &'static City,
}

pub struct Generator {
    seed: u64,
    customers: u64,
    countries: WeightedIndex,
    cities: Vec<WeightedIndex>,
    categories: WeightedIndex,
    payment_methods: WeightedIndex,
    quantities: WeightedIndex,
    discounts: WeightedIndex,
    ratings: WeightedIndex,
    hours: WeightedIndex,
}

impl Generator {
    /// `table_rows` is the size the table will have, which sets the number of
    /// customers (one per ten orders, at least 1000).
    pub fn new(seed: u64, table_rows: u64) -> Self {
        Self {
            seed,
            customers: (table_rows / 10).max(MIN_CUSTOMERS),
            countries: WeightedIndex::new(COUNTRIES.iter().map(|c| c.weight)),
            // Larger (earlier) cities get proportionally more customers.
            cities: COUNTRIES
                .iter()
                .map(|country| {
                    let count = country.cities.len() as u32;
                    WeightedIndex::new((0..count).map(|rank| 60 / (rank + 1)))
                })
                .collect(),
            categories: WeightedIndex::new(CATEGORIES.iter().map(|c| c.weight)),
            payment_methods: WeightedIndex::new(PAYMENT_METHODS.iter().map(|&(_, w)| w)),
            quantities: WeightedIndex::new(QUANTITY_WEIGHTS),
            discounts: WeightedIndex::new(DISCOUNTS.iter().map(|&(_, w)| w)),
            ratings: WeightedIndex::new(RATING_WEIGHTS),
            hours: WeightedIndex::new(HOUR_WEIGHTS),
        }
    }

    /// The orders of one chunk, in id order.
    pub fn orders(&self, chunk: Chunk) -> impl Iterator<Item = Order> + '_ {
        let mut rng = stream_rng(self.seed, CHUNK_STREAM, chunk.first_id as u64);
        chunk.ids().map(move |id| self.order(&mut rng, id))
    }

    fn order(&self, rng: &mut Xoshiro256PlusPlus, id: i64) -> Order {
        // A few loyal customers place many orders.
        let customer_id = 1 + (self.customers as f64 * rng.random::<f64>().powf(1.5)) as u64;
        let customer_id = customer_id.min(self.customers);
        let customer = self.customer(customer_id);

        let created_at = self.created_at(rng);
        let ship_date = (!rng.random_bool(NOT_SHIPPED_PROBABILITY))
            .then(|| Date(created_at.date().0 + rng.random_range(1..=10)));

        let category = &CATEGORIES[self.categories.sample(rng)];
        let product = &category.products[rng.random_range(0..category.products.len())];
        let quantity = self.quantities.sample(rng) as i16 + 1;
        let noise = rng.random_range(-PRICE_NOISE..PRICE_NOISE);
        let unit_price = round(product.base_price * (1.0 + noise), 2);
        let (discount_percent, _) = DISCOUNTS[self.discounts.sample(rng)];
        let discount_fraction = f64::from(discount_percent) / 100.0;
        let total = round(
            f64::from(quantity) * unit_price * (1.0 - discount_fraction),
            2,
        );

        let (payment_method, _) = PAYMENT_METHODS[self.payment_methods.sample(rng)];
        let is_returned = rng.random_bool(RETURNED_PROBABILITY);
        let rating =
            (!rng.random_bool(UNRATED_PROBABILITY)).then(|| self.ratings.sample(rng) as i16 + 1);
        let latitude = jitter(rng, customer.city.latitude);
        let longitude = jitter(rng, customer.city.longitude);
        let notes = rng
            .random_bool(NOTE_PROBABILITY)
            .then(|| NOTES[rng.random_range(0..NOTES.len())]);

        Order {
            id,
            created_at,
            ship_date,
            customer_id: customer_id as i32,
            customer_name: customer.name,
            email: customer.email,
            country: customer.country.code,
            city: customer.city.name,
            category: category.name,
            product: product.name,
            quantity,
            unit_price,
            discount: f32::from(discount_percent) / 100.0,
            total,
            payment_method,
            is_returned,
            rating,
            latitude,
            longitude,
            notes,
        }
    }

    /// Customers keep their name, email and home city across orders.
    fn customer(&self, customer_id: u64) -> Customer {
        let mut rng = stream_rng(self.seed, CUSTOMER_STREAM, customer_id);
        let first = FIRST_NAMES[rng.random_range(0..FIRST_NAMES.len())];
        let last = LAST_NAMES[rng.random_range(0..LAST_NAMES.len())];
        let domain = EMAIL_DOMAINS[rng.random_range(0..EMAIL_DOMAINS.len())];
        let country_index = self.countries.sample(&mut rng);
        let country = &COUNTRIES[country_index];
        let city = &country.cities[self.cities[country_index].sample(&mut rng)];
        Customer {
            name: format!("{first} {last}"),
            email: format!(
                "{}.{}{customer_id}@{domain}",
                ascii_slug(first),
                ascii_slug(last)
            ),
            country,
            city,
        }
    }

    /// Skewed towards recent days (a growing shop) and busy hours.
    fn created_at(&self, rng: &mut Xoshiro256PlusPlus) -> Timestamp {
        let day =
            ((WINDOW_DAYS as f64 * rng.random::<f64>().powf(0.8)) as i64).min(WINDOW_DAYS - 1);
        let hour = self.hours.sample(rng) as i64;
        let within_hour = rng.random_range(0..MICROS_PER_HOUR);
        Timestamp(WINDOW_START.0 + day * MICROS_PER_DAY + hour * MICROS_PER_HOUR + within_hour)
    }
}

fn stream_rng(seed: u64, stream: u64, index: u64) -> Xoshiro256PlusPlus {
    Xoshiro256PlusPlus::seed_from_u64(mix(mix(seed ^ stream) ^ index))
}

/// SplitMix64 finalizer: spreads nearby inputs over the whole range.
fn mix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn round(value: f64, decimals: i32) -> f64 {
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

fn jitter(rng: &mut Xoshiro256PlusPlus, coordinate: f64) -> f64 {
    round(
        coordinate + rng.random_range(-COORDINATE_JITTER..COORDINATE_JITTER),
        6,
    )
}

/// Lowercase ASCII letters for email addresses: accents are folded and
/// everything else (spaces, hyphens, apostrophes) dropped.
fn ascii_slug(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.chars().flat_map(char::to_lowercase) {
        match c {
            'a'..='z' | '0'..='9' => slug.push(c),
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => slug.push('a'),
            'é' | 'è' | 'ê' | 'ë' => slug.push('e'),
            'í' | 'ì' | 'î' | 'ï' => slug.push('i'),
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' => slug.push('o'),
            'ú' | 'ù' | 'û' | 'ü' => slug.push('u'),
            'ñ' | 'ń' => slug.push('n'),
            'ç' => slug.push('c'),
            'ł' => slug.push('l'),
            'ß' => slug.push_str("ss"),
            _ => {}
        }
    }
    slug
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::seed::chunks::ChunkPlan;

    fn chunk(first_id: i64, len: u64) -> Chunk {
        Chunk { first_id, len }
    }

    #[test]
    fn same_seed_and_chunk_give_the_same_rows() {
        let a: Vec<Order> = Generator::new(42, 10_000).orders(chunk(501, 200)).collect();
        let b: Vec<Order> = Generator::new(42, 10_000).orders(chunk(501, 200)).collect();
        assert_eq!(a, b);
        assert_eq!(a.first().unwrap().id, 501);
        assert_eq!(a.last().unwrap().id, 700);
    }

    #[test]
    fn different_seeds_or_chunks_give_different_rows() {
        let generator = Generator::new(42, 10_000);
        let base: Vec<Order> = generator.orders(chunk(1, 50)).collect();
        let other_seed: Vec<Order> = Generator::new(43, 10_000).orders(chunk(1, 50)).collect();
        let other_chunk: Vec<Order> = generator.orders(chunk(51, 50)).collect();
        assert_ne!(base, other_seed);
        assert_ne!(
            base.iter().map(|o| o.created_at).collect::<Vec<_>>(),
            other_chunk.iter().map(|o| o.created_at).collect::<Vec<_>>()
        );
    }

    #[test]
    fn output_does_not_depend_on_the_worker_count() {
        let plan = ChunkPlan::new(1, 5_000, 300);
        let generator = Generator::new(7, 5_000);
        let generate = |workers: usize| -> Vec<Order> {
            let queue = plan.queue();
            let mut orders: Vec<Order> = std::thread::scope(|scope| {
                let handles: Vec<_> = (0..workers)
                    .map(|_| {
                        scope.spawn(|| {
                            let mut mine = Vec::new();
                            while let Some(chunk) = queue.next() {
                                mine.extend(generator.orders(chunk));
                            }
                            mine
                        })
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|h| h.join().unwrap())
                    .collect()
            });
            orders.sort_by_key(|order| order.id);
            orders
        };
        let sequential = generate(1);
        assert_eq!(sequential.len(), 5_000);
        assert!(sequential.iter().zip(1..).all(|(order, id)| order.id == id));
        assert_eq!(generate(4), sequential);
        assert_eq!(generate(7), sequential);
    }

    #[test]
    fn values_follow_the_design() {
        let orders: Vec<Order> = Generator::new(1, 20_000).orders(chunk(1, 20_000)).collect();
        let window_end = WINDOW_START.0 + WINDOW_DAYS * MICROS_PER_DAY;
        let share = |predicate: &dyn Fn(&Order) -> bool| {
            orders.iter().filter(|o| predicate(o)).count() as f64 / orders.len() as f64
        };

        for order in &orders {
            assert!((WINDOW_START.0..window_end).contains(&order.created_at.0));
            if let Some(ship) = order.ship_date {
                assert!((1..=10).contains(&(ship.0 - order.created_at.date().0)));
            }
            assert!((1..=2000).contains(&order.customer_id));
            assert!((1..=20).contains(&order.quantity));
            assert!([0.0, 0.05, 0.10, 0.15, 0.20, 0.25].contains(&order.discount));
            let expected_total = round(
                f64::from(order.quantity) * order.unit_price * (1.0 - f64::from(order.discount)),
                2,
            );
            assert!((order.total - expected_total).abs() < 0.011, "{order:?}");
            assert!(order.rating.is_none_or(|r| (1..=5).contains(&r)));
            assert!((-90.0..=90.0).contains(&order.latitude));
            assert!((-180.0..=180.0).contains(&order.longitude));
            assert!(
                order.email.ends_with(".com")
                    || order.email.ends_with(".org")
                    || order.email.ends_with(".net")
            );
            assert!(order.email.contains(&order.customer_id.to_string()));
        }

        let near = |actual: f64, expected: f64| (actual - expected).abs() < 0.02;
        assert!(near(share(&|o| o.ship_date.is_none()), 0.15));
        assert!(near(share(&|o| o.rating.is_none()), 0.40));
        assert!(near(share(&|o| o.notes.is_some()), 0.15));
        assert!(near(share(&|o| o.is_returned), 0.05));

        let countries: HashSet<_> = orders.iter().map(|o| o.country).collect();
        assert!(countries.len() >= 20, "weighted countries all appear");
        let quantities_small = share(&|o| o.quantity <= 3);
        assert!(quantities_small > 0.5, "quantities skew small");
    }

    #[test]
    fn customers_are_consistent_across_orders() {
        let generator = Generator::new(3, 10_000);
        let mut seen = std::collections::HashMap::new();
        for order in generator.orders(chunk(1, 5_000)) {
            let identity = (order.customer_name.clone(), order.email.clone(), order.city);
            let previous = seen
                .entry(order.customer_id)
                .or_insert_with(|| identity.clone());
            assert_eq!(*previous, identity);
        }
    }

    #[test]
    fn every_name_has_an_ascii_slug() {
        for name in FIRST_NAMES.iter().chain(LAST_NAMES) {
            let slug = ascii_slug(name);
            assert!(
                !slug.is_empty() && slug.bytes().all(|b| b.is_ascii_alphanumeric()),
                "{name}"
            );
        }
        assert_eq!(ascii_slug("Łukasz"), "lukasz");
        assert_eq!(ascii_slug("O'Brien"), "obrien");
        assert_eq!(ascii_slug("van Rossum"), "vanrossum");
    }

    #[test]
    fn reference_data_matches_the_design() {
        assert!((20..=30).contains(&COUNTRIES.len()));
        for country in COUNTRIES {
            assert!((4..=10).contains(&country.cities.len()), "{}", country.code);
        }
        assert!((10..=14).contains(&CATEGORIES.len()));
        let mut products = HashSet::new();
        for category in CATEGORIES {
            assert!(
                (8..=20).contains(&category.products.len()),
                "{}",
                category.name
            );
            for product in category.products {
                assert!(
                    products.insert(product.name),
                    "duplicate product {}",
                    product.name
                );
            }
        }
        assert_eq!(PAYMENT_METHODS.len(), 6);
    }

    #[test]
    fn weighted_index_respects_weights() {
        let index = WeightedIndex::new([1, 0, 3]);
        let mut rng = Xoshiro256PlusPlus::seed_from_u64(1);
        let mut counts = [0; 3];
        for _ in 0..40_000 {
            counts[index.sample(&mut rng)] += 1;
        }
        assert_eq!(counts[1], 0);
        assert!((9_000..11_000).contains(&counts[0]), "{counts:?}");
    }

    #[test]
    fn window_start_is_2024() {
        let mut buf = [0; crate::pg_time::RFC3339_LEN];
        assert_eq!(
            WINDOW_START.write_rfc3339(&mut buf).unwrap(),
            "2024-01-01T00:00:00.000000Z"
        );
    }
}
