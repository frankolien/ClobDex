//! A ClickHouse-backed store.
//!
//! Speaks the HTTP interface — a POST whose body is SQL — rather than depending on a
//! driver crate. Four statements is not enough to justify one, and the wire format here
//! is `TabSeparated`, which is stable and trivially checkable.
//!
//! # Why the table is what it is
//!
//! `ReplacingMergeTree` rather than `MergeTree`: a reconnect can replay a slot that was
//! already flushed, so the same row can legitimately arrive twice. Deduplication happens
//! on merge, and `FINAL` on read makes it exact rather than eventual — correctness over
//! throughput, on a table that is only written once per finalized slot.
//!
//! Ordered by `(market, slot)` because every query is "this market, this slot range".

use anyhow::{Context, Result, bail};
use solana_pubkey::Pubkey;

use super::{Checkpoint, Range, StoredTrade, Store};

/// The table trades are written to.
pub const TABLE: &str = "clob_trades";

/// The table market checkpoints are written to.
pub const CHECKPOINT_TABLE: &str = "clob_checkpoints";

/// A ClickHouse endpoint.
pub struct ClickHouse {
    url: String,
    database: String,
    client: reqwest::Client,
}

impl ClickHouse {
    /// Points at an HTTP endpoint, e.g. `http://localhost:8123`.
    pub fn new(url: String, database: String) -> Self {
        Self {
            url,
            database,
            client: reqwest::Client::new(),
        }
    }

    /// Reads the endpoint out of the environment, if one is configured.
    ///
    /// `None` rather than an error when unset: persistence is optional, and a deployment
    /// that only wants a live feed should not have to configure a database to start.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("CLICKHOUSE_URL").ok()?;
        let database = std::env::var("CLICKHOUSE_DATABASE").unwrap_or_else(|_| "default".into());
        Some(Self::new(url, database))
    }

    /// Creates the database and table if they are absent.
    pub async fn migrate(&self) -> Result<()> {
        self.execute(&format!(
            "CREATE DATABASE IF NOT EXISTS {}",
            self.database
        ))
        .await?;

        self.execute(&format!(
            "CREATE TABLE IF NOT EXISTS {}.{TABLE} (
                market       String,
                slot         UInt64,
                signature    String,
                price_in_ticks UInt64,
                base_lots    UInt64,
                quote_lots   UInt64,
                maker_seat   UInt32,
                maker_order  UInt64,
                taker_is_bid UInt8
             ) ENGINE = ReplacingMergeTree()
             ORDER BY (market, slot, signature, maker_order)",
            self.database
        ))
        .await?;

        // One row per market, the highest slot winning. ReplacingMergeTree's version
        // column does exactly that, so a late write cannot overwrite a newer checkpoint.
        self.execute(&format!(
            "CREATE TABLE IF NOT EXISTS {}.{CHECKPOINT_TABLE} (
                market String,
                slot   UInt64,
                data   String
             ) ENGINE = ReplacingMergeTree(slot)
             ORDER BY market",
            self.database
        ))
        .await?;
        Ok(())
    }

    /// Runs a statement, returning its body.
    async fn execute(&self, sql: &str) -> Result<String> {
        let response = self
            .client
            .post(&self.url)
            .body(sql.to_string())
            .send()
            .await
            .context("could not reach ClickHouse")?;

        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("ClickHouse returned {status}: {}", body.trim());
        }
        Ok(body)
    }
}

/// Hex, because a signature is 64 raw bytes and SQL string literals are not.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Hex of any length, for the account blob a checkpoint carries.
fn unhex_bytes(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2)
        .map(|index| u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok())
        .collect()
}

fn unhex(text: &str) -> Option<[u8; 64]> {
    if text.len() != 128 {
        return None;
    }
    let mut out = [0u8; 64];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// One row, tab-separated, as `INSERT ... FORMAT TabSeparated` expects.
///
/// Every field is a number or a hex string, so nothing here can contain a tab or a
/// newline and no escaping is needed. That is a property of the schema, not luck — a
/// `String` column taking arbitrary input would need quoting.
fn row(trade: &StoredTrade) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        trade.market,
        trade.slot,
        hex(&trade.signature),
        trade.price_in_ticks,
        trade.base_lots,
        trade.quote_lots,
        trade.maker_seat,
        trade.maker_order_sequence,
        u8::from(trade.taker_side_is_bid),
    )
}

#[async_trait::async_trait]
impl Store for ClickHouse {
    async fn append(&self, trades: &[StoredTrade]) -> Result<()> {
        if trades.is_empty() {
            return Ok(());
        }
        let rows: Vec<String> = trades.iter().map(row).collect();
        self.execute(&format!(
            "INSERT INTO {}.{TABLE} FORMAT TabSeparated\n{}",
            self.database,
            rows.join("\n")
        ))
        .await?;
        Ok(())
    }

    async fn trades(&self, market: &Pubkey, range: Range) -> Result<Vec<StoredTrade>> {
        // FINAL collapses duplicate rows at read time. Without it a replayed slot would
        // be counted twice until ClickHouse happened to merge the parts.
        let body = self
            .execute(&format!(
                "SELECT market, slot, signature, price_in_ticks, base_lots, quote_lots,
                        maker_seat, maker_order, taker_is_bid
                 FROM {}.{TABLE} FINAL
                 WHERE market = '{market}' AND slot >= {} AND slot <= {}
                 ORDER BY slot ASC
                 LIMIT {} FORMAT TabSeparated",
                self.database, range.from_slot, range.to_slot, range.limit
            ))
            .await?;

        body.lines().filter(|line| !line.is_empty()).map(parse).collect()
    }

