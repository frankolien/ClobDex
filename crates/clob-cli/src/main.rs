//! Devnet operations for a ClobDex market.

mod commands;
mod config;
mod rpc;
mod spl;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use clob_book::Side;

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
    /// Claim a seat and deposit funds.
    Fund {
        /// Base lots to deposit.
        #[arg(long, default_value_t = 100_000)]
        base: u64,
        /// Quote lots to deposit.
        #[arg(long, default_value_t = 100_000_000)]
        quote: u64,
    },
    /// Place a limit order.
    Order {
        /// Which side.
        side: OrderSide,
        /// Price in ticks.
        price: u64,
        /// Size in base lots.
        size: u64,
        /// Emit an event receipt.
        #[arg(long)]
        receipt: bool,
    },
    /// Trade against the book without a seat.
    Swap {
        /// Which side the taker is on.
        side: OrderSide,
        /// Limit price in ticks.
        price: u64,
        /// Size in base lots.
        size: u64,
        /// Emit an event receipt.
        #[arg(long)]
        receipt: bool,
    },
    /// Cancel every resting order on one side.
    CancelAll {
        /// Which side to clear.
        side: OrderSide,
        /// Maximum orders to cancel.
        #[arg(long, default_value_t = 64)]
        limit: u32,
    },
    /// Print the payer and its SOL balance.
    Balance,
}

/// `Side` is `no_std` and cannot derive clap's traits, so the CLI has its own.
#[derive(Copy, Clone, Debug, ValueEnum)]
enum OrderSide {
    Bid,
    Ask,
}

impl From<OrderSide> for Side {
    fn from(side: OrderSide) -> Self {
        match side {
            OrderSide::Bid => Side::Bid,
            OrderSide::Ask => Side::Ask,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = Client::new(Config::load(cli.keypair.as_deref())?);

    match cli.command {
        Command::CreateMarket { fee_bps } => commands::market::create(&client, &cli.cluster, fee_bps),
        Command::Show { depth } => commands::market::show(&client, &cli.cluster, depth),
        Command::Fund { base, quote } => commands::trade::fund(&client, &cli.cluster, base, quote),
        Command::Order { side, price, size, receipt } => {
            commands::trade::place(&client, &cli.cluster, side.into(), price, size, receipt)
        }
        Command::Swap { side, price, size, receipt } => {
            commands::trade::swap(&client, &cli.cluster, side.into(), price, size, receipt)
        }
        Command::CancelAll { side, limit } => {
            commands::trade::cancel_all(&client, &cli.cluster, side.into(), limit)
        }
        Command::Balance => {
            println!("payer   {}", client.payer_key());
            println!("balance {:.4} SOL", client.balance()? as f64 / 1e9);
            Ok(())
        }
    }
}
