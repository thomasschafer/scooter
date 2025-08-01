use frep_core::{run, validation::SearchConfiguration};

pub fn run_headless(search_config: SearchConfiguration<'_>) -> anyhow::Result<String> {
    run::find_and_replace(search_config)
}
