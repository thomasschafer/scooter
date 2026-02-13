use anyhow::{Context, Result};
use lru::LruCache;
use std::{
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

/// Provides file contents for features that need full-text context.
pub trait FileContentProvider: Send + Sync {
    fn read_to_string(&self, path: &Path) -> Result<Arc<String>>;

    fn invalidate(&self, _path: &Path) {}

    fn clear(&self) {}
}

const DEFAULT_FILE_CACHE_CAPACITY: usize = 200;

type FileContentCache = Mutex<LruCache<PathBuf, Arc<String>>>;

fn file_content_cache() -> Arc<FileContentCache> {
    static FILE_CONTENT_CACHE: OnceLock<Arc<FileContentCache>> = OnceLock::new();
    FILE_CONTENT_CACHE
        .get_or_init(|| {
            let cache_capacity = NonZeroUsize::new(DEFAULT_FILE_CACHE_CAPACITY)
                .expect("File content cache capacity must be non-zero");
            Arc::new(Mutex::new(LruCache::new(cache_capacity)))
        })
        .clone()
}

#[derive(Clone)]
struct CachedFileContentProvider {
    cache: Arc<FileContentCache>,
}

impl CachedFileContentProvider {
    fn new(cache: Arc<FileContentCache>) -> Self {
        Self { cache }
    }
}

impl FileContentProvider for CachedFileContentProvider {
    fn read_to_string(&self, path: &Path) -> Result<Arc<String>> {
        let mut cache_guard = self.cache.lock().unwrap();
        if let Some(cached) = cache_guard.get(path) {
            return Ok(Arc::clone(cached));
        }
        drop(cache_guard);

        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read file contents: {}", path.display()))?;
        let contents = Arc::new(contents);

        let mut cache_guard = self.cache.lock().unwrap();
        cache_guard.put(path.to_path_buf(), Arc::clone(&contents));
        Ok(contents)
    }

    fn invalidate(&self, path: &Path) {
        let mut cache_guard = self.cache.lock().unwrap();
        cache_guard.pop(path);
    }

    fn clear(&self) {
        let mut cache_guard = self.cache.lock().unwrap();
        cache_guard.clear();
    }
}

pub fn default_file_content_provider() -> Arc<dyn FileContentProvider> {
    Arc::new(CachedFileContentProvider::new(file_content_cache()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, Write};
    use tempfile::NamedTempFile;

    #[test]
    fn test_invalidate_cache_entry() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "first").unwrap();

        let cache_capacity = NonZeroUsize::new(4).unwrap();
        let cache = Arc::new(Mutex::new(LruCache::new(cache_capacity)));
        let provider = CachedFileContentProvider::new(cache);
        let first = provider.read_to_string(file.path()).unwrap();
        assert_eq!(first.as_str(), "first");

        file.as_file_mut().set_len(0).unwrap();
        file.as_file_mut().rewind().unwrap();
        write!(file, "second").unwrap();

        let cached = provider.read_to_string(file.path()).unwrap();
        assert_eq!(cached.as_str(), "first");

        provider.invalidate(file.path());
        let refreshed = provider.read_to_string(file.path()).unwrap();
        assert_eq!(refreshed.as_str(), "second");
    }

    #[test]
    fn test_clear_cache() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "first").unwrap();

        let cache_capacity = NonZeroUsize::new(4).unwrap();
        let cache = Arc::new(Mutex::new(LruCache::new(cache_capacity)));
        let provider = CachedFileContentProvider::new(cache);
        let first = provider.read_to_string(file.path()).unwrap();
        assert_eq!(first.as_str(), "first");

        file.as_file_mut().set_len(0).unwrap();
        file.as_file_mut().rewind().unwrap();
        write!(file, "second").unwrap();

        provider.clear();
        let refreshed = provider.read_to_string(file.path()).unwrap();
        assert_eq!(refreshed.as_str(), "second");
    }
}