    async fn save_checkpoint(&self, market: &Pubkey, checkpoint: &Checkpoint) -> Result<()> {
        // Hex rather than base64: TabSeparated has no escaping, and hex cannot contain a
        // tab, a newline, or a backslash whatever the bytes are.
        self.execute(&format!(
            "INSERT INTO {}.{CHECKPOINT_TABLE} FORMAT TabSeparated\n{market}\t{}\t{}",
            self.database,
            checkpoint.slot,
            hex(&checkpoint.data)
        ))
        .await?;
        Ok(())
    }

    async fn checkpoint(&self, market: &Pubkey) -> Result<Option<Checkpoint>> {
        let body = self
            .execute(&format!(
                "SELECT slot, data FROM {}.{CHECKPOINT_TABLE} FINAL
                 WHERE market = '{market}' FORMAT TabSeparated",
                self.database
            ))
            .await?;

        let Some(line) = body.lines().find(|line| !line.is_empty()) else {
            return Ok(None);
        };
        let (slot, data) = line
            .split_once('\t')
            .context("a checkpoint row had no data column")?;

        Ok(Some(Checkpoint {
            slot: slot.parse().context("a checkpoint slot was not a number")?,
            data: unhex_bytes(data).context("checkpoint data was not hex")?,
        }))
    }

    async fn checkpointed_markets(&self) -> Result<Vec<Pubkey>> {
        let body = self
            .execute(&format!(
                "SELECT market FROM {}.{CHECKPOINT_TABLE} FINAL FORMAT TabSeparated",
                self.database
            ))
            .await?;

        body.lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.parse().context("a checkpoint market was not a pubkey"))
            .collect()
    }

    async fn highest_slot(&self, market: &Pubkey) -> Result<Option<u64>> {
        let body = self
            .execute(&format!(
                "SELECT max(slot) FROM {}.{TABLE} WHERE market = '{market}' FORMAT TabSeparated",
                self.database
            ))
            .await?;

        // ClickHouse renders max() over no rows as 0, which is indistinguishable from a
        // real slot 0 — so the count decides whether anything is there at all.
        let count = self
            .execute(&format!(
                "SELECT count() FROM {}.{TABLE} WHERE market = '{market}' FORMAT TabSeparated",
                self.database
            ))
            .await?;
        if count.trim() == "0" {
            return Ok(None);
        }
        Ok(body.trim().parse::<u64>().ok())
    }
}

fn parse(line: &str) -> Result<StoredTrade> {
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 9 {
        bail!("expected 9 columns, got {}: {line}", fields.len());
    }
    let number = |index: usize| -> Result<u64> {
        fields[index]
            .parse()
            .with_context(|| format!("column {index} was not a number: {}", fields[index]))
    };

    Ok(StoredTrade {
        market: fields[0].parse().context("column 0 was not a pubkey")?,
        slot: number(1)?,
        signature: unhex(fields[2]).context("column 2 was not a 64-byte signature")?,
        price_in_ticks: number(3)?,
        base_lots: number(4)?,
        quote_lots: number(5)?,
        maker_seat: number(6)? as u32,
        maker_order_sequence: number(7)?,
        taker_side_is_bid: number(8)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade() -> StoredTrade {
        StoredTrade {
            market: Pubkey::new_from_array([3u8; 32]),
            slot: 42,
            signature: [7u8; 64],
            price_in_ticks: 150_000,
            base_lots: 25,
            quote_lots: 3_750_000,
            maker_seat: 4,
            maker_order_sequence: 991,
            taker_side_is_bid: true,
        }
    }

    #[test]
    fn a_row_survives_a_round_trip() {
        // The encoder and the parser are checked against each other rather than against
        // a fixture, which would freeze whatever the encoder did the day it was captured.
        let encoded = row(&trade());
        assert_eq!(parse(&encoded).unwrap(), trade());
    }

    #[test]
    fn a_row_has_no_tabs_or_newlines_inside_its_fields() {
        // TabSeparated has no escaping, so a field containing a separator would silently
        // shift every column after it.
        let encoded = row(&trade());
        assert_eq!(encoded.matches('\t').count(), 8);
        assert!(!encoded.contains('\n'));
    }

    #[test]
    fn a_signature_survives_hex() {
        let mut signature = [0u8; 64];
        for (index, byte) in signature.iter_mut().enumerate() {
            *byte = index as u8;
        }
        assert_eq!(unhex(&hex(&signature)), Some(signature));
    }

    #[test]
    fn a_truncated_signature_is_refused_rather_than_padded() {
        // Padding would attribute a trade to a transaction that does not exist.
        assert_eq!(unhex("00"), None);
        assert_eq!(unhex(&"z".repeat(128)), None);
    }

    #[test]
    fn a_checkpoint_blob_survives_hex_at_any_length() {
        // A market account is 19 KB, not 64 bytes, so it needs the variable-length path.
        let data: Vec<u8> = (0..1000).map(|index| index as u8).collect();
        assert_eq!(unhex_bytes(&hex(&data)), Some(data));
    }

    #[test]
    fn odd_length_hex_is_refused_rather_than_truncated() {
        assert_eq!(unhex_bytes("abc"), None);
    }

    #[test]
    fn a_short_row_is_an_error_not_a_guess() {
        assert!(parse("only\tthree\tfields").is_err());
    }
}
