//! Hash maps built on every core: entries are split into shards by a hash
//! of their keys, and each shard's map is built by its own thread.

use std::hash::{BuildHasher, Hash};

use turbonop_hash::{Map, State};

/// A map split into shards by key hash.
pub(crate) struct Sharded<K, V> {
    maps: Vec<Map<K, V>>,
    state: State,
}

impl<K: Hash + Eq + Send + Sync, V: Send + Sync> Sharded<K, V> {
    /// The map of every entry `entries` gives for each of `items` (read
    /// in parallel), each folded into its key's value by `fold` (in item
    /// order, within a shard).
    pub(crate) fn build<'i, T: Sync, E: Send>(
        items: &'i [T],
        entries: impl Fn(usize, &'i T, &mut Vec<(K, E)>) + Sync,
        fold: impl Fn(&mut Map<K, V>, K, E) + Sync,
    ) -> Self {
        let state = State::default();
        let shards = (turbonop_par::threads() * 4).next_power_of_two();
        let indexed: Vec<(usize, &'i T)> = items.iter().enumerate().collect();
        let buckets: Vec<Vec<Vec<(K, E)>>> =
            turbonop_par::map_balanced(&indexed, 64, |&(i, item)| {
                let mut list = Vec::new();
                entries(i, item, &mut list);
                let mut b: Vec<Vec<(K, E)>> = (0..shards).map(|_| Vec::new()).collect();
                for (k, e) in list {
                    let s = shard(&state, &k, shards);
                    b[s].push((k, e));
                }
                b
            });
        let mut by_shard: Vec<Vec<Vec<(K, E)>>> = (0..shards).map(|_| Vec::new()).collect();
        for item in buckets {
            for (s, b) in item.into_iter().enumerate() {
                if !b.is_empty() {
                    by_shard[s].push(b);
                }
            }
        }
        let maps = turbonop_par::map_into(by_shard, 1, |lists| {
            let n = lists.iter().map(Vec::len).sum();
            let mut m = turbonop_hash::map_with_capacity(n);
            for list in lists {
                for (k, e) in list {
                    fold(&mut m, k, e);
                }
            }
            m
        });
        Self { maps, state }
    }

    /// The value of `key`.
    pub(crate) fn get(&self, key: &K) -> Option<&V> {
        self.maps[shard(&self.state, key, self.maps.len())].get(key)
    }

    /// Whether `key` has a value.
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    /// The shards' maps.
    pub(crate) fn shards(&self) -> &[Map<K, V>] {
        &self.maps
    }

    /// The number of keys.
    pub(crate) fn len(&self) -> usize {
        self.maps.iter().map(Map::len).sum()
    }
}

fn shard<K: Hash>(state: &State, key: &K, shards: usize) -> usize {
    // (The shard count is a power of two.)
    (state.hash_one(key) >> 40) as usize & (shards - 1)
}
