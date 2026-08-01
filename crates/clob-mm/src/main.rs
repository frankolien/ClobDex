//! A reference market maker for a ClobDex market.
//!
//! Reads the market, decides what it wants resting, and sends the difference as one
//! `BatchUpdate`. Every cycle, until it is stopped — at which point it cancels its
//! ladder, because quotes outlive the process that placed them.
//!
//! The strategy is the [`clob_mm`] library and is pure. This file is the arguments, the
//! loop, and the signal handler.

mod session;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use clob_mm::Params;
use clob_mm::plan::{Plan, Reason};
use clob_ops::record::{MarketRecord, TraderRecord};
use clob_ops::{Client, Config};

use crate::session::{Report, Session};

#[derive(Parser)]
#[command(name = "clob-mm", about = "A reference market maker for a ClobDex market", version)]
struct Cli {
    /// Which saved market to quote on. Names the file under `.clob/`.
    #[arg(long, default_value = "devnet")]
    cluster: String,

    /// Signer. Defaults to the Solana CLI's keypair.
    #[arg(long)]
    keypair: Option<std::path::PathBuf>,

    /// Quote as a wallet created by `clob new-trader`, rather than as the payer.
    ///
    /// Usually what you want: a market needs someone on the other side, and a bot that
    /// shares a seat with the wallet taking against it only ever produces self-trades.
    #[arg(long)]
    trader: Option<String>,

    /// What the asset is worth, in ticks. Overridden by a two-sided book.
    #[arg(long)]
    reference: u64,

    /// Distance from fair to the first quote on each side, in ticks.
    #[arg(long, default_value_t = 50)]
    half_spread: u64,

    /// Ticks between levels on the same side.
    #[arg(long, default_value_t = 25)]
    step: u64,

    /// Levels per side.
    #[arg(long, default_value_t = 3)]
    levels: u8,

    /// Size of each level, in base lots.
    #[arg(long, default_value_t = 100)]
    size: u64,

    /// Base lots to aim to hold.
    #[arg(long, default_value_t = 0)]
    target: u64,

    /// How far from target the position may drift before a side stops quoting.
    #[arg(long, default_value_t = 5_000)]
    inventory_limit: u64,

    /// The most inventory may shift the ladder, in ticks. Must be under the half-spread.
    #[arg(long, default_value_t = 20)]
    max_skew: u64,

    /// How far a resting quote may drift before it is worth re-quoting, in ticks.
    #[arg(long, default_value_t = 10)]
    drift: u64,

    /// Seconds between cycles.
    #[arg(long, default_value_t = 5)]
    interval: u64,

    /// Run one cycle and stop.
    #[arg(long)]
    once: bool,

    /// Decide, print, and send nothing.
    #[arg(long)]
    dry_run: bool,
}

impl Cli {
    fn params(&self) -> Params {
        Params {
            reference_in_ticks: self.reference,
            half_spread_in_ticks: self.half_spread,
            level_step_in_ticks: self.step,
            levels: self.levels,
            size_in_base_lots: self.size,
            target_base_lots: self.target,
            inventory_limit_lots: self.inventory_limit,
            max_skew_in_ticks: self.max_skew,
            drift_tolerance_in_ticks: self.drift,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Before anything is connected or signed. A configuration that could cross its own
    // ladder should not reach a cluster to find that out.
    let params = cli.params();
    params.validate().context("these parameters cannot be quoted safely")?;

    let keypair = match &cli.trader {
        Some(name) => Some(TraderRecord::keypair_path(&cli.cluster, name)),
        None => cli.keypair.clone(),
    };
    let client = Client::new(Config::load(keypair.as_deref())?);
    let record = MarketRecord::load(&cli.cluster)?;
    let session = Session::new(client, record.addresses()?, params, cli.dry_run);

    println!("market   {}", record.market);
    println!("quoting  {}", session.trader());
    println!(
        "ladder   {} level(s) per side, {} lots each, {}±{} ticks",
        params.levels, params.size_in_base_lots, params.half_spread_in_ticks, params.max_skew_in_ticks
    );
    if cli.dry_run {
        println!("dry run  deciding only, nothing will be sent");
    } else {
        println!("seat     {}", session.claim_seat()?);
    }
    println!();

    if cli.once {
        report(&session.cycle()?);
        return Ok(());
    }
    run(&session, Duration::from_secs(cli.interval), cli.dry_run)
}

/// Cycles until interrupted, then takes the ladder down.
fn run(session: &Session, interval: Duration, dry_run: bool) -> Result<()> {
    let running = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&running);
    ctrlc::set_handler(move || flag.store(false, Ordering::SeqCst))
        .context("installing the shutdown handler")?;

    while running.load(Ordering::SeqCst) {
        match session.cycle() {
            Ok(outcome) => report(&outcome),
            // A cycle that fails changes nothing: the ladder that was resting is still
            // resting, at prices that were fine a moment ago. Stopping on the first RPC
            // hiccup would abandon a live position over a network error.
            Err(error) => eprintln!("cycle failed: {error:#}"),
        }
        // Broken into short waits so an interrupt is noticed within a second rather than
        // at the end of the interval.
        for _ in 0..interval.as_secs().max(1) {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    println!("\nstopping");
    if dry_run {
        return Ok(());
    }
    // The one thing that must happen on the way out.
    match session.withdraw_quotes() {
        Ok(signature) => println!("ladder withdrawn  {signature}"),
        Err(error) => eprintln!("could not withdraw the ladder: {error:#}"),
    }
    Ok(())
}

/// One line per cycle: what the bot saw, what it wanted, and what it did.
fn report(report: &Report) {
    let action = match &report.plan {
        Plan::Hold => "hold".to_string(),
        Plan::Replace { reason, .. } => match reason {
            Reason::Missing => "refresh (a quote is gone)".to_string(),
            Reason::Resized => "refresh (partly filled)".to_string(),
            Reason::Drifted { by_ticks } => format!("refresh (drifted {by_ticks} ticks)"),
        },
    };

    print!(
        "fair {:>9} [{:?}]  skew {:>+4}  inv {:>+8}  book {} -> {}  {action}",
        report.fair.price_in_ticks,
        report.fair.source,
        report.skew_in_ticks,
        report.inventory.deviation_in_lots,
        report.resting,
        report.desired.len(),
    );
    match &report.signature {
        Some(signature) => println!("  {signature}"),
        None => println!(),
    }
}
