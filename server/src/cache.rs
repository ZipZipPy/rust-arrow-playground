//! Small in-memory LRU cache for fetched rows.
//!
//! It lets `/api/data?cache=true` measure encoding without the database.
//! Rows are large (roughly 0.8 KB each with all `orders` columns), so the
//! cache is bounded both by entry count and by the total number of cached
//! rows.
//!
//! Freeing a million rows takes tens of milliseconds, so methods that drop
//! entries hand them back to the caller instead of freeing them while
//! holding the lock.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::db::{FetchedRows, SeedGeneration};
use crate::query::RowsQuery;

/// LRU cache bounded by entry count and by the summed weight of its values.
/// Lookups are linear, which is fine for the handful of entries it holds.
pub struct BoundedCache<K, V> {
    max_entries: usize,
    max_weight: usize,
    state: Mutex<State<K, V>>,
}

struct State<K, V> {
    /// Least recently used first.
    entries: VecDeque<Entry<K, V>>,
    weight: usize,
}

struct Entry<K, V> {
    key: K,
    value: Arc<V>,
    weight: usize,
}

impl<K: Eq, V> BoundedCache<K, V> {
    pub fn new(max_entries: usize, max_weight: usize) -> Self {
        Self {
            max_entries,
            max_weight,
            state: Mutex::new(State {
                entries: VecDeque::new(),
                weight: 0,
            }),
        }
    }

    pub fn get(&self, key: &K) -> Option<Arc<V>> {
        let mut state = self.lock();
        let position = state.entries.iter().position(|entry| entry.key == *key)?;
        let entry = state.entries.remove(position)?;
        let value = Arc::clone(&entry.value);
        state.entries.push_back(entry);
        Some(value)
    }

    /// Stores `value`, evicting least recently used entries to stay within
    /// both bounds, and returns the values it dropped. A value heavier than
    /// the whole budget is not stored.
    #[must_use = "evicted values should be freed off the async runtime"]
    pub fn insert(&self, key: K, value: Arc<V>, weight: usize) -> Vec<Arc<V>> {
        if self.max_entries == 0 || weight > self.max_weight {
            return Vec::new();
        }
        let mut dropped = Vec::new();
        let mut state = self.lock();
        if let Some(position) = state.entries.iter().position(|entry| entry.key == key) {
            let old = state
                .entries
                .remove(position)
                .expect("position is in range");
            state.weight -= old.weight;
            dropped.push(old.value);
        }
        while state.entries.len() >= self.max_entries || state.weight + weight > self.max_weight {
            let evicted = state
                .entries
                .pop_front()
                .expect("over budget implies entries");
            state.weight -= evicted.weight;
            dropped.push(evicted.value);
        }
        state.weight += weight;
        state.entries.push_back(Entry { key, value, weight });
        dropped
    }

    /// Empties the cache and returns the values it held.
    #[must_use = "dropped values should be freed off the async runtime"]
    pub fn clear(&self) -> Vec<Arc<V>> {
        let mut state = self.lock();
        state.weight = 0;
        state.entries.drain(..).map(|entry| entry.value).collect()
    }

