//! Where the CLI gets its endpoint, its program, and its signer.
//!
//! Everything comes from the environment, and `.env` is read into the environment first
//! so a token never has to be typed on a command line — shell history is a file too.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use solana_commitment_config::CommitmentConfig;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;

/// Everything the CLI needs to talk to a cluster.
pub struct Config {
    /// HTTP JSON-RPC endpoint.
    pub rpc_url: String,
    /// The deployed ClobDex program.
    pub program_id: Pubkey,
    /// How long to wait before believing a transaction landed.
    pub commitment: CommitmentConfig,
    /// Pays for and signs everything.
    pub payer: Keypair,
}

impl Config {
    /// Reads `.env`, then the environment, then the keypair file.
    pub fn load(keypair_path: Option<&Path>) -> Result<Self> {
        load_dotenv(Path::new(".env"));

        let rpc_url = var("RPC_URL")?;
        let program_id = Pubkey::from_str(&var("CLOB_PROGRAM_ID")?)
            .context("CLOB_PROGRAM_ID is not a valid pubkey")?;

        let commitment = match std::env::var("COMMITMENT").as_deref() {
            Ok("finalized") => CommitmentConfig::finalized(),
            Ok("processed") => CommitmentConfig::processed(),
            _ => CommitmentConfig::confirmed(),
        };

        let path = match keypair_path {
            Some(path) => path.to_path_buf(),
            None => default_keypair_path()?,
        };

        Ok(Self {
            rpc_url,
            program_id,
            commitment,
            payer: read_keypair(&path)?,
        })
    }
}

fn var(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("{key} is not set — see .env.example"))
}

/// Loads `KEY=VALUE` lines into the environment, without overwriting anything already
/// set, so an explicit environment variable always beats the file.
fn load_dotenv(path: &Path) {
    let Ok(contents) = std::fs::read_to_string(path) else {
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

fn default_keypair_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/solana/id.json"))
}

/// Reads the JSON byte-array format the Solana CLI writes.
pub fn read_keypair(path: &Path) -> Result<Keypair> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read keypair at {}", path.display()))?;
    let bytes: Vec<u8> =
        serde_json::from_str(&contents).with_context(|| format!("{} is not a keypair", path.display()))?;

    if bytes.len() != 64 {
        bail!("{} holds {} bytes, expected 64", path.display(), bytes.len());
    }
    Keypair::try_from(&bytes[..]).map_err(|e| anyhow::anyhow!("{} is not a valid keypair: {e}", path.display()))
}

/// Writes a keypair in the same format, so the Solana CLI can read it back.
pub fn write_keypair(path: &Path, keypair: &Keypair) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = keypair.to_bytes().to_vec();
    std::fs::write(path, serde_json::to_string(&bytes)?)
        .with_context(|| format!("cannot write keypair to {}", path.display()))?;
    Ok(())
}
