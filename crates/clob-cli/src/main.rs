//! Devnet operations for a ClobDex market.

mod config;
mod rpc;
mod spl;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::config::Config;
use crate::rpc::Client;

#[derive(Parser)]
#[command(name = "clob", about = "Devnet operations for a ClobDex market", version)]
struct Cli {
    /// Signer. Defaults to the Solana CLI's keypair.
    #[arg(long, global = true)]
    keypair: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the payer and its SOL balance.
    Balance,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new(Config::load(cli.keypair.as_deref())?);

    match cli.command {
        Command::Balance => {
            println!("payer   {}", client.payer_key());
            println!("balance {:.4} SOL", client.balance()? as f64 / 1e9);
            Ok(())
        }
    }
}
