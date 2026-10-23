//! Deterministic mock data: dimension tables, customers, orders and their
//! lines.
//!
//! Every chunk of orders draws from its own RNG seeded with `(seed, first id
//! of the chunk)`, the extra lines of an order from `(seed, order_id)`,
//! customer attributes from `(seed, customer_id)` and city and product
//! attributes from `(seed, their id)`. Output therefore depends only on the
//! seed, the total row count and the chunk layout, never on which worker
//! generated a chunk or in which order.

use rand::{Rng, RngExt, SeedableRng};
use rand_xoshiro::Xoshiro256PlusPlus;
use tokio_postgres::types::{ToSql, Type};

use super::chunks::Chunk;
use super::copy::CopyRow;
use super::data::{
    BRANDS, CATEGORIES, COUNTRIES, Category, City, Country, DISCOUNTS, EMAIL_DOMAINS, FIRST_NAMES,
    HOUR_WEIGHTS, LAST_NAMES, LINE_COUNT_WEIGHTS, NOTE_CHANNELS, NOTE_OCCASIONS, NOTE_PLACES,
    PAYMENT_METHODS, Product, QUANTITY_WEIGHTS, RATING_WEIGHTS, SEGMENTS, SIZES, WEEKDAYS,
};
use crate::pg_time::{Date, MICROS_PER_DAY, Timestamp};
use crate::schema::ORDERS_COLUMNS;

/// 2024-01-01T00:00:00Z: orders span the two calendar years 2024 and 2025.
const WINDOW_START: Timestamp = Timestamp(1_704_067_200_000_000);
const WINDOW_DAYS: i64 = 731;
const MICROS_PER_HOUR: i64 = 3_600_000_000;
/// Customers signed up from 2016-01-01 to 2023-12-31, so before any order.
const SIGNUP_START: Date = Date(16_801);
const SIGNUP_DAYS: i32 = 2_922;
/// Products launched from 2015-01-01 to 2023-12-31.
const LAUNCH_START: Date = Date(16_436);
const LAUNCH_DAYS: i32 = 3_287;
const BIRTH_YEARS: std::ops::RangeInclusive<i16> = 1945..=2006;

const MIN_CUSTOMERS: u64 = 1000;
const NOT_SHIPPED_PROBABILITY: f64 = 0.15;
const RETURNED_PROBABILITY: f64 = 0.05;
const UNRATED_PROBABILITY: f64 = 0.40;
const NOTE_PROBABILITY: f64 = 0.15;
const NO_BIRTH_YEAR_PROBABILITY: f64 = 0.20;
const ACTIVE_PROBABILITY: f64 = 0.90;
const PRICE_NOISE: f64 = 0.05;
/// About 5 km, so orders scatter around the city centre.
const COORDINATE_JITTER: f64 = 0.05;
/// `order_items.id = order_id * 10 + line_no`, so at most 9 lines fit.
pub const LINE_ID_FACTOR: i64 = 10;

/// Domain separation so the RNG streams never coincide.
const CHUNK_STREAM: u64 = 0x6368_756e_6b73_0001;
const CUSTOMER_STREAM: u64 = 0x6375_7374_6f6d_0002;
const LINE_STREAM: u64 = 0x6c69_6e65_7300_0003;
const CITY_STREAM: u64 = 0x6369_7479_0000_0004;
const PRODUCT_STREAM: u64 = 0x7072_6f64_0000_0005;

/// One generated `orders` row (in catalog column order) plus the product id
/// of its main line.
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
    pub notes: Option<String>,
    /// `products.id` of `product`: line 1 of the order.
    pub product_id: i32,
}

impl CopyRow for Order {
    const TABLE: &'static str = "orders";

    fn columns() -> Vec<&'static str> {
        ORDERS_COLUMNS.iter().map(|column| column.name).collect()
    }

    fn types() -> Vec<Type> {
        ORDERS_COLUMNS
            .iter()
            .map(|column| column.ty.pg_type())
            .collect()
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
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

/// One `order_items` row.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderLine {
    pub id: i64,
    pub order_id: i64,
    pub line_no: i16,
    pub product_id: i32,
    pub quantity: i16,
    pub unit_price: f64,
    pub discount: f32,
}

