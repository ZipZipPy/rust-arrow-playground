//! Small in-memory LRU cache for fetched rows.
//!
//! It lets `/api/data?cache=true` measure encoding without the database.
//! Rows are large (roughly 0.7 KB each with all columns), so the cache is
//! bounded both by entry count and by the total number of cached rows.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use crate::db::FetchedRows;
use crate::query::RowsQuery;

pub type RowCache = BoundedCache<RowsQuery, FetchedRows>;

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
    /// both bounds. A value heavier than the whole budget is not stored.
    pub fn insert(&self, key: K, value: Arc<V>, weight: usize) {
        if self.max_entries == 0 || weight > self.max_weight {
            return;
        }
        let mut state = self.lock();
        if let Some(position) = state.entries.iter().position(|entry| entry.key == key) {
            let old = state
                .entries
                .remove(position)
                .expect("position is in range");
            state.weight -= old.weight;
        }
        while state.entries.len() >= self.max_entries || state.weight + weight > self.max_weight {
            let evicted = state
                .entries
                .pop_front()
                .expect("over budget implies entries");
            state.weight -= evicted.weight;
        }
        state.weight += weight;
        state.entries.push_back(Entry { key, value, weight });
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

    fn lock(&self) -> std::sync::MutexGuard<'_, State<K, V>> {
        // The state is consistent after every statement, so a panic while
        // holding the lock cannot leave it half-updated.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache(entries: usize, weight: usize) -> BoundedCache<u32, &'static str> {
        BoundedCache::new(entries, weight)
    }

    #[test]
    fn returns_what_was_stored() {
        let cache = cache(2, 100);
        assert!(cache.get(&1).is_none());
        cache.insert(1, Arc::new("one"), 10);
        assert_eq!(*cache.get(&1).unwrap(), "one");
        assert_eq!(cache.weight(), 10);
    }

    #[test]
    fn evicts_least_recently_used_by_entry_count() {
        let cache = cache(2, 100);
        cache.insert(1, Arc::new("one"), 1);
        cache.insert(2, Arc::new("two"), 1);
        cache.get(&1); // 2 is now the least recently used
        cache.insert(3, Arc::new("three"), 1);
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&1).is_some());
        assert!(cache.get(&3).is_some());
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn evicts_until_the_weight_fits() {
        let cache = cache(8, 100);
        cache.insert(1, Arc::new("one"), 40);
        cache.insert(2, Arc::new("two"), 40);
        cache.insert(3, Arc::new("three"), 50);
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_some());
        assert_eq!(cache.weight(), 90);
        cache.insert(4, Arc::new("four"), 95);
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_none());
        assert!(cache.get(&4).is_some());
        assert_eq!(cache.weight(), 95);
    }

    #[test]
    fn skips_values_heavier_than_the_budget() {
        let cache = cache(8, 100);
        cache.insert(1, Arc::new("one"), 10);
        cache.insert(2, Arc::new("huge"), 101);
        assert!(cache.get(&2).is_none());
        assert!(
            cache.get(&1).is_some(),
            "an oversized value must not evict others"
        );
    }

    #[test]
    fn replacing_a_key_releases_its_weight() {
        let cache = cache(8, 100);
        cache.insert(1, Arc::new("old"), 60);
        cache.insert(1, Arc::new("new"), 70);
        assert_eq!(*cache.get(&1).unwrap(), "new");
        assert_eq!(cache.weight(), 70);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn zero_entries_disables_the_cache() {
        let cache = cache(0, 100);
        cache.insert(1, Arc::new("one"), 1);
        assert!(cache.is_empty());
    }
}