    pub fn len(&self) -> usize {
        self.lock().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Total weight of the cached values.
    pub fn weight(&self) -> usize {
        self.lock().weight
    }

    fn lock(&self) -> MutexGuard<'_, State<K, V>> {
        // The state is consistent after every statement, so a panic while
        // holding the lock cannot leave it half-updated.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The row cache of `/api/data` and `/api/bench`. Its entries were all read
/// under one seed generation; seeing another one (a re-seed while the app
/// runs) drops them all, so rows of dropped tables are never served.
pub struct RowCache {
    entries: BoundedCache<RowsQuery, FetchedRows>,
    generation: Mutex<Option<SeedGeneration>>,
}

impl RowCache {
    /// At most `max_entries` results and `max_rows` rows in total.
    pub fn new(max_entries: usize, max_rows: usize) -> Self {
        Self {
            entries: BoundedCache::new(max_entries, max_rows),
            generation: Mutex::new(None),
        }
    }

    /// Records the database's current seed generation; if it changed, empties
    /// the cache and returns what it held.
    #[must_use = "dropped rows should be freed off the async runtime"]
    pub fn sync_generation(&self, current: Option<SeedGeneration>) -> Vec<Arc<FetchedRows>> {
        let mut generation = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *generation == current {
            return Vec::new();
        }
        *generation = current;
        self.entries.clear()
    }

    pub fn get(&self, query: &RowsQuery) -> Option<Arc<FetchedRows>> {
        self.entries.get(query)
    }

    /// Stores rows read under `generation`, unless the cache has moved on to
    /// another one meanwhile. Returns evicted rows.
    #[must_use = "evicted rows should be freed off the async runtime"]
    pub fn insert(
        &self,
        generation: Option<SeedGeneration>,
        query: RowsQuery,
        rows: Arc<FetchedRows>,
    ) -> Vec<Arc<FetchedRows>> {
        let current = self
            .generation
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != generation {
            return Vec::new();
        }
        let weight = rows.rows.len();
        self.entries.insert(query, rows, weight)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Cache(BoundedCache<u32, &'static str>);

    impl Cache {
        /// Inserts and returns the dropped values.
        fn insert(&self, key: u32, value: &'static str, weight: usize) -> Vec<&'static str> {
            self.0
                .insert(key, Arc::new(value), weight)
                .into_iter()
                .map(|v| *v)
                .collect()
        }
    }

    impl std::ops::Deref for Cache {
        type Target = BoundedCache<u32, &'static str>;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    fn cache(entries: usize, weight: usize) -> Cache {
        Cache(BoundedCache::new(entries, weight))
    }

    #[test]
    fn returns_what_was_stored() {
        let cache = cache(2, 100);
        assert!(cache.get(&1).is_none());
        cache.insert(1, "one", 10);
        assert_eq!(*cache.get(&1).unwrap(), "one");
        assert_eq!(cache.weight(), 10);
    }

    #[test]
    fn evicts_least_recently_used_by_entry_count() {
        let cache = cache(2, 100);
        cache.insert(1, "one", 1);
        cache.insert(2, "two", 1);
        cache.get(&1); // 2 is now the least recently used
        assert_eq!(cache.insert(3, "three", 1), ["two"]);
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&1).is_some());
        assert!(cache.get(&3).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn evicts_until_the_weight_fits() {
        let cache = cache(8, 100);
        cache.insert(1, "one", 40);
        cache.insert(2, "two", 40);
        cache.insert(3, "three", 50);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
        assert_eq!(cache.weight(), 90);
        assert_eq!(cache.insert(4, "four", 95), ["three", "two"]);
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_none());
        assert!(cache.get(&4).is_some());
        assert_eq!(cache.weight(), 95);
    }

    #[test]
    fn skips_values_heavier_than_the_budget() {
        let cache = cache(8, 100);
        cache.insert(1, "one", 10);
        cache.insert(2, "huge", 101);
        assert!(cache.get(&2).is_none());
        assert!(
            cache.get(&1).is_some(),
            "an oversized value must not evict others"
        );
    }

    #[test]
    fn replacing_a_key_releases_its_weight() {
        let cache = cache(8, 100);
        cache.insert(1, "old", 60);
        assert_eq!(cache.insert(1, "new", 70), ["old"]);
        assert_eq!(*cache.get(&1).unwrap(), "new");
        assert_eq!(cache.weight(), 70);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zero_entries_disables_the_cache() {
        let cache = cache(0, 100);
        cache.insert(1, "one", 1);
        assert!(cache.is_empty());
    }

    #[test]
    fn clear_hands_back_every_value() {
        let cache = cache(8, 100);
        cache.insert(1, "one", 10);
        cache.insert(2, "two", 10);
        let cleared: Vec<&str> = cache.clear().into_iter().map(|v| *v).collect();
        assert_eq!(cleared, ["one", "two"]);
        assert!(cache.is_empty());
        assert_eq!(cache.weight(), 0);
    }
}
