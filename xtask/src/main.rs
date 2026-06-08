use std::{
    env,
    path::{Path, PathBuf},
};

use clap::Parser;

mod comp;

use comp::CompletionShell;

#[derive(Parser)]
struct Cli {
    #[arg(long, default_value = "comp")]
    out_dir: String,
    /// Shell to generate completions for (generates all if not specified)
    #[arg(value_enum)]
    shell: Option<CompletionShell>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli { out_dir, shell } = Cli::parse();

    env::set_current_dir(project_root())?;

    comp::generate(&out_dir, shell).map_err(std::convert::Into::into)
}

fn project_root() -> PathBuf {
    Path::new(
        &env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_owned()),
    )
    .ancestors()
    .nth(1)
    .unwrap()
    .to_path_buf()
}
