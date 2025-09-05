use frep_core::search::{FileSearcher, ParsedSearchConfig, SearchType};

#[derive(Clone, Debug)]
pub enum Searcher {
    FileSearcher(FileSearcher),
    TextSearcher { search_config: ParsedSearchConfig },
}

impl Searcher {
    pub fn search(&self) -> &SearchType {
        match self {
            Self::FileSearcher(file_searcher) => file_searcher.search(),
            Self::TextSearcher { search_config } => &search_config.search,
        }
    }

    pub fn replace(&self) -> &str {
        match self {
            Self::FileSearcher(file_searcher) => file_searcher.replace(),
            Self::TextSearcher { search_config } => &search_config.replace,
        }
    }
}
