//! Devnet operations for a ClobDex market.

mod commands;
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
    /// Which saved market to act on. Names the file under `.clob/`.
    #[arg(long, default_value = "devnet", global = true)]
    cluster: String,

    /// Signer. Defaults to the Solana CLI's keypair.
    #[arg(long, global = true)]
    keypair: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create two mints and a market over them.
    CreateMarket {
        /// Taker fee in basis points.
        #[arg(long, default_value_t = 2)]
        fee_bps: u64,
    },
    /// Print the book.
    Show {
        /// Price levels per side.
        #[arg(long, default_value_t = 10)]
        depth: usize,
    },
    /// Print the payer and its SOL balance.
    Balance,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new(Config::load(cli.keypair.as_deref())?);

    match cli.command {
        Command::CreateMarket { fee_bps } => commands::market::create(&client, &cli.cluster, fee_bps),
        Command::Show { depth } => commands::market::show(&client, &cli.cluster, depth),
        Command::Balance => {
            println!("payer   {}", client.payer_key());
            println!("balance {:.4} SOL", client.balance()? as f64 / 1e9);
            Ok(())
        }
    }
}
