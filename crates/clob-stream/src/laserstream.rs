//! The Yellowstone endpoint.
//!
//! Everything else in this crate is tested against a scripted [`Replay`](crate::source::Replay);
//! this is the part that cannot be, because it needs a cluster. Keeping it thin is the
//! point — it converts protobuf into [`Update`] and does no interpretation of its own.
//!
//! # Gap recovery
//!
//! LaserStream tracks the last slot it delivered and replays from there on reconnect,
//! so a dropped connection resumes rather than jumping to the tip. That is why this uses
//! the Helius client rather than a bare Yellowstone one: without replay, every
//! disconnect leaves a hole that has to be backfilled over RPC, and a hole in the middle
//! of a derived tape is indistinguishable from a quiet market.

use anyhow::{Context, Result};
use futures::{Stream, StreamExt};
use helius_laserstream::grpc::{
    SlotStatus as ProtoSlotStatus,
    CommitmentLevel, SubscribeRequest, SubscribeRequestFilterAccounts,
    SubscribeRequestFilterSlots, SubscribeRequestFilterTransactions, SubscribeUpdate,
    subscribe_update::UpdateOneof,
};
use helius_laserstream::{LaserstreamConfig, subscribe};
use solana_pubkey::Pubkey;
use std::collections::HashMap;
use std::pin::Pin;

use crate::source::{RawInstruction, SlotStatus, Source, Update};

/// A live subscription to one program's markets.
pub struct LaserStream {
    updates: Pin<Box<dyn Stream<Item = Result<SubscribeUpdate, helius_laserstream::LaserstreamError>> + Send>>,
}

/// How to reach the endpoint.
pub struct Endpoint {
    /// gRPC URL, e.g. `https://laserstream-devnet-ewr.helius-rpc.com`.
    pub url: String,
    /// Sent as the x-token metadata header.
    pub token: String,
    /// The program whose accounts and transactions to follow.
    pub program_id: Pubkey,
    /// `confirmed` indexes about a slot sooner; `finalized` cannot be rolled back.
    pub finalized: bool,
    /// Replay from this slot instead of starting at the tip.
    ///
    /// How a restart picks up where it stopped: the same pipeline runs over the missed
    /// slots, so backfill needs no second derivation path. Bounded by whatever history
    /// the endpoint retains — beyond that the stream starts wherever it can, and the
    /// caller is told the gap exists rather than left to infer it from a quiet tape.
    pub from_slot: Option<u64>,
}

impl LaserStream {
    /// Opens a subscription.
    ///
    /// Accounts are filtered by owner rather than by address, so a market created after
    /// the subscription opened is picked up without resubscribing.
    pub fn connect(endpoint: &Endpoint) -> Result<Self> {
        let config = LaserstreamConfig::new(endpoint.url.clone(), endpoint.token.clone())
            .with_replay(true);

        let commitment = match endpoint.finalized {
            true => CommitmentLevel::Finalized,
            false => CommitmentLevel::Confirmed,
        };
        let program = endpoint.program_id.to_string();

        let request = SubscribeRequest {
            accounts: HashMap::from([(
                "markets".to_string(),
                SubscribeRequestFilterAccounts {
                    owner: vec![program.clone()],
                    ..Default::default()
                },
            )]),
            transactions: HashMap::from([(
                "instructions".to_string(),
                SubscribeRequestFilterTransactions {
                    account_include: vec![program],
                    // Votes are the bulk of the cluster's traffic and touch nothing here.
                    vote: Some(false),
                    failed: Some(false),
                    ..Default::default()
                },
            )]),
            // Slot updates keep the tip moving while no market is trading, so a quiet
            // market is distinguishable from a stalled stream.
            // Dead slots are the reason this subscription exists: indexing at confirmed
            // publishes trades that can still be rolled back.
            slots: HashMap::from([(
                "tip".to_string(),
                SubscribeRequestFilterSlots {
                    filter_by_commitment: Some(false),
                    ..Default::default()
                },
            )]),
            commitment: Some(commitment as i32),
            from_slot: endpoint.from_slot,
            ..Default::default()
        };

        let (stream, _handle) = subscribe(config, request);
        Ok(Self {
            updates: Box::pin(stream),
        })
    }
}

