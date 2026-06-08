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
    command: Option<Command>,

    /// Legacy positional shell argument. Implies `comp`.
    #[arg(value_enum, hide = true)]
    legacy_shell: Option<CompletionShell>,

    /// Legacy output directory. Used by both `comp` (default: "comp") and `man` (default: "man").
    #[arg(long, hide = true)]
    out_dir: Option<String>,
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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    env::set_current_dir(project_root())?;

    match cli.command {
        Some(Command::Comp { out_dir, shell }) => {
            comp::generate(&out_dir, shell).map_err(std::convert::Into::into)
        }
        Some(Command::Man { out_dir }) => {
            man::generate(&out_dir).map_err(std::convert::Into::into)
        }
        None => {
            // Preserve backwards-compatible behaviour: no subcommand falls
            // through to `comp` so existing invocations keep working.
            let out_dir = cli.out_dir.unwrap_or_else(|| "comp".to_string());
            comp::generate(&out_dir, cli.legacy_shell).map_err(std::convert::Into::into)
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
