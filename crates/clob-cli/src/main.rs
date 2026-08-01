//! Devnet operations for a ClobDex market.
//!
//! Exists so the off-chain half of this project can be tested against a real cluster:
//! the indexer needs a live market producing real trades before anything it derives can
//! be believed. Every instruction is built through `clob-client` rather than assembled
//! here, so using this tool also exercises the SDK.

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

    /// Act as a wallet created by `new-trader`, rather than as the payer.
    #[arg(long, global = true)]
    trader: Option<String>,

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
    /// Create a second wallet, funded with SOL and tokens.
    NewTrader {
        /// Name to refer to it by, with `--trader`.
        name: String,
        /// Whole token units to mint to it, of each side.
        #[arg(long, default_value_t = 100_000)]
        units: u64,
    },
    /// Cancel every resting order on one side.
    CancelAll {
        /// Which side to clear.
        side: OrderSide,
        /// Maximum orders to cancel.
        #[arg(long, default_value_t = 64)]
        limit: u32,
    },
    /// Print the signer and its SOL balance.
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
    let cluster = &cli.cluster;
    let as_trader = cli.trader.as_deref();

    // `--trader` names a wallet this CLI created; it signs, and it pays its own fees.
    // `new-trader` itself is the exception: it has to be signed by the payer, which
    // holds the mint authority and the SOL.
    let keypair = match (as_trader, &cli.command) {
        (Some(name), command) if !matches!(command, Command::NewTrader { .. }) => {
            Some(store::TraderRecord::keypair_path(cluster, name))
        }
        _ => cli.keypair.clone(),
    };
    let client = Client::new(Config::load(keypair.as_deref())?);

    match cli.command {
        Command::CreateMarket { fee_bps } => commands::market::create(&client, cluster, fee_bps),
        Command::Show { depth } => commands::market::show(&client, cluster, depth),
        Command::NewTrader { name, units } => {
            commands::trader::create(&client, cluster, &name, units)
        }
        Command::Fund { base, quote } => {
            commands::trade::fund(&client, cluster, as_trader, base, quote)
        }
        Command::Order {
            side,
            price,
            size,
            receipt,
        } => commands::trade::place(&client, cluster, side.into(), price, size, receipt),
        Command::Swap {
            side,
            price,
            size,
            receipt,
        } => commands::trade::swap(&client, cluster, as_trader, side.into(), price, size, receipt),
        Command::CancelAll { side, limit } => {
            commands::trade::cancel_all(&client, cluster, side.into(), limit)
        }
        Command::Balance => {
            println!("signer  {}", client.payer_key());
            println!("balance {:.4} SOL", client.balance()? as f64 / 1e9);
            Ok(())
        }
    }
}