impl Source for LaserStream {
    async fn next(&mut self) -> Option<Update> {
        loop {
            match self.updates.next().await? {
                // A decode failure is one malformed message, not a dead stream; the
                // client reconnects and replays on its own.
                Err(_) => continue,
                Ok(update) => {
                    if let Some(converted) = convert(update) {
                        return Some(converted);
                    }
                }
            }
        }
    }
}

/// Protobuf into [`Update`]. `None` for anything this crate does not consume.
fn convert(update: SubscribeUpdate) -> Option<Update> {
    match update.update_oneof? {
        UpdateOneof::Account(account) => {
            let info = account.account?;
            Some(Update::Account {
                slot: account.slot,
                market: pubkey(&info.pubkey)?,
                data: info.data,
                signature: info.txn_signature.as_deref().and_then(signature),
            })
        }
        UpdateOneof::Transaction(transaction) => {
            let info = transaction.transaction?;
            let message = info.transaction?.message?;
            let keys = account_keys(&message.account_keys)?;

            Some(Update::Transaction {
                slot: transaction.slot,
                signature: signature(&info.signature)?,
                // The subscription already excludes failures, but a stream is not a
                // contract: a failed transaction wrote nothing and must not be derived.
                succeeded: info.meta.as_ref().is_none_or(|meta| meta.err.is_none()),
                instructions: message
                    .instructions
                    .iter()
                    .filter_map(|instruction| {
                        Some(RawInstruction {
                            program_id: *keys.get(instruction.program_id_index as usize)?,
                            accounts: instruction
                                .accounts
                                .iter()
                                .filter_map(|index| keys.get(*index as usize).copied())
                                .collect(),
                            data: instruction.data.clone(),
                        })
                    })
                    .collect(),
            })
        }
        UpdateOneof::Slot(slot) => Some(Update::Slot {
            slot: slot.slot,
            // The rest describe a slot's progress through block production, which says
            // nothing about whether its writes will survive.
            status: match ProtoSlotStatus::try_from(slot.status).ok()? {
                ProtoSlotStatus::SlotConfirmed => SlotStatus::Confirmed,
                ProtoSlotStatus::SlotFinalized => SlotStatus::Finalized,
                ProtoSlotStatus::SlotDead => SlotStatus::Dead,
                _ => return None,
            },
        }),
        _ => None,
    }
}

fn pubkey(bytes: &[u8]) -> Option<Pubkey> {
    <[u8; 32]>::try_from(bytes).ok().map(Pubkey::new_from_array)
}

fn signature(bytes: &[u8]) -> Option<[u8; 64]> {
    <[u8; 64]>::try_from(bytes).ok()
}

fn account_keys(keys: &[Vec<u8>]) -> Option<Vec<Pubkey>> {
    keys.iter().map(|key| pubkey(key)).collect()
}

/// Reads an endpoint out of the environment.
pub fn endpoint_from_env(program_id: Pubkey) -> Result<Endpoint> {
    Ok(Endpoint {
        url: std::env::var("LASERSTREAM_ENDPOINT").context("LASERSTREAM_ENDPOINT is not set")?,
        token: std::env::var("LASERSTREAM_TOKEN").context("LASERSTREAM_TOKEN is not set")?,
        program_id,
        finalized: std::env::var("COMMITMENT").as_deref() == Ok("finalized"),
        from_slot: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_update_converts() {
        let update = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Slot(
                helius_laserstream::grpc::SubscribeUpdateSlot {
                    slot: 99,
                    status: ProtoSlotStatus::SlotConfirmed as i32,
                    ..Default::default()
                },
            )),
            ..Default::default()
        };
        assert_eq!(
            convert(update),
            Some(Update::Slot {
                slot: 99,
                status: SlotStatus::Confirmed
            })
        );
    }

    #[test]
    fn a_wrong_length_key_is_refused_rather_than_padded() {
        // Truncating or padding a key would silently attribute activity to the wrong
        // account, which is worse than dropping the message.
        assert_eq!(pubkey(&[1u8; 31]), None);
        assert_eq!(signature(&[1u8; 63]), None);
        assert!(pubkey(&[1u8; 32]).is_some());
    }
}
