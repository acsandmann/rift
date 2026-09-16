//! Compatibility entry point for scripts that still invoke `rift-cli`.

use std::process;

use clap::{Parser, Subcommand};
use rift_wm::cli::{self, ClientCommand};
use rift_wm::sys::service::{ServiceCommands, handle_service_command};

#[derive(Parser)]
#[command(name = "rift-cli", version = env!("RIFT_VERSION"))]
#[command(about = "Deprecated command-line interface for rift")]
#[command(
    long_about = "Deprecated command-line interface for rift.\n\nUse `rift` instead; it accepts the same subcommands. `rift-cli` remains available for compatibility with existing scripts."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage the launchd service for rift
    Service {
        #[command(subcommand)]
        service: ServiceCommands,
    },
    #[command(flatten)]
    Client(ClientCommand),
}

fn main() {
    sigpipe::reset();
    eprintln!("warning: `rift-cli` is deprecated; use `rift` instead");

    match Cli::parse().command {
        Commands::Service { service } => match handle_service_command(&service) {
            Ok(message) => println!("{message}"),
            Err(error) => {
                eprintln!("rift-cli: {error}");
                process::exit(1);
            }
        },
        Commands::Client(command) => cli::run(command),
    }
}
