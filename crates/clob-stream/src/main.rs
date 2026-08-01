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
use clob_stream::flush::{Pending, flush};
use clob_stream::registry::Registry;
use clob_stream::snapshot::Rpc;
use clob_stream::store::{Checkpoint, Files, Memory, Store, clickhouse::ClickHouse};
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

    // One store, shared: ingest writes to it and the history endpoints read from it.
    // Persistence is optional — a deployment that only wants a live feed should not have
    // to run a database to start — and the in-memory store is a real implementation, not
    // a stub, so the read path is identical either way.
    let store = open_store()?;
    let ingest_store = Arc::clone(&store);

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
            runtime.block_on(run_ingest(ingest_registry, program_id, ingest_store))
        })
        .context("spawning the ingest thread")?;

    println!("serving on http://{bind}");
    actix_web::rt::System::new().block_on(api::serve(registry, store, &bind))?;

    // Only reached when the API stops; the ingest thread ends with the process.
    match ingest.join() {
        Ok(result) => result,
        Err(_) => anyhow::bail!("the ingest thread panicked"),
    }
}

/// Consumes the stream until it ends.
async fn run_ingest(registry: Arc<Registry>, program_id: Pubkey, store: Arc<dyn Store>) -> Result<()> {
    // Needs a runtime, so it happens here rather than where the store was chosen.
    if let Some(clickhouse) = ClickHouse::from_env() {
        clickhouse
            .migrate()
            .await
            .context("preparing the ClickHouse table")?;
    }

    let mut endpoint = laserstream::endpoint_from_env(program_id)?;
    let mut correlator = Correlator::new();
    let mut pending = Pending::new();

    // Checkpoints decide where to start. Each is a market's book at a rooted slot, so
    // replaying from the oldest of them means the same pipeline derives everything this
    // process missed — backfill with no second derivation path.
    let resume = restore(&mut correlator, &registry, store.as_ref()).await;
    if let Some(from_slot) = resume {
        endpoint.from_slot = Some(from_slot + 1);
        println!("resuming from slot {}", from_slot + 1);
    }

    println!("subscribing to {} at {}", program_id, endpoint.url);

    // Subscribe first, then snapshot. The other order leaves a window in which a
    // transaction lands between the two and is never seen: its account write would be
    // missed and the next diff would report two transactions' effects as one.
    let mut source = LaserStream::connect(&endpoint)?;

    // Only when there is nothing to resume from. Seeding at the current slot while
    // replaying from the past would discard the replay as stale.
    if resume.is_none() {
        seed_from_rpc(&mut correlator, &registry, program_id, endpoint.finalized).await;
    }
    if let Some(highest) = highest_stored(store.as_ref(), &correlator).await {
        pending = Pending::resuming_from(highest);
    }

    while let Some(update) = source.next().await {
        // Slot status is acted on before correlation, because a dead slot has to reach
        // the registry whether or not it produced a change worth pairing.
        if let Update::Slot { slot, status } = &update {
            match status {
                SlotStatus::Finalized => {
                    registry.finalize(*slot);
                    checkpoint(&correlator, store.as_ref(), *slot).await;
                    // Only rooted trades are written, which is what lets the store be
                    // append-only: anything retractable has not been written yet.
                    match flush(&mut pending, store.as_ref(), *slot).await {
                        Ok(0) => {}
                        Ok(written) => println!("stored {written} trade(s) rooted at slot {slot}"),
                        // Left queued for the next finalization: an unreachable store
                        // should cost latency, not data.
                        Err(error) => eprintln!("could not store trades: {error:#}"),
                    }
                }
                SlotStatus::Dead => {
                    let dropped = pending.retract(*slot);
                    println!("slot {slot} was abandoned — retracting {dropped} unstored trade(s)");
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
                pending.record(
                    derived.market,
                    derived.slot,
                    derived.signature,
                    &derived.delta.trades,
                );
                registry.apply(derived, reconciled);
            }
            // One undecodable account is not a reason to stop streaming the others.
            Err(error) => eprintln!("slot {}: {error:#}", change.slot),
        }
    }

    anyhow::bail!("the stream ended")
}

/// Chooses where trades are kept, in order of preference.
///
/// ClickHouse if configured, a directory if one is named, memory otherwise. Memory is a
/// real implementation rather than a fallback stub, so the only thing lost by taking it
/// is durability across a restart.
fn open_store() -> Result<Arc<dyn Store>> {
    if let Some(clickhouse) = ClickHouse::from_env() {
        println!("persisting rooted trades to ClickHouse");
        return Ok(Arc::new(clickhouse));
    }
    if let Some(path) = Files::path_from_env() {
        // Loading is the only blocking part and there is no runtime yet, so it gets one
        // of its own rather than deferring the choice into the ingest thread.
        let files = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(Files::open(&path))
            .with_context(|| format!("opening the store at {}", path.display()))?;
        println!("persisting rooted trades to {}", path.display());
        return Ok(Arc::new(files));
    }
    println!("no CLICKHOUSE_URL or STORE_PATH — keeping trades in memory only");
    Ok(Arc::new(Memory::new()))
}

/// Reloads every checkpointed market, returning the slot to replay from.
///
/// The oldest checkpoint wins: a market further behind than the others would otherwise
/// have its missed slots skipped, and a hole in one market's tape is still a hole.
async fn restore(
    correlator: &mut Correlator,
    registry: &Arc<Registry>,
    store: &dyn Store,
) -> Option<u64> {
    let markets = store.checkpointed_markets().await.unwrap_or_default();
    let mut oldest: Option<u64> = None;

    for market in markets {
        let Ok(Some(checkpoint)) = store.checkpoint(&market).await else {
            continue;
        };
        match MarketState::decode(&checkpoint.data) {
            Ok(state) => {
                correlator.seed_at(market, checkpoint.data.clone(), checkpoint.slot);
                registry.seed(market, state, checkpoint.slot);
                oldest = Some(oldest.map_or(checkpoint.slot, |slot: u64| slot.min(checkpoint.slot)));
                println!("restored {market} at slot {}", checkpoint.slot);
            }
            // A checkpoint that no longer decodes means the account layout changed under
            // it. Starting cold is correct; pretending to resume would diff two formats.
            Err(error) => eprintln!("checkpoint for {market} no longer decodes: {error:?}"),
        }
    }
    oldest
}

/// Seeds current state over RPC, for a cold start with nothing to resume from.
async fn seed_from_rpc(
    correlator: &mut Correlator,
    registry: &Arc<Registry>,
    program_id: Pubkey,
    finalized: bool,
) {
    let rpc = match Rpc::from_env() {
        Ok(rpc) => rpc,
        Err(error) => {
            eprintln!("no RPC to seed from, starting cold: {error:#}");
            return;
        }
    };

    match rpc.program_accounts(&program_id, finalized).await {
        Ok(snapshot) => {
            for account in &snapshot.accounts {
                // The program owns more than markets — vaults are token accounts and will
                // not decode. Skipping them is expected, not an error.
                if let Ok(state) = MarketState::decode(&account.data) {
                    correlator.seed_at(account.market, account.data.clone(), snapshot.slot);
                    registry.seed(account.market, state, snapshot.slot);
                    println!("seeded {} from slot {}", account.market, snapshot.slot);
                }
            }
        }
        // Streaming still works without it; the first transaction per market is lost,
        // which is the behaviour seeding replaced.
        Err(error) => eprintln!("could not seed from RPC, starting cold: {error:#}"),
    }
}

/// The highest slot already durable across every known market.
async fn highest_stored(store: &dyn Store, correlator: &Correlator) -> Option<u64> {
    let mut highest = None;
    for market in correlator.markets() {
        if let Ok(Some(slot)) = store.highest_slot(&market).await {
            highest = Some(highest.map_or(slot, |current: u64| current.max(slot)));
        }
    }
    highest
}

/// Records each market's book at `slot`, if that book is itself rooted.
///
/// A market whose latest state is newer than `slot` is skipped rather than written with
/// the wrong label — a checkpoint claiming a slot it does not describe would resume from
/// a book that never existed at that point.
async fn checkpoint(correlator: &Correlator, store: &dyn Store, slot: u64) {
    for market in correlator.markets() {
        let (Some(known_at), Some(data)) = (correlator.known_at(&market), correlator.latest(&market))
        else {
            continue;
        };
        if known_at > slot {
            continue;
        }
        let checkpoint = Checkpoint {
            slot: known_at,
            data: data.to_vec(),
        };
        if let Err(error) = store.save_checkpoint(&market, &checkpoint).await {
            eprintln!("could not checkpoint {market}: {error:#}");
        }
    }
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
