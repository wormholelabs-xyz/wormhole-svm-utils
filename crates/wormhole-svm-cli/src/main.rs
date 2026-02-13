use std::io::{self, IsTerminal, Read};
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::read_keypair_file;

#[derive(Parser)]
#[command(name = "svm-vaa")]
#[command(about = "Submit signed VAAs to Solana programs")]
struct Cli {
    /// Solana RPC URL
    #[arg(
        long,
        env = "SOLANA_RPC_URL",
        default_value = "https://api.devnet.solana.com"
    )]
    rpc_url: String,

    /// Wormhole Core Bridge program ID (auto-detected from --rpc-url if omitted)
    #[arg(long, env = "CORE_BRIDGE_PROGRAM_ID")]
    core_bridge: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Submit a signed VAA to a Solana program
    Submit {
        /// Program ID implementing resolve_execute_vaa_v1
        #[arg(long, env = "PROGRAM_ID")]
        program_id: String,

        /// Payer keypair file
        #[arg(long, env = "PAYER_KEYPAIR")]
        payer: String,

        /// Signed VAA (hex string, @file, or stdin)
        vaa: Option<String>,
    },

    /// Fetch and dump an account's data as hex
    Account {
        /// Account address (or use `pda:<PROGRAM_ID>:seed1:seed2` to derive)
        address: String,
    },

    /// Derive a PDA for a program
    ///
    /// Seeds can be strings or hex (prefix with 0x).
    ///
    /// Examples:
    ///   svm-vaa pda <PROGRAM_ID> foo bar baz
    ///   svm-vaa pda <PROGRAM_ID> 0xdeadbeef "hello"
    Pda {
        /// Program ID to derive PDA for
        program_id: String,

        /// Seeds (strings or 0x-prefixed hex)
        #[arg(required = true, num_args = 1..)]
        seeds: Vec<String>,
    },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {:#}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    match &cli.command {
        Command::Submit {
            program_id,
            payer,
            vaa,
        } => cmd_submit(&cli, program_id, payer, vaa.clone()),
        Command::Account { address } => cmd_account(&cli, address),
        Command::Pda { program_id, seeds } => cmd_pda(program_id, seeds),
    }
}

