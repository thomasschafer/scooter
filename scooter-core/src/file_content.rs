use anyhow::{Context, Result};
use lru::LruCache;
use std::{
    fs::{self, Metadata},
    io,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

/// Identifies a version of a file's contents using its metadata, so that data derived from a file
/// can be cached and discarded once the file changes.
///
/// To avoid caching data derived from a newer version of the file than the stamp describes, take
/// the stamp before reading the file.
///
/// A modification that leaves the file's metadata unchanged isn't detected, so code that modifies
/// files should also invalidate any caches explicitly where possible.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    len: u64,
    modified: SystemTime,
    // Files are replaced by renaming a new file over them, which gives them a new inode, so this
    // detects replacements made by scooter even if the length and modification time are unchanged
    #[cfg(unix)]
    inode: u64,
}

impl FileStamp {
    pub fn of(path: &Path) -> io::Result<Self> {
        Self::from_metadata(&fs::metadata(path)?)
    }

    pub fn from_metadata(metadata: &Metadata) -> io::Result<Self> {
        Ok(Self {
            len: metadata.len(),
            modified: metadata.modified()?,
            #[cfg(unix)]
            inode: std::os::unix::fs::MetadataExt::ino(metadata),
        })
    }
}

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

        let contents = crate::encoding::read_text(path)
            .with_context(|| format!("Failed to read file contents: {}", path.display()))?;
        let contents = Arc::new(contents.into_text());

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

    #[test]
    fn test_file_stamp_unchanged_if_file_unchanged() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "contents").unwrap();

        let stamp = FileStamp::of(file.path()).unwrap();
        std::fs::read(file.path()).unwrap();
        assert_eq!(FileStamp::of(file.path()).unwrap(), stamp);
    }

    #[test]
    fn test_file_stamp_changes_when_length_changes() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "contents").unwrap();
        let modified = file.as_file().metadata().unwrap().modified().unwrap();

        let stamp = FileStamp::of(file.path()).unwrap();
        write!(file, " and more").unwrap();
        file.as_file().set_modified(modified).unwrap();
        assert_ne!(FileStamp::of(file.path()).unwrap(), stamp);
    }

    #[test]
    fn test_file_stamp_changes_when_modification_time_changes() {
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "contents").unwrap();
        let modified = file.as_file().metadata().unwrap().modified().unwrap();

        let stamp = FileStamp::of(file.path()).unwrap();
        file.as_file()
            .set_modified(modified + std::time::Duration::from_secs(1))
            .unwrap();
        assert_ne!(FileStamp::of(file.path()).unwrap(), stamp);
    }

    #[cfg(unix)]
    #[test]
    fn test_file_stamp_changes_when_file_replaced_with_same_length_and_modification_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.txt");
        std::fs::write(&path, "foo").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let stamp = FileStamp::of(&path).unwrap();

        let mut replacement = NamedTempFile::new_in(dir.path()).unwrap();
        write!(replacement, "bar").unwrap();
        replacement.as_file().set_modified(modified).unwrap();
        replacement.persist(&path).unwrap();

        let new_metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(new_metadata.len(), 3);
        assert_eq!(new_metadata.modified().unwrap(), modified);
        assert_ne!(FileStamp::of(&path).unwrap(), stamp);
    }
}