impl CopyRow for OrderLine {
    const TABLE: &'static str = "order_items";

    fn columns() -> Vec<&'static str> {
        vec![
            "id",
            "order_id",
            "line_no",
            "product_id",
            "quantity",
            "unit_price",
            "discount",
        ]
    }

    fn types() -> Vec<Type> {
        vec![
            Type::INT8,
            Type::INT8,
            Type::INT2,
            Type::INT4,
            Type::INT2,
            Type::FLOAT8,
            Type::FLOAT4,
        ]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
            &self.order_id,
            &self.line_no,
            &self.product_id,
            &self.quantity,
            &self.unit_price,
            &self.discount,
        ]
    }
}

/// One `customers` row, plus the home city every order of theirs ships to.
#[derive(Debug, Clone, PartialEq)]
pub struct Customer {
    pub id: i32,
    pub name: String,
    pub email: String,
    pub segment: &'static str,
    pub signup_date: Date,
    pub city_id: i32,
    pub birth_year: Option<i16>,
    pub is_active: bool,
    country: &'static Country,
    city: &'static City,
}

impl CopyRow for Customer {
    const TABLE: &'static str = "customers";

    fn columns() -> Vec<&'static str> {
        vec![
            "id",
            "name",
            "email",
            "segment",
            "signup_date",
            "city_id",
            "birth_year",
            "is_active",
        ]
    }

    fn types() -> Vec<Type> {
        vec![
            Type::INT4,
            Type::TEXT,
            Type::TEXT,
            Type::TEXT,
            Type::DATE,
            Type::INT4,
            Type::INT2,
            Type::BOOL,
        ]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
            &self.name,
            &self.email,
            &self.segment,
            &self.signup_date,
            &self.city_id,
            &self.birth_year,
            &self.is_active,
        ]
    }
}

/// One `countries` row.
#[derive(Debug, Clone, Copy)]
pub struct CountryRow(&'static Country);

impl CopyRow for CountryRow {
    const TABLE: &'static str = "countries";

    fn columns() -> Vec<&'static str> {
        vec!["code", "name", "region", "currency", "vat_rate"]
    }

    fn types() -> Vec<Type> {
        vec![Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT, Type::FLOAT4]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        let country = self.0;
        [
            &country.code as &(dyn ToSql + Sync),
            &country.name,
            &country.region,
            &country.currency,
            &country.vat_rate,
        ]
    }
}

/// One `cities` row.
#[derive(Debug, Clone)]
pub struct CityRow {
    pub id: i32,
    pub country_code: &'static str,
    pub population: i32,
    city: &'static City,
}

impl CopyRow for CityRow {
    const TABLE: &'static str = "cities";

    fn columns() -> Vec<&'static str> {
        vec![
            "id",
            "name",
            "country_code",
            "latitude",
            "longitude",
            "population",
        ]
    }

    fn types() -> Vec<Type> {
        vec![
            Type::INT4,
            Type::TEXT,
            Type::TEXT,
            Type::FLOAT8,
            Type::FLOAT8,
            Type::INT4,
        ]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
            &self.city.name,
            &self.country_code,
            &self.city.latitude,
            &self.city.longitude,
            &self.population,
        ]
    }
}

/// One `categories` row.
#[derive(Debug, Clone)]
pub struct CategoryRow {
    pub id: i32,
    category: &'static Category,
}

impl CopyRow for CategoryRow {
    const TABLE: &'static str = "categories";

    fn columns() -> Vec<&'static str> {
        vec!["id", "name", "department"]
    }

    fn types() -> Vec<Type> {
        vec![Type::INT4, Type::TEXT, Type::TEXT]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
            &self.category.name,
            &self.category.department,
        ]
    }
}

/// One `products` row.
#[derive(Debug, Clone)]
pub struct ProductRow {
    pub id: i32,
    pub category_id: i32,
    pub brand: &'static str,
    pub weight_kg: f32,
    pub launched_on: Date,
    product: &'static Product,
}

impl CopyRow for ProductRow {
    const TABLE: &'static str = "products";

