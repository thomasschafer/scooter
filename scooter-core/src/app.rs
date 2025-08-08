use std::{
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
};

use frep_core::search::SearchResult;

use crate::replace::ReplaceState;

#[derive(Debug, PartialEq, Eq)]
pub enum EventHandlingResult {
    Rerender,
    Exit(Option<ReplaceState>),
    None,
}

#[derive(Debug)]
pub enum BackgroundProcessingEvent {
    AddSearchResults(Vec<SearchResult>),
    SearchCompleted,
    ReplacementCompleted(ReplaceState),
    UpdateReplacements {
        start: usize,
        end: usize,
        cancelled: Arc<AtomicBool>,
    },
    UpdateAllReplacements {
        cancelled: Arc<AtomicBool>,
    },
}

#[derive(Debug)]
pub enum AppEvent {
    Rerender,
    PerformSearch,
}

#[derive(Debug)]
pub enum Event {
    LaunchEditor((PathBuf, usize)),
    App(AppEvent),
    PerformReplacement,
}
