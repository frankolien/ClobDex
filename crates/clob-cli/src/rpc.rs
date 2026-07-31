//! Sending transactions and reading accounts.
//!
//! A thin layer over `RpcClient` that exists for one reason: every command needs the
//! same "fetch a blockhash, sign, send, confirm, print the signature" sequence, and
//! writing it out per command is how one of them ends up not confirming.

use anyhow::{Context, Result};
use solana_client::rpc_client::RpcClient;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;

use crate::config::Config;

/// A connected cluster.
pub struct Client {
    rpc: RpcClient,
    /// The deployed ClobDex program.
    pub program_id: Pubkey,
    /// Pays for and signs everything.
    pub payer: Keypair,
}

impl Client {
    /// Connects using an already-loaded config.
    pub fn new(config: Config) -> Self {
        Self {
            rpc: RpcClient::new_with_commitment(config.rpc_url, config.commitment),
            program_id: config.program_id,
            payer: config.payer,
        }
    }

    /// The payer's address.
    pub fn payer_key(&self) -> Pubkey {
        self.payer.pubkey()
    }

    /// Signs and sends, waiting for confirmation.
    ///
    /// `extra` holds the keypairs of accounts being created in this transaction, which
    /// have to sign for their own allocation; the payer is always added.
    pub fn send(&self, instructions: &[Instruction], extra: &[&Keypair]) -> Result<String> {
        let blockhash = self
            .rpc
            .get_latest_blockhash()
            .context("cannot reach the RPC endpoint")?;

        let mut signers: Vec<&Keypair> = vec![&self.payer];
        // A keypair that signs twice makes the transaction invalid, and the payer is
        // frequently also the authority or the trader.
        for keypair in extra {
            if !signers.iter().any(|s| s.pubkey() == keypair.pubkey()) {
                signers.push(keypair);
            }
        }

        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &signers,
            blockhash,
        );

        let signature = self
            .rpc
            .send_and_confirm_transaction(&transaction)
            .context("transaction failed")?;
        Ok(signature.to_string())
    }

    /// Reads an account's data, or `None` if it does not exist.
    pub fn account_data(&self, address: &Pubkey) -> Result<Option<Vec<u8>>> {
        match self.rpc.get_account(address) {
            Ok(account) => Ok(Some(account.data)),
            Err(_) => Ok(None),
        }
    }

    /// Rent-exempt lamports for an account of `len` bytes.
    pub fn rent(&self, len: usize) -> Result<u64> {
        self.rpc
            .get_minimum_balance_for_rent_exemption(len)
            .context("cannot read rent parameters")
    }

    /// The payer's SOL balance, in lamports.
    pub fn balance(&self) -> Result<u64> {
        self.rpc
            .get_balance(&self.payer.pubkey())
            .context("cannot read balance")
    }
}
