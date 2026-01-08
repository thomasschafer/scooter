use lru::LruCache;
use scooter_core::{search::MatchContent, utils::HighlightedLine};
use std::{
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

define_cache! {
    /// Cache of sections of files (plain text)
    plain_window_cache: FileWindow => Vec<(usize, String)>
}

define_cache! {
    /// Cache of sections of files (with syntax highlighting)
    highlighted_window_cache: FileWindow => Vec<(usize, HighlightedLine)>
}

define_cache! {
    /// Cache of entire files (with syntax highlighting)
    highlighted_file_cache: PathBuf => Vec<(usize, HighlightedLine)>
}

define_cache! {
    /// Cache of line diffs
    diff_cache: (MatchContent, String) => SearchResultPreview
}
