//! A file-backed store.
//!
//! Exists for two reasons. It makes a single-node deployment possible without running a
//! database — and, more usefully here, it makes the resume path testable: an in-memory
//! store loses its checkpoints on restart, so nothing that only has one can prove a
//! restart picks up where it stopped.
//!
//! Reads are served from memory and writes go through to disk, so this is [`Memory`]
//! plus durability rather than a second implementation of the query logic. Loading
//! everything at startup bounds it to what fits in memory, which is the honest limit of
//! a file-backed store and the reason ClickHouse exists alongside it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;

use super::{Checkpoint, Memory, Range, StoredTrade, Store};

/// Trades, one JSON object per line.
const TRADES_FILE: &str = "trades.jsonl";

/// Checkpoints, one file per market.
const CHECKPOINT_DIR: &str = "checkpoints";

/// A store kept in a directory.
pub struct Files {
    memory: Memory,
    dir: PathBuf,
}

/// A trade, as written to disk.
///
/// Separate from [`StoredTrade`] because that one holds a `Pubkey` and a `[u8; 64]`,
/// neither of which serialises to anything a human can read in a log file.
#[derive(Serialize, Deserialize)]
struct TradeRow {
    market: String,
    slot: u64,
    signature: String,
    price_in_ticks: u64,
    base_lots: u64,
    quote_lots: u64,
    maker_seat: u32,
    /// Defaulted, so a file written before the column existed still loads — as
    /// unattributed, which is exactly what it is.
    #[serde(default)]
    taker_seat: Option<u32>,
    maker_order_sequence: u64,
    taker_side_is_bid: bool,
}

#[derive(Serialize, Deserialize)]
struct CheckpointRow {
    slot: u64,
    data: String,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect()
}

impl TradeRow {
    fn of(trade: &StoredTrade) -> Self {
        Self {
            market: trade.market.to_string(),
            slot: trade.slot,
            signature: hex(&trade.signature),
            price_in_ticks: trade.price_in_ticks,
            base_lots: trade.base_lots,
            quote_lots: trade.quote_lots,
            maker_seat: trade.maker_seat,
            taker_seat: trade.taker_seat,
            maker_order_sequence: trade.maker_order_sequence,
            taker_side_is_bid: trade.taker_side_is_bid,
        }
    }

    fn into_trade(self) -> Result<StoredTrade> {
        let bytes = unhex(&self.signature).context("a signature was not hex")?;
        Ok(StoredTrade {
            market: self.market.parse().context("a market was not a pubkey")?,
            slot: self.slot,
            signature: <[u8; 64]>::try_from(bytes.as_slice())
                .ok()
                .context("a signature was not 64 bytes")?,
            price_in_ticks: self.price_in_ticks,
            base_lots: self.base_lots,
            quote_lots: self.quote_lots,
            maker_seat: self.maker_seat,
            taker_seat: self.taker_seat,
            maker_order_sequence: self.maker_order_sequence,
            taker_side_is_bid: self.taker_side_is_bid,
        })
    }
}

impl Files {
    /// Opens a directory, creating it and loading whatever it already holds.
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(dir.join(CHECKPOINT_DIR))
            .with_context(|| format!("cannot create {}", dir.display()))?;

