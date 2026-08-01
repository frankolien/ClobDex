//! Streams a program's markets and serves the derived tape.
//!
//! # Two runtimes
//!
//! actix spawns `!Send` futures on a runtime per worker, and the gRPC client wants a
//! multi-threaded one. Rather than forcing either to accommodate the other, ingest runs
//! on its own tokio runtime in a dedicated thread and the two share an `Arc<Registry>`.
//! The alternative — one runtime, with the stream driven from an actix worker — ties
//! ingest's liveness to that worker's.

use std::sync::Arc;

use anyhow::{Context, Result};
use clob_stream::api;
use clob_stream::correlate::Correlator;
use clob_stream::laserstream::{self, LaserStream};
use clob_stream::pipeline::{self, Outcome};
use clob_stream::registry::Registry;
use clob_stream::source::{SlotStatus, Source, Update};
use solana_pubkey::Pubkey;

fn main() -> Result<()> {
    load_dotenv();

    let program_id: Pubkey = std::env::var("CLOB_PROGRAM_ID")
        .context("CLOB_PROGRAM_ID is not set — see .env.example")?
        .parse()
        .context("CLOB_PROGRAM_ID is not a valid pubkey")?;
    let bind = std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let registry = Registry::new();
    let ingest_registry = Arc::clone(&registry);

    // Ingest gets its own runtime and its own thread, so a stalled HTTP worker cannot
    // stop the stream and a stalled stream cannot stop the API from serving what it
    // already has.
    let ingest = std::thread::Builder::new()
        .name("ingest".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("building the ingest runtime");
            runtime.block_on(run_ingest(ingest_registry, program_id))
        })
        .context("spawning the ingest thread")?;

    println!("serving on http://{bind}");
    actix_web::rt::System::new().block_on(api::serve(registry, &bind))?;

    // Only reached when the API stops; the ingest thread ends with the process.
    match ingest.join() {
        Ok(result) => result,
        Err(_) => anyhow::bail!("the ingest thread panicked"),
    }
}

/// Consumes the stream until it ends.
async fn run_ingest(registry: Arc<Registry>, program_id: Pubkey) -> Result<()> {
    let endpoint = laserstream::endpoint_from_env(program_id)?;
    println!("subscribing to {} at {}", program_id, endpoint.url);

    let mut source = LaserStream::connect(&endpoint)?;
    let mut correlator = Correlator::new();

    while let Some(update) = source.next().await {
        // Slot status is acted on before correlation, because a dead slot has to reach
        // the registry whether or not it produced a change worth pairing.
        if let Update::Slot { slot, status } = &update {
            match status {
                SlotStatus::Finalized => registry.finalize(*slot),
                SlotStatus::Dead => {
                    println!("slot {slot} was abandoned — retracting its trades");
                    registry.retract(*slot);
                }
                SlotStatus::Confirmed => {}
            }
        }

        let Some(change) = correlator.accept(update) else {
            continue;
        };
        match pipeline::process(&change, &program_id) {
            // A market becomes visible on first sighting, before it has traded — the
            // alternative is an endpoint that reports no such market until its second
            // transaction.
            Ok(Outcome::Baseline { market, slot, state }) => {
                println!("tracking {market} from slot {slot}");
                registry.seed(market, state, slot);
            }
            Ok(Outcome::Derived(derived)) => {
                let reconciled = pipeline::reconciles(&derived);
                if !reconciled {
                    eprintln!(
                        "fees disagree with the derived trades at slot {} on {}",
                        derived.slot, derived.market
                    );
                }
                if !derived.delta.trades.is_empty() {
                    println!(
                        "slot {}: {} trade(s) on {}",
                        derived.slot,
                        derived.delta.trades.len(),
                        derived.market
                    );
                }
                registry.apply(derived, reconciled);
            }
            // One undecodable account is not a reason to stop streaming the others.
            Err(error) => eprintln!("slot {}: {error:#}", change.slot),
        }
    }

    anyhow::bail!("the stream ended")
}

/// Loads `.env` into the environment without overwriting anything already set.
fn load_dotenv() {
    let Ok(contents) = std::fs::read_to_string(".env") else {
        return;
    };
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && std::env::var(key.trim()).is_err()
        {
            unsafe { std::env::set_var(key.trim(), value.trim()) };
        }
    }
}
