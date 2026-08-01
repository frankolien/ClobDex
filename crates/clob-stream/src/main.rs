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
use clob_client::state::MarketState;
use clob_stream::api;
use clob_stream::correlate::Correlator;
use clob_stream::laserstream::{self, LaserStream};
use clob_stream::pipeline::{self, Outcome};
use clob_stream::registry::Registry;
use clob_stream::snapshot::Rpc;
use clob_stream::source::{SlotStatus, Source, Update};
use solana_pubkey::Pubkey;

fn main() -> Result<()> {
    load_dotenv();
    install_crypto_provider()?;

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

    // Subscribe first, then snapshot. The other order leaves a window in which a
    // transaction lands between the two and is never seen: its account write would be
    // missed and the next diff would report two transactions' effects as one.
    let mut source = LaserStream::connect(&endpoint)?;
    let mut correlator = Correlator::new();

    // Without this, the first update for each market only establishes a baseline, so
    // every restart silently loses the first transaction on every market.
    match Rpc::from_env()?
        .program_accounts(&program_id, endpoint.finalized)
        .await
    {
        Ok(snapshot) => {
            for account in &snapshot.accounts {
                match MarketState::decode(&account.data) {
                    Ok(state) => {
                        correlator.seed_at(account.market, account.data.clone(), snapshot.slot);
                        registry.seed(account.market, state, snapshot.slot);
                        println!("seeded {} from slot {}", account.market, snapshot.slot);
                    }
                    // The program owns more than markets — vaults are token accounts and
                    // will not decode. Anything else undecodable is a version skew worth
                    // seeing, but not worth refusing to start over.
                    Err(_) => continue,
                }
            }
        }
        // Streaming still works without a snapshot; it just loses the first transaction
        // per market, which is the behaviour this replaced.
        Err(error) => eprintln!("could not seed from RPC, starting cold: {error:#}"),
    }

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

/// Picks the TLS backend for the whole process.
///
/// Two of them end up in the dependency graph — the RPC client pulls aws-lc-rs and the
/// gRPC stack pulls ring — and rustls refuses to guess, panicking on the first
/// connection instead. Choosing here fails at startup with a clear message rather than
/// on a background thread mid-handshake.
fn install_crypto_provider() -> Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("a TLS provider was already installed"))
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
