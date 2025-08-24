use frep_core::{run, validation::SearchConfiguration};

pub fn run_headless(search_config: SearchConfiguration<'_>) -> anyhow::Result<String> {
    run::find_and_replace(search_config)
}

pub fn run_headless_with_stdin(
    search_config: SearchConfiguration<'_>,
    stdin_content: &str,
) -> anyhow::Result<String> {
    run::find_and_replace_text(stdin_content, search_config)
}
