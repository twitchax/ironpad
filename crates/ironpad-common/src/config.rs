use std::path::PathBuf;

/// Application configuration, derived from CLI arguments and environment variables.
///
/// Provided via Leptos context on the server side so that `#[server]` functions
/// can access it with `expect_context::<AppConfig>()`.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub port: u16,
    pub ironpad_cell_path: PathBuf,
}
