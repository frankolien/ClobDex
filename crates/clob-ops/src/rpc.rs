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

/// What a validator did with a transaction it did not keep.
pub struct Simulation {
    /// Compute consumed, if the endpoint reported it.
    pub compute_units: Option<u64>,
    /// Why it failed, if it did.
    pub error: Option<String>,
}

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

    /// Runs instructions on the validator without sending them, reporting the compute
    /// they consumed.
    ///
    /// The only way to measure a program you did not build: it needs no signature that
    /// spends anything and no cooperation from the venue, so the same procedure works
    /// against any market on the cluster. Which is the point — a compute number is worth
    /// something only if the same method can be pointed at whatever you are comparing to.
    ///
    /// The transaction is signed anyway. Simulation does not require it, and a signed one
    /// is the same transaction that would land, so nothing is being measured that could
    /// not be sent.
    pub fn simulate(&self, instructions: &[Instruction]) -> Result<Simulation> {
        let blockhash = self
            .rpc
            .get_latest_blockhash()
            .context("cannot reach the RPC endpoint")?;
        let transaction = Transaction::new_signed_with_payer(
            instructions,
            Some(&self.payer.pubkey()),
            &[&self.payer],
            blockhash,
        );

        let response = self
            .rpc
            .simulate_transaction(&transaction)
            .context("the endpoint refused to simulate")?;

        Ok(Simulation {
            compute_units: response.value.units_consumed,
            // An instruction that reverts still reports the compute it burned before
            // reverting, which is a different number from the one being measured. Keeping
            // the error means a failed sample is reported as failed rather than as cheap.
            error: response.value.err.map(|error| error.to_string()),
        })
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