fn cmd_submit(
    cli: &Cli,
    program_id: &str,
    payer_path: &str,
    vaa_arg: Option<String>,
) -> Result<()> {
    let raw = read_input(vaa_arg)?;

    let (guardian_set_index, signatures, body) =
        parse_signed_vaa(&raw).context("parsing signed VAA")?;

    let payer = read_keypair_file(payer_path)
        .map_err(|e| anyhow::anyhow!("failed to read payer keypair: {}", e))?;
    let program_id = Pubkey::from_str(program_id).context("invalid program ID")?;
    let core_bridge = match &cli.core_bridge {
        Some(addr) => Pubkey::from_str(addr).context("invalid core bridge ID")?,
        None => core_bridge_from_rpc_url(&cli.rpc_url)
            .context("cannot auto-detect core bridge for this RPC URL; use --core-bridge")?,
    };

    let mut rpc_client = solana_client::rpc_client::RpcClient::new(&cli.rpc_url);

    eprintln!("Submitting VAA to {}...", program_id);
    eprintln!("  Payer: {}", solana_sdk::signer::Signer::pubkey(&payer));
    eprintln!("  Core Bridge: {}", core_bridge);
    eprintln!("  Guardian set index: {}", guardian_set_index);
    eprintln!("  Signatures: {}", signatures.len());
    eprintln!("  RPC: {}", cli.rpc_url);

    let tx_sigs = wormhole_svm_submit::broadcast_vaa(
        &mut rpc_client,
        &payer,
        &program_id,
        guardian_set_index,
        &body,
        &signatures,
        &core_bridge,
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    for sig in &tx_sigs {
        println!("{}", sig);
    }

    Ok(())
}

/// Parse a signed VAA into (guardian_set_index, signatures, body).
fn parse_signed_vaa(raw: &[u8]) -> Result<(u32, Vec<[u8; 66]>, Vec<u8>)> {
    if raw.is_empty() {
        bail!("empty VAA");
    }
    if raw[0] != 1 {
        bail!("unsupported VAA version: {}", raw[0]);
    }
    if raw.len() < 6 {
        bail!("VAA too short to contain header");
    }

    let guardian_set_index = u32::from_be_bytes(raw[1..5].try_into().unwrap());
    let sig_count = raw[5] as usize;
    let body_offset = 6 + sig_count * 66;

    if raw.len() < body_offset {
        bail!(
            "VAA truncated: expected at least {} bytes for {} signatures, got {}",
            body_offset,
            sig_count,
            raw.len()
        );
    }

    let mut signatures = Vec::with_capacity(sig_count);
    for i in 0..sig_count {
        let start = 6 + i * 66;
        let mut sig = [0u8; 66];
        sig.copy_from_slice(&raw[start..start + 66]);
        signatures.push(sig);
    }

    let body = raw[body_offset..].to_vec();
    Ok((guardian_set_index, signatures, body))
}

/// Read input from hex string argument, @file reference, or stdin.
fn read_input(arg: Option<String>) -> Result<Vec<u8>> {
    match arg {
        Some(s) if s.starts_with('@') => {
            let path = &s[1..];
            let contents =
                std::fs::read_to_string(path).with_context(|| format!("reading file: {}", path))?;
            hex::decode(contents.trim()).context("decoding hex from file")
        }
        Some(s) => hex::decode(s.trim()).context("decoding hex payload"),
        None => {
            if io::stdin().is_terminal() {
                bail!("no VAA provided; pass as argument, @file, or pipe to stdin");
            }
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            hex::decode(buf.trim()).context("decoding hex from stdin")
        }
    }
}

const CORE_BRIDGE_MAINNET: Pubkey =
    wormhole_svm_definitions::solana::mainnet::CORE_BRIDGE_PROGRAM_ID;
const CORE_BRIDGE_DEVNET: Pubkey = wormhole_svm_definitions::solana::devnet::CORE_BRIDGE_PROGRAM_ID;

fn cmd_account(cli: &Cli, address: &str) -> Result<()> {
    let pubkey = parse_address(address)?;
    let rpc = solana_client::rpc_client::RpcClient::new(&cli.rpc_url);
    let account = rpc
        .get_account(&pubkey)
        .with_context(|| format!("fetching account {}", pubkey))?;

    eprintln!("address: {}", pubkey);
    eprintln!("owner:   {}", account.owner);
    eprintln!("lamports: {}", account.lamports);
    eprintln!("data len: {}", account.data.len());
    println!("{}", hex::encode(&account.data));
    Ok(())
}

/// Parse an address as a base58 pubkey or `pda:<PROGRAM_ID>:seed1:seed2:...`
fn parse_address(address: &str) -> Result<Pubkey> {
    if let Some(rest) = address.strip_prefix("pda:") {
        let parts: Vec<&str> = rest.split(':').collect();
        if parts.len() < 2 {
            bail!("pda: syntax requires at least program_id and one seed: pda:<PROGRAM_ID>:seed1:...");
        }
        let program_id = Pubkey::from_str(parts[0]).context("invalid program ID in pda: address")?;
        let seed_bytes: Vec<Vec<u8>> = parts[1..]
            .iter()
            .map(|s| {
                if let Some(hex_str) = s.strip_prefix("0x") {
                    hex::decode(hex_str).with_context(|| format!("invalid hex seed: {}", s))
                } else {
                    Ok(s.as_bytes().to_vec())
                }
            })
            .collect::<Result<_>>()?;
        let seed_slices: Vec<&[u8]> = seed_bytes.iter().map(|s| s.as_slice()).collect();
        let (pda, bump) = Pubkey::find_program_address(&seed_slices, &program_id);
        eprintln!("pda: {} (bump {})", pda, bump);
        Ok(pda)
    } else {
        Pubkey::from_str(address).context("invalid account address")
    }
}

fn cmd_pda(program_id: &str, seeds: &[String]) -> Result<()> {
    let program_id = Pubkey::from_str(program_id).context("invalid program ID")?;

    let seed_bytes: Vec<Vec<u8>> = seeds
        .iter()
        .map(|s| {
            if let Some(hex_str) = s.strip_prefix("0x") {
                hex::decode(hex_str).with_context(|| format!("invalid hex seed: {}", s))
            } else {
                Ok(s.as_bytes().to_vec())
            }
        })
        .collect::<Result<_>>()?;

    let seed_slices: Vec<&[u8]> = seed_bytes.iter().map(|s| s.as_slice()).collect();
    let (pda, bump) =
        Pubkey::find_program_address(&seed_slices, &program_id);

    println!("{}", pda);
    eprintln!("bump: {}", bump);
    Ok(())
}

fn core_bridge_from_rpc_url(rpc_url: &str) -> Option<Pubkey> {
    let url = rpc_url.to_lowercase();
    if url.contains("mainnet") {
        Some(CORE_BRIDGE_MAINNET)
    } else if url.contains("devnet") {
        Some(CORE_BRIDGE_DEVNET)
    } else {
        None
    }
}
