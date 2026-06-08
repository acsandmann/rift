use std::{
    env,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};

mod comp;
mod man;

use comp::CompletionShell;

#[derive(Parser)]
#[command(name = "xtask", about = "Build helpers for rift")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate shell completions
    Comp {
        /// Output directory
        #[arg(long, default_value = "comp")]
        out_dir: String,
        /// Shell to generate completions for (generates all if not specified)
        #[arg(value_enum)]
        shell: Option<CompletionShell>,
    },
    /// Generate manpages (rift.1 and rift-cli.1)
    Man {
        /// Output directory
        #[arg(long, default_value = "man")]
        out_dir: String,
    },
    Dist,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let Cli { command } = Cli::parse();
    env::set_current_dir(project_root())?;

    match command {
        Command::Comp { out_dir, shell } => {
            comp::generate(&out_dir, shell).map_err(std::convert::Into::into)
        }
        Command::Man { out_dir } => man::generate(&out_dir).map_err(std::convert::Into::into),
        Command::Dist => {
            let man_handle = std::thread::spawn(|| man::generate("man"));
            let comp_handle = std::thread::spawn(|| comp::generate("comp", None));

            let man_result = man_handle.join().map_err(|_| "man thread panicked".to_string())?;
            let comp_result = comp_handle.join().map_err(|_| "comp thread panicked".to_string())?;

            match (man_result, comp_result) {
                (Ok(()), Ok(())) => Ok(()),
                (Err(e), _) | (_, Err(e)) => Err(e.into()),
            }
            // Preserve backwards-compatible behaviour: no subcommand falls
        }
    }
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