    fn columns() -> Vec<&'static str> {
        vec![
            "id",
            "name",
            "category_id",
            "brand",
            "base_price",
            "weight_kg",
            "launched_on",
        ]
    }

    fn types() -> Vec<Type> {
        vec![
            Type::INT4,
            Type::TEXT,
            Type::INT4,
            Type::TEXT,
            Type::FLOAT8,
            Type::FLOAT4,
            Type::DATE,
        ]
    }

    fn values(&self) -> impl AsRef<[&(dyn ToSql + Sync)]> + Send {
        [
            &self.id as &(dyn ToSql + Sync),
            &self.product.name,
            &self.category_id,
            &self.brand,
            &self.product.base_price,
            &self.weight_kg,
            &self.launched_on,
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

/// `first_ids[i]` is the id of the first item of group `i` when the items
/// of all groups are numbered in list order from 1.
fn first_ids(group_sizes: impl Iterator<Item = usize>) -> Vec<i32> {
    group_sizes
        .scan(1i32, |next, size| {
            let first = *next;
            *next += size as i32;
            Some(first)
        })
        .collect()
}

pub struct Generator {
    seed: u64,
    customers: u64,
    countries: WeightedIndex,
    cities: Vec<WeightedIndex>,
    first_city_ids: Vec<i32>,
    categories: WeightedIndex,
    first_product_ids: Vec<i32>,
    payment_methods: WeightedIndex,
    quantities: WeightedIndex,
    discounts: WeightedIndex,
    ratings: WeightedIndex,
    hours: WeightedIndex,
    line_counts: WeightedIndex,
    segments: WeightedIndex,
}

impl Generator {
    /// `orders` is the number of orders the table will hold, which sets the
    /// number of customers (one per ten orders, at least 1000).
    pub fn new(seed: u64, orders: u64) -> Self {
        Self {
            seed,
            customers: (orders / 10).max(MIN_CUSTOMERS),
            countries: WeightedIndex::new(COUNTRIES.iter().map(|c| c.weight)),
            // Larger (earlier) cities get proportionally more customers.
            cities: COUNTRIES
                .iter()
                .map(|country| {
                    let count = country.cities.len() as u32;
                    WeightedIndex::new((0..count).map(|rank| 60 / (rank + 1)))
                })
                .collect(),
            first_city_ids: first_ids(COUNTRIES.iter().map(|c| c.cities.len())),
            categories: WeightedIndex::new(CATEGORIES.iter().map(|c| c.weight)),
            first_product_ids: first_ids(CATEGORIES.iter().map(|c| c.products.len())),
            payment_methods: WeightedIndex::new(PAYMENT_METHODS.iter().map(|&(_, w)| w)),
            quantities: WeightedIndex::new(QUANTITY_WEIGHTS),
            discounts: WeightedIndex::new(DISCOUNTS.iter().map(|&(_, w)| w)),
            ratings: WeightedIndex::new(RATING_WEIGHTS),
            hours: WeightedIndex::new(HOUR_WEIGHTS),
            line_counts: WeightedIndex::new(LINE_COUNT_WEIGHTS),
            segments: WeightedIndex::new(SEGMENTS.iter().map(|&(_, w)| w)),
        }
    }

    /// Customer ids orders can reference: `1..=customer_count()`.
    pub fn customer_count(&self) -> u64 {
        self.customers
    }

    /// The orders of one chunk, in id order.
    pub fn orders(&self, chunk: Chunk) -> impl Iterator<Item = Order> + '_ {
        let mut rng = stream_rng(self.seed, CHUNK_STREAM, chunk.first_id as u64);
        chunk.ids().map(move |id| self.order(&mut rng, id))
    }

    /// The customers with the ids of `chunk`.
    pub fn customers(&self, chunk: Chunk) -> impl Iterator<Item = Customer> + '_ {
        chunk.ids().map(|id| self.customer(id as u64))
    }

    fn order(&self, rng: &mut Xoshiro256PlusPlus, id: i64) -> Order {
        // A few loyal customers place many orders.
        let customer_id = 1 + (self.customers as f64 * rng.random::<f64>().powf(1.5)) as u64;
        let customer_id = customer_id.min(self.customers);
        let customer = self.customer(customer_id);

        let created_at = self.created_at(rng);
        let ship_date = (!rng.random_bool(NOT_SHIPPED_PROBABILITY))
            .then(|| Date(created_at.date().0 + rng.random_range(1..=10)));

        let (product_id, category, product) = self.product(rng);
        let quantity = self.quantities.sample(rng) as i16 + 1;
        let unit_price = price(rng, product);
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
        let notes = rng.random_bool(NOTE_PROBABILITY).then(|| note(rng));

        Order {
            id,
            created_at,
            ship_date,
            customer_id: customer_id as i32,
            country: customer.country.code,
            city: customer.city.name,
            customer_name: customer.name,
            email: customer.email,
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
            product_id,
        }
    }

    /// Appends every line of `order` to `lines`: line 1 is the order's own
    /// product, quantity, price and discount; lines 2..=k (k is 1..=5,
    /// about 2.5 on average) are other products, drawn from `(seed, order
    /// id)` alone.
    pub fn push_lines(&self, order: &Order, lines: &mut Vec<OrderLine>) {
        let line = |line_no: i16, product_id, quantity, unit_price, discount| OrderLine {
            id: order.id * LINE_ID_FACTOR + i64::from(line_no),
            order_id: order.id,
            line_no,
            product_id,
            quantity,
            unit_price,
            discount,
        };
        lines.push(line(
            1,
            order.product_id,
            order.quantity,
            order.unit_price,
            order.discount,
        ));

        let mut rng = stream_rng(self.seed, LINE_STREAM, order.id as u64);
        let count = self.line_counts.sample(&mut rng) + 1;
        let mut used = vec![order.product_id];
        for line_no in 2..=count as i16 {
            // An order lists each product once.
            let (product_id, _, product) = loop {
                let candidate = self.product(&mut rng);
                if !used.contains(&candidate.0) {
                    break candidate;
                }
            };
            used.push(product_id);
            let quantity = self.quantities.sample(&mut rng) as i16 + 1;
            let unit_price = price(&mut rng, product);
            let (discount_percent, _) = DISCOUNTS[self.discounts.sample(&mut rng)];
            let discount = f32::from(discount_percent) / 100.0;
            lines.push(line(line_no, product_id, quantity, unit_price, discount));
        }
    }

    /// A product: category by weight, then uniform within the category.
    fn product(&self, rng: &mut Xoshiro256PlusPlus) -> (i32, &'static Category, &'static Product) {
        let category_index = self.categories.sample(rng);
        let category = &CATEGORIES[category_index];
        let product_index = rng.random_range(0..category.products.len());
        let id = self.first_product_ids[category_index] + product_index as i32;
        (id, category, &category.products[product_index])
    }

    /// Customers keep their name, email, segment and home city across
    /// orders; every order of theirs ships to that city.
    pub fn customer(&self, customer_id: u64) -> Customer {
        let mut rng = stream_rng(self.seed, CUSTOMER_STREAM, customer_id);
        let first = FIRST_NAMES[rng.random_range(0..FIRST_NAMES.len())];
        let initial = char::from(b'A' + rng.random_range(0..26u8));
        let last = LAST_NAMES[rng.random_range(0..LAST_NAMES.len())];
        let domain = EMAIL_DOMAINS[rng.random_range(0..EMAIL_DOMAINS.len())];
        let country_index = self.countries.sample(&mut rng);
        let country = &COUNTRIES[country_index];
        let city_index = self.cities[country_index].sample(&mut rng);
        let (segment, _) = SEGMENTS[self.segments.sample(&mut rng)];
        let signup_date = Date(SIGNUP_START.0 + rng.random_range(0..SIGNUP_DAYS));
        let birth_year =
            (!rng.random_bool(NO_BIRTH_YEAR_PROBABILITY)).then(|| rng.random_range(BIRTH_YEARS));
        let is_active = rng.random_bool(ACTIVE_PROBABILITY);
        Customer {
            id: customer_id as i32,
            name: format!("{first} {initial}. {last}"),
            email: format!(
                "{}.{}{customer_id}@{domain}",
                ascii_slug(first),
                ascii_slug(last)
            ),
            segment,
            signup_date,
            city_id: self.first_city_ids[country_index] + city_index as i32,
            birth_year,
            is_active,
            country,
            city: &country.cities[city_index],
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

    pub fn countries(&self) -> Vec<CountryRow> {
        COUNTRIES.iter().map(CountryRow).collect()
    }

    /// Cities numbered in list order; larger (earlier) cities of a country
    /// are more populous.
    pub fn cities(&self) -> Vec<CityRow> {
        COUNTRIES
            .iter()
            .zip(&self.first_city_ids)
            .flat_map(|(country, &first_id)| {
                country.cities.iter().enumerate().map(move |(rank, city)| {
                    let id = first_id + rank as i32;
                    let mut rng = stream_rng(self.seed, CITY_STREAM, id as u64);
                    let size = rng.random_range(0.7..1.3) * 6_000_000.0;
                    let population = size / (rank as f64 + 1.0).powf(1.2);
                    CityRow {
                        id,
                        country_code: country.code,
                        population: population as i32 + rng.random_range(20_000..80_000),
                        city,
                    }
                })
            })
            .collect()
    }

    pub fn categories(&self) -> Vec<CategoryRow> {
        CATEGORIES
            .iter()
            .zip(1..)
            .map(|(category, id)| CategoryRow { id, category })
            .collect()
    }

    /// Products numbered in list order, with a brand, shipping weight and
    /// launch date drawn from `(seed, product id)`.
    pub fn products(&self) -> Vec<ProductRow> {
        CATEGORIES
            .iter()
            .zip(1..)
            .zip(&self.first_product_ids)
            .flat_map(|((category, category_id), &first_id)| {
                category
                    .products
                    .iter()
                    .zip(first_id..)
                    .map(move |(product, id)| {
                        let mut rng = stream_rng(self.seed, PRODUCT_STREAM, id as u64);
                        let (min_kg, max_kg) = category.weight_kg;
                        ProductRow {
                            id,
                            category_id,
                            brand: BRANDS[rng.random_range(0..BRANDS.len())],
                            weight_kg: round(f64::from(rng.random_range(min_kg..max_kg)), 2) as f32,
                            launched_on: Date(LAUNCH_START.0 + rng.random_range(0..LAUNCH_DAYS)),
                            product,
                        }
                    })
            })
            .collect()
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

/// The product's base price with a little noise, in cents.
fn price(rng: &mut Xoshiro256PlusPlus, product: &Product) -> f64 {
    let noise = rng.random_range(-PRICE_NOISE..PRICE_NOISE);
    round(product.base_price * (1.0 + noise), 2)
}

fn jitter(rng: &mut Xoshiro256PlusPlus, coordinate: f64) -> f64 {
    round(
        coordinate + rng.random_range(-COORDINATE_JITTER..COORDINATE_JITTER),
        6,
    )
}

fn pick<T: Copy>(rng: &mut Xoshiro256PlusPlus, items: &[T]) -> T {
    items[rng.random_range(0..items.len())]
}

/// A delivery note: one of a dozen templates with names, places and
/// numbers filled in, often followed by a sign-off or reference, so notes
/// are about as varied as real free text.
fn note(rng: &mut Xoshiro256PlusPlus) -> String {
    let mut note = match rng.random_range(0..13) {
        0 => format!("Please leave the parcel {}.", pick(rng, NOTE_PLACES)),
        1 => format!("Deliver after {} pm.", rng.random_range(1..=8)),
        2 => format!("Call {} on arrival.", pick(rng, FIRST_NAMES)),
        3 => format!(
            "Gift for {}'s {}, please wrap it.",
            pick(rng, FIRST_NAMES),
            pick(rng, NOTE_OCCASIONS)
        ),
        4 => format!(
            "Ring the bell at flat {}{}.",
            rng.random_range(1..=120),
            char::from(b'A' + rng.random_range(0..4u8))
        ),
        5 => format!("Back entrance, floor {}.", rng.random_range(1..=12)),
        6 => format!(
            "Replacement for order #{}.",
            rng.random_range(100_000..10_000_000)
        ),
        7 => format!("Customer asked to exchange for size {}.", pick(rng, SIZES)),
        8 => format!("Fragile: {} glass items inside.", rng.random_range(2..=8)),
        9 => format!(
            "Gate code {:04}, then leave it {}.",
            rng.random_range(0..10_000),
            pick(rng, NOTE_PLACES)
        ),
        10 => format!(
            "Customer prefers {} notifications.",
            pick(rng, NOTE_CHANNELS)
        ),
        11 => format!(
            "Second delivery attempt; the first was on {}.",
            pick(rng, WEEKDAYS)
        ),
        _ => {
            let boxes = rng.random_range(2..=6);
            format!(
                "Box {} of {boxes}, keep them together.",
                rng.random_range(1..=boxes)
            )
        }
    };
    match rng.random_range(0..10) {
        0..=2 => note.push_str(&format!(" Thanks, {}!", pick(rng, FIRST_NAMES))),
        3..=4 => note.push_str(&format!(" Ref. {:05}.", rng.random_range(0..100_000))),
        5 => note.push_str(&format!(
            " Phone ending {:04}.",
            rng.random_range(0..10_000)
        )),
        _ => {}
    }
    note
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
    use std::collections::{HashMap, HashSet};

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
        let generate = |workers: usize| -> (Vec<Order>, Vec<OrderLine>) {
            let queue = plan.queue();
            let (mut orders, mut lines): (Vec<Order>, Vec<OrderLine>) =
                std::thread::scope(|scope| {
                    let handles: Vec<_> = (0..workers)
                        .map(|_| {
                            scope.spawn(|| {
                                let (mut orders, mut lines) = (Vec::new(), Vec::new());
                                while let Some(chunk) = queue.next() {
                                    for order in generator.orders(chunk) {
                                        generator.push_lines(&order, &mut lines);
                                        orders.push(order);
                                    }
                                }
                                (orders, lines)
                            })
                        })
                        .collect();
                    handles.into_iter().map(|h| h.join().unwrap()).fold(
                        (Vec::new(), Vec::new()),
                        |(mut orders, mut lines), (o, l)| {
                            orders.extend(o);
                            lines.extend(l);
                            (orders, lines)
                        },
                    )
                });
            orders.sort_by_key(|order| order.id);
            lines.sort_by_key(|line| line.id);
            (orders, lines)
        };
        let sequential = generate(1);
        assert_eq!(sequential.0.len(), 5_000);
        assert!(
            sequential
                .0
                .iter()
                .zip(1..)
                .all(|(order, id)| order.id == id)
        );
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
    fn names_and_notes_have_high_cardinality() {
        let generator = Generator::new(5, 1_000_000);
        let customers: Vec<Customer> = generator.customers(chunk(1, 20_000)).collect();
        let names: HashSet<&str> = customers.iter().map(|c| c.name.as_str()).collect();
        assert!(
            names.len() > 19_000,
            "{} distinct names for 20000 customers",
            names.len()
        );
        assert!(customers[0].name.split(' ').nth(1).unwrap().ends_with('.'));

        let notes: Vec<String> = generator
            .orders(chunk(1, 30_000))
            .filter_map(|order| order.notes)
            .collect();
        let distinct: HashSet<&String> = notes.iter().collect();
        assert!(
            distinct.len() * 2 > notes.len(),
            "{} distinct of {} notes",
            distinct.len(),
            notes.len()
        );
    }

    #[test]
    fn customers_are_consistent_across_orders_and_live_in_the_order_city() {
        let generator = Generator::new(3, 10_000);
        let cities: HashMap<i32, CityRow> = generator
            .cities()
            .into_iter()
            .map(|city| (city.id, city))
            .collect();
        let mut seen = HashMap::new();
        for order in generator.orders(chunk(1, 5_000)) {
            let customer = generator.customer(order.customer_id as u64);
            assert_eq!(customer.name, order.customer_name);
            assert_eq!(customer.email, order.email);
            let city = &cities[&customer.city_id];
            assert_eq!(
                (city.city.name, city.country_code),
                (order.city, order.country)
            );
            let identity = (order.customer_name.clone(), order.email.clone(), order.city);
            let previous = seen
                .entry(order.customer_id)
                .or_insert_with(|| identity.clone());
            assert_eq!(*previous, identity);
            assert!(customer.signup_date.0 < WINDOW_START.date().0);
        }
        let segments: HashSet<&str> = generator
            .customers(chunk(1, 2_000))
            .map(|c| c.segment)
            .collect();
        assert_eq!(segments.len(), 3);
    }

    #[test]
    fn line_one_is_the_order_and_the_others_are_new_products() {
        let generator = Generator::new(9, 20_000);
        let mut lines = Vec::new();
        let orders: Vec<Order> = generator.orders(chunk(1, 20_000)).collect();
        for order in &orders {
            let start = lines.len();
            generator.push_lines(order, &mut lines);
            let mine = &lines[start..];
            assert!((1..=5).contains(&mine.len()));
            let first = &mine[0];
            assert_eq!(
                (
                    first.product_id,
                    first.quantity,
                    first.unit_price,
                    first.discount
                ),
                (
                    order.product_id,
                    order.quantity,
                    order.unit_price,
                    order.discount
                )
            );
            for (line, line_no) in mine.iter().zip(1..) {
                assert_eq!(line.line_no, line_no);
                assert_eq!(line.id, order.id * 10 + i64::from(line_no));
                assert_eq!(line.order_id, order.id);
            }
            let products: HashSet<i32> = mine.iter().map(|l| l.product_id).collect();
            assert_eq!(products.len(), mine.len(), "each product once per order");
        }
        let average = lines.len() as f64 / orders.len() as f64;
        assert!((2.4..2.6).contains(&average), "{average} lines per order");

        // Lines depend on the order alone, not on the chunk that made it.
        let again: Vec<Order> = generator.orders(chunk(1, 20_000)).collect();
        let mut repeated = Vec::new();
        generator.push_lines(&again[777], &mut repeated);
        let original: Vec<&OrderLine> = lines.iter().filter(|l| l.order_id == 778).collect();
        assert_eq!(repeated.iter().collect::<Vec<_>>(), original);
    }

    #[test]
    fn dimensions_number_the_reference_data_in_list_order() {
        let generator = Generator::new(1, 1000);
        let products = generator.products();
        let names: HashSet<&str> = products.iter().map(|p| p.product.name).collect();
        assert_eq!(names.len(), products.len(), "product names are unique");
        assert!(products.iter().zip(1..).all(|(p, id)| p.id == id));
        let (_, category, product) = generator.product(&mut stream_rng(1, 0, 0));
        let row = products
            .iter()
            .find(|p| p.product.name == product.name)
            .unwrap();
        assert_eq!(CATEGORIES[row.category_id as usize - 1].name, category.name);
        let cities = generator.cities();
        assert_eq!(
            cities.len(),
            COUNTRIES.iter().map(|c| c.cities.len()).sum::<usize>()
        );
        assert!(cities.iter().zip(1..).all(|(c, id)| c.id == id));
        assert_eq!(generator.categories().len(), CATEGORIES.len());
        assert_eq!(generator.countries().len(), COUNTRIES.len());
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
        let codes: HashSet<&str> = COUNTRIES.iter().map(|c| c.code).collect();
        assert_eq!(codes.len(), COUNTRIES.len());
        for country in COUNTRIES {
            assert!((4..=10).contains(&country.cities.len()), "{}", country.code);
            assert!((0.0..0.3).contains(&country.vat_rate), "{}", country.code);
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
        let unique_first: HashSet<&str> = FIRST_NAMES.iter().copied().collect();
        let unique_last: HashSet<&str> = LAST_NAMES.iter().copied().collect();
        assert_eq!(unique_first.len(), FIRST_NAMES.len());
        assert_eq!(unique_last.len(), LAST_NAMES.len());
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
    fn calendar_constants_are_right() {
        let mut buf = [0; crate::pg_time::DATE_LEN];
        let mut day = |date: Date| date.write_iso(&mut buf).unwrap().to_owned();
        assert_eq!(day(WINDOW_START.date()), "2024-01-01");
        assert_eq!(day(SIGNUP_START), "2016-01-01");
        assert_eq!(day(Date(SIGNUP_START.0 + SIGNUP_DAYS - 1)), "2023-12-31");
        assert_eq!(day(LAUNCH_START), "2015-01-01");
        assert_eq!(day(Date(LAUNCH_START.0 + LAUNCH_DAYS - 1)), "2023-12-31");
    }
}
