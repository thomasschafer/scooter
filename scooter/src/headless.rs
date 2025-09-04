use frep_core::{
    run,
    validation::{DirConfig, SearchConfig},
};

pub fn run_headless(
    search_config: SearchConfig<'_>,
    dir_config: DirConfig<'_>,
) -> anyhow::Result<String> {
    run::find_and_replace(search_config, dir_config)
}

pub fn run_headless_with_stdin(
    search_config: SearchConfig<'_>,
    stdin_content: &str,
) -> anyhow::Result<String> {
    run::find_and_replace_text(stdin_content, search_config)
}