        let store = Self {
            memory: Memory::new(),
            dir,
        };
        store.load().await?;
        Ok(store)
    }

    /// Reads it out of the environment, if one is configured.
    pub fn path_from_env() -> Option<PathBuf> {
        std::env::var("STORE_PATH").ok().map(PathBuf::from)
    }

    async fn load(&self) -> Result<()> {
        if let Ok(contents) = std::fs::read_to_string(self.dir.join(TRADES_FILE)) {
            let mut trades = Vec::new();
            for line in contents.lines().filter(|line| !line.trim().is_empty()) {
                // One corrupt line — a half-written record from a kill mid-append —
                // should not cost the whole history. Skip it and keep the rest.
                match serde_json::from_str::<TradeRow>(line).map(TradeRow::into_trade) {
                    Ok(Ok(trade)) => trades.push(trade),
                    _ => eprintln!("skipping an unreadable trade record"),
                }
            }
            self.memory.append(&trades).await?;
        }

        let checkpoints = self.dir.join(CHECKPOINT_DIR);
        let Ok(entries) = std::fs::read_dir(&checkpoints) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let Ok(market) = entry
                .path()
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .parse::<Pubkey>()
            else {
                continue;
            };
            let Ok(contents) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let Ok(row) = serde_json::from_str::<CheckpointRow>(&contents) else {
                continue;
            };
            if let Some(data) = unhex(&row.data) {
                self.memory
                    .save_checkpoint(&market, &Checkpoint { slot: row.slot, data })
                    .await?;
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for Files {
    async fn append(&self, trades: &[StoredTrade]) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }

        // Written before the in-memory copy is updated. The other order would report a
        // trade as stored that a crash could still lose.
        let mut body = String::new();
        for trade in trades {
            body.push_str(&serde_json::to_string(&TradeRow::of(trade))?);
            body.push('\n');
        }

        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(TRADES_FILE))
            .context("cannot open the trades file")?;
        file.write_all(body.as_bytes())
            .context("cannot write trades")?;
        file.sync_all().context("cannot flush trades")?;

        self.memory.append(trades).await
    }

    async fn trades(&self, market: &Pubkey, range: Range) -> Result<Vec<StoredTrade>> {
        self.memory.trades(market, range).await
    }

    async fn highest_slot(&self, market: &Pubkey) -> Result<Option<u64>> {
        self.memory.highest_slot(market).await
    }

    async fn save_checkpoint(&self, market: &Pubkey, checkpoint: &Checkpoint) -> Result<()> {
        // Memory refuses to move a checkpoint backwards; asking it first means the file
        // is not rewritten with an older state either.
        let before = self.memory.checkpoint(market).await?;
        self.memory.save_checkpoint(market, checkpoint).await?;
        let after = self.memory.checkpoint(market).await?;
        if before == after {
            return Ok(());
        }

        let row = CheckpointRow {
            slot: checkpoint.slot,
            data: hex(&checkpoint.data),
        };
        let path = self.dir.join(CHECKPOINT_DIR).join(format!("{market}.json"));
        // Written to a temporary file and renamed, so a crash mid-write leaves the
        // previous checkpoint intact rather than a truncated one.
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, serde_json::to_string(&row)?)
            .context("cannot write a checkpoint")?;
        std::fs::rename(&temporary, &path).context("cannot replace a checkpoint")?;
        Ok(())
    }

    async fn checkpoint(&self, market: &Pubkey) -> Result<Option<Checkpoint>> {
        self.memory.checkpoint(market).await
    }

    async fn checkpointed_markets(&self) -> Result<Vec<Pubkey>> {
        self.memory.checkpointed_markets().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(taker_seat: Option<u32>) -> TradeRow {
        TradeRow::of(&StoredTrade {
            market: Pubkey::new_from_array([3u8; 32]),
            slot: 42,
            signature: [7u8; 64],
            price_in_ticks: 150_000,
            base_lots: 25,
            quote_lots: 3_750_000,
            maker_seat: 4,
            taker_seat,
            maker_order_sequence: 991,
            taker_side_is_bid: true,
        })
    }

    #[test]
    fn a_row_survives_a_round_trip_through_json() {
        for seat in [Some(9), None] {
            let text = serde_json::to_string(&row(seat)).unwrap();
            let back: TradeRow = serde_json::from_str(&text).unwrap();
            assert_eq!(back.into_trade().unwrap().taker_seat, seat, "for {seat:?}");
        }
    }

    #[test]
    fn a_line_written_before_the_column_existed_still_loads() {
        // This store appends to one file forever, so every line ever written has to stay
        // readable. Without a default, adding a column would make the whole tape
        // unloadable at the next restart — and the failure would arrive at startup, long
        // after the change that caused it.
        let text = serde_json::to_string(&row(Some(9))).unwrap();
        let mut fields: serde_json::Value = serde_json::from_str(&text).unwrap();
        fields.as_object_mut().unwrap().remove("taker_seat");

        let old: TradeRow = serde_json::from_value(fields).expect("an old line should load");
        assert_eq!(old.into_trade().unwrap().taker_seat, None);
    }
}
