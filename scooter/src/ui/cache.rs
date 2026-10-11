use lru::LruCache;
use scooter_core::{file_content::FileStamp, search::MatchContent, utils::HighlightedLine};
use std::{
    borrow::Borrow,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use crate::ui::view::SearchResultPreview;

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct FileWindow {
    pub(crate) path: PathBuf,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

macro_rules! define_cache {
    (
        $(#[$meta:meta])*
        $fn_name:ident: $key:ty => $value:ty
    ) => {
        paste::paste! {
            type [<$fn_name:camel Cache>] = Mutex<LruCache<$key, $value>>;

            static [<$fn_name:upper _CACHE>]: OnceLock<[<$fn_name:camel Cache>]> = OnceLock::new();

            $(#[$meta])*
            pub(crate) fn $fn_name() -> &'static [<$fn_name:camel Cache>] {
                [<$fn_name:upper _CACHE>].get_or_init(|| {
                    let cache_capacity = NonZeroUsize::new(200).unwrap();
                    Mutex::new(LruCache::new(cache_capacity))
                })
            }
        }
    };
}

/// Data derived from the contents of a file, along with the stamp of the file taken before it was
/// read
pub(crate) struct FileDerived<T> {
    stamp: FileStamp,
    value: T,
}

type FileDerivedCache<K, T> = Mutex<LruCache<K, FileDerived<T>>>;

/// Applies `f` to the value cached for `key`, unless the file it was derived from has changed since
/// (as determined by comparing with `current_stamp`), in which case the stale value is evicted
pub(crate) fn get_if_unchanged<K, Q, T, R>(
    cache: &FileDerivedCache<K, T>,
    key: &Q,
    current_stamp: FileStamp,
    f: impl FnOnce(&T) -> R,
) -> Option<R>
where
    K: Hash + Eq + Borrow<Q>,
    Q: Hash + Eq + ?Sized,
{
    let mut cache_guard = cache.lock().unwrap();
    let cached = cache_guard.get(key)?;
    if cached.stamp == current_stamp {
        return Some(f(&cached.value));
    }
    cache_guard.pop(key);
    None
}

/// Caches `value`, which must have been derived from the file contents described by `stamp`
pub(crate) fn put_file_derived<K: Hash + Eq, T>(
    cache: &FileDerivedCache<K, T>,
    key: K,
    stamp: FileStamp,
    value: T,
) {
    cache.lock().unwrap().put(key, FileDerived { stamp, value });
}

define_cache! {
    /// Cache of sections of files (plain text)
    plain_window_cache: FileWindow => FileDerived<Vec<(usize, String)>>
}

define_cache! {
    /// Cache of sections of files (with syntax highlighting)
    highlighted_window_cache: FileWindow => FileDerived<Vec<(usize, HighlightedLine)>>
}

define_cache! {
    /// Cache of entire files (with syntax highlighting)
    highlighted_file_cache: PathBuf => FileDerived<Vec<(usize, HighlightedLine)>>
}

define_cache! {
    /// Cache of computed line diff tasks, keyed by hash with collision buckets.
    diff_cache: u64 => Vec<DiffCacheRecord>
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiffCacheFullKey {
    pub(crate) old: MatchContent,
    pub(crate) replacement: String,
}

impl DiffCacheFullKey {
    pub(crate) fn new(old: MatchContent, replacement: String) -> Self {
        Self { old, replacement }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiffTaskCacheEntry {
    InProgress { simple_preview: SearchResultPreview },
    Ready { preview: SearchResultPreview },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DiffCacheRecord {
    pub(crate) full_key: DiffCacheFullKey,
    pub(crate) entry: DiffTaskCacheEntry,
}

pub(crate) fn diff_cache_hash(old: &MatchContent, replacement: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    old.hash(&mut hasher);
    replacement.hash(&mut hasher);
    hasher.finish()
}
