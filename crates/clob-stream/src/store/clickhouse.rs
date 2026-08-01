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
                taker_seat   Nullable(UInt32),
                maker_order  UInt64,
                taker_is_bid UInt8
             ) ENGINE = ReplacingMergeTree()
             ORDER BY (market, slot, signature, maker_order)",
            self.database
        ))
        .await?;

        // `CREATE TABLE IF NOT EXISTS` leaves an existing table exactly as it was, so a
        // column added after a deployment would never appear and every read would fail on
        // the missing name. Stating the addition separately is what makes this a
        // migration rather than a create; it is a no-op on a table that already has it.
        self.execute(&format!(
            "ALTER TABLE {}.{TABLE} ADD COLUMN IF NOT EXISTS taker_seat Nullable(UInt32) AFTER maker_seat",
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

/// How TabSeparated spells a null, and the only field here that is ever absent.
const NULL: &str = "\\N";

/// One row, tab-separated, as `INSERT ... FORMAT TabSeparated` expects.
///
/// Every field is a number, a hex string, or `\N`, so nothing here can contain a tab or a
/// newline and no escaping is needed. That is a property of the schema, not luck — a
/// `String` column taking arbitrary input would need quoting.
fn row(trade: &StoredTrade) -> String {
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
        trade.market,
        trade.slot,
        hex(&trade.signature),
        trade.price_in_ticks,
        trade.base_lots,
        trade.quote_lots,
        trade.maker_seat,
        trade
            .taker_seat
            .map(|seat| seat.to_string())
            .unwrap_or_else(|| NULL.into()),
        trade.maker_order_sequence,
        u8::from(trade.taker_side_is_bid),
    )
}

/// The query behind [`Store::trades`].
///
/// Two things here are load-bearing.
///
/// `FINAL` collapses duplicate rows at read time. Without it a replayed slot is counted
/// twice until ClickHouse happens to merge the parts.
///
/// The limit is applied to a `DESC` inner query and the order restored outside it, so a
/// bounded read keeps the *newest* rows. A plain `ORDER BY slot ASC LIMIT n` keeps the
/// oldest, which answers "the last hundred trades" with the first hundred ever — and
/// silently, since both return exactly `n` rows in ascending order.
fn trades_query(database: &str, market: &Pubkey, range: Range) -> String {
    format!(
        "SELECT * FROM (
             SELECT market, slot, signature, price_in_ticks, base_lots, quote_lots,
                    maker_seat, taker_seat, maker_order, taker_is_bid
             FROM {database}.{TABLE} FINAL
             WHERE market = '{market}' AND slot >= {} AND slot <= {}
             ORDER BY slot DESC
             LIMIT {}
         ) ORDER BY slot ASC FORMAT TabSeparated",
        range.from_slot, range.to_slot, range.limit
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
        let body = self.execute(&trades_query(&self.database, market, range)).await?;
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
    if fields.len() != 10 {
        bail!("expected 10 columns, got {}: {line}", fields.len());
    }
    let number = |index: usize| -> Result<u64> {
        fields[index]
            .parse()
            .with_context(|| format!("column {index} was not a number: {}", fields[index]))
    };

    // The one nullable column. `\N` is absence, not a parse failure, so it must be tested
    // for before parsing rather than recovered from afterwards — otherwise a genuinely
    // malformed seat would also read as unattributed.
    let taker_seat = match fields[7] {
        NULL => None,
        _ => Some(number(7)? as u32),
    };

    Ok(StoredTrade {
        market: fields[0].parse().context("column 0 was not a pubkey")?,
        slot: number(1)?,
        signature: unhex(fields[2]).context("column 2 was not a 64-byte signature")?,
        price_in_ticks: number(3)?,
        base_lots: number(4)?,
        quote_lots: number(5)?,
        maker_seat: number(6)? as u32,
        taker_seat,
        maker_order_sequence: number(8)?,
        taker_side_is_bid: number(9)? != 0,
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
            taker_seat: Some(9),
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
    fn an_unattributed_taker_survives_as_absent_rather_than_as_a_seat() {
        // The one nullable column. Round-tripping it to `Some(0)` would silently file
        // every unattributable fill under seat zero, which is a real trader.
        let unattributed = StoredTrade {
            taker_seat: None,
            ..trade()
        };
        let encoded = row(&unattributed);
        assert!(encoded.contains(NULL), "encoded as {encoded}");
        assert_eq!(parse(&encoded).unwrap(), unattributed);
        assert_eq!(parse(&encoded).unwrap().taker_seat, None);
    }

    #[test]
    fn a_malformed_seat_is_an_error_rather_than_an_absence() {
        // Only `\N` means absent. Recovering from a parse failure by returning `None`
        // would turn corruption into a plausible-looking row.
        let broken = row(&trade()).replace("\t9\t", "\tnine\t");
        assert!(parse(&broken).is_err());
    }

    #[test]
    fn a_bounded_read_asks_for_the_newest_rows() {
        // Pins the shape of the query, not the behaviour of the server — only a live
        // ClickHouse proves the latter, and there isn't one in a unit test. It is still
        // worth having: the difference between keeping the newest and the oldest rows is
        // one word, both spellings return `limit` rows in ascending order, and the wrong
        // one shows a stale tape rather than an error.
        let sql = trades_query(
            "clob",
            &Pubkey::new_from_array([3u8; 32]),
            Range::latest(100),
        );
        let inner = sql.find("ORDER BY slot DESC").expect("inner order is descending");
        let outer = sql.rfind("ORDER BY slot ASC").expect("outer order is ascending");
        let limit = sql.find("LIMIT 100").expect("the limit is applied");

        assert!(inner < limit, "the limit must apply to the descending order");
        assert!(limit < outer, "and the ascending order must be restored after it");
        assert!(sql.contains("FINAL"), "duplicates must collapse at read time");
    }

    #[test]
    fn a_row_has_no_tabs_or_newlines_inside_its_fields() {
        // TabSeparated has no escaping, so a field containing a separator would silently
        // shift every column after it.
        let encoded = row(&trade());
        assert_eq!(encoded.matches('\t').count(), 9);
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
