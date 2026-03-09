use std::path::PathBuf;

use clap::Parser;
use ironpad_common::AppConfig;

/// ironpad — Interactive Rust Notebooks
#[derive(Parser, Debug)]
#[command(name = "ironpad", about = "Interactive Rust Notebooks")]
pub struct CliArgs {
    /// Directory for notebook data storage.
    #[arg(long, default_value = "./data", env = "IRONPAD_DATA_DIR")]
    pub data_dir: PathBuf,

    /// Directory for compilation cache.
    #[arg(long, default_value = "./cache", env = "IRONPAD_CACHE_DIR")]
    pub cache_dir: PathBuf,

    /// Port to serve the application on.
    #[arg(long, default_value_t = 3111, env = "IRONPAD_PORT")]
    pub port: u16,

    /// Path to the ironpad-cell crate (injected into user cells as a path dependency).
    #[arg(
        long,
        default_value = "./crates/ironpad-cell",
        env = "IRONPAD_CELL_PATH"
    )]
    pub ironpad_cell_path: PathBuf,
}

impl From<CliArgs> for AppConfig {
    fn from(args: CliArgs) -> Self {
        Self {
            data_dir: args.data_dir,
            cache_dir: args.cache_dir,
            port: args.port,
            ironpad_cell_path: args.ironpad_cell_path,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let args = CliArgs::parse_from(["ironpad"]);

        assert_eq!(args.data_dir, PathBuf::from("./data"));
        assert_eq!(args.cache_dir, PathBuf::from("./cache"));
        assert_eq!(args.port, 3111);
        assert_eq!(
            args.ironpad_cell_path,
            PathBuf::from("./crates/ironpad-cell")
        );
    }

    #[test]
    fn cli_args_override() {
        let args = CliArgs::parse_from([
            "ironpad",
            "--data-dir",
            "/tmp/ironpad-data",
            "--cache-dir",
            "/tmp/ironpad-cache",
            "--port",
            "8080",
            "--ironpad-cell-path",
            "/opt/ironpad-cell",
        ]);

        assert_eq!(args.data_dir, PathBuf::from("/tmp/ironpad-data"));
        assert_eq!(args.cache_dir, PathBuf::from("/tmp/ironpad-cache"));
        assert_eq!(args.port, 8080);
        assert_eq!(args.ironpad_cell_path, PathBuf::from("/opt/ironpad-cell"));
    }

    #[test]
    fn conversion_to_app_config() {
        let args = CliArgs::parse_from(["ironpad", "--data-dir", "/data", "--port", "9090"]);
        let config: AppConfig = args.into();

        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.cache_dir, PathBuf::from("./cache"));
        assert_eq!(config.port, 9090);
        assert_eq!(
            config.ironpad_cell_path,
            PathBuf::from("./crates/ironpad-cell")
        );
    }
}
