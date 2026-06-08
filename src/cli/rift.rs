use crate::sys::service::ServiceCommands;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
pub struct Cli {
    /// Only run the window manager on the current space.
    #[arg(long)]
    pub one: bool,

    /// Disable new spaces by default.
    ///
    /// Ignored if --one is used.
    #[arg(long)]
    pub default_disable: bool,

    /// Disable animations.
    #[arg(long)]
    pub no_animate: bool,

    /// No-op compatibility check for the deprecated restore file path.
    #[arg(long)]
    pub validate: bool,

    /// Deprecated no-op flag retained for CLI compatibility.
    #[arg(long)]
    pub restore: bool,

    /// Record reactor events to the specified file path. Overwrites the file if
    /// exists.
    #[arg(long)]
    pub record: Option<PathBuf>,

    /// Path to configuration file to use (overrides default).
    #[arg(long, value_name = "PATH")]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage the launchd service for rift
    Service {
        #[command(subcommand)]
        service: ServiceCommands,
    },
}
