//! VAA submission library for the executor-account-resolver protocol.
//!
//! Provides a [`SolanaConnection`] trait that abstracts over RPC and LiteSVM,
//! plus generic resolver and executor logic.
//!
//! # RPC convenience
//!
//! [`broadcast_vaa`] is a high-level function that performs the complete flow:
//! 1. Post guardian signatures
//! 2. Resolve accounts via simulation
//! 3. Execute the resolved instructions
//! 4. Close the signatures account

pub mod connection;
pub mod execute;
pub mod resolve;
pub mod signatures;

pub use connection::SolanaConnection;
pub use resolve::{
    InstructionGroup, ResolverResult, SerializableAccountMeta, SerializableInstruction,
    RESOLVER_PUBKEY_SHIM_VAA_SIGS,
};

// Re-export placeholder constants at crate root for convenience.
pub use executor_account_resolver_svm::{RESOLVER_PUBKEY_GUARDIAN_SET, RESOLVER_PUBKEY_PAYER};

use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
};
use wormhole_svm_definitions::find_guardian_set_address;

/// Maximum resolver iterations before giving up.
const MAX_RESOLVER_ITERATIONS: usize = 10;

/// Errors that can occur during VAA submission.
#[derive(thiserror::Error, Debug)]
pub enum SubmitError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Resolver simulation error: {0}")]
    ResolverSimulation(String),

    #[error("Execution error: {0}")]
    Execution(String),
}

impl From<solana_client::client_error::ClientError> for SubmitError {
    fn from(e: solana_client::client_error::ClientError) -> Self {
        SubmitError::Connection(e.to_string())
    }
}

/// Submit a signed VAA to a program that implements `resolve_execute_vaa_v1`.
///
/// This performs the complete broadcast flow:
/// 1. Resolve accounts via simulated resolver calls
/// 2. Post guardian signatures to the Wormhole Verify VAA Shim
/// 3. Execute the resolved instructions (substituting placeholders)
/// 4. Close the signatures account to reclaim rent
///
/// Currently only supports programs that use the Verify VAA Shim (i.e. the
/// resolved instructions reference `RESOLVER_PUBKEY_SHIM_VAA_SIGS`). Legacy
/// programs that verify VAAs differently are not yet supported.
///
/// # Arguments
///
/// * `rpc_client` - Connected RPC client
/// * `payer` - Keypair that pays for transactions
/// * `program_id` - The program implementing `resolve_execute_vaa_v1`
/// * `guardian_set_index` - On-chain guardian set index
/// * `vaa_body` - The VAA body bytes (without header/signatures)
/// * `guardian_signatures` - Guardian signatures (66 bytes each: [index, r, s, v])
/// * `core_bridge` - Wormhole Core Bridge program ID (for guardian set PDA derivation)
pub fn broadcast_vaa(
    rpc_client: &mut RpcClient,
    payer: &Keypair,
    program_id: &Pubkey,
    guardian_set_index: u32,
    vaa_body: &[u8],
    guardian_signatures: &[[u8; 66]],
    core_bridge: &Pubkey,
) -> Result<Vec<Signature>, SubmitError> {
    let (guardian_set, _bump) =
        find_guardian_set_address(guardian_set_index.to_be_bytes(), core_bridge);

    // Step 1: Resolve accounts (no on-chain state needed yet)
    eprintln!("Resolving accounts...");
    let resolved = resolve::resolve_execute_vaa_v1(
        rpc_client,
        program_id,
        payer,
        vaa_body,
        &guardian_set,
        MAX_RESOLVER_ITERATIONS,
    )?;
    eprintln!(
        "Resolved in {} iterations ({} instruction groups)",
        resolved.iterations,
        resolved.instruction_groups.len()
    );

    // Check that the program uses the Verify VAA Shim.
    // TODO: support legacy programs that verify VAAs without the shim
    let uses_shim = resolved.instruction_groups.iter().any(|group| {
        group.instructions.iter().any(|ix| {
            ix.accounts
                .iter()
                .any(|a| a.pubkey == RESOLVER_PUBKEY_SHIM_VAA_SIGS)
        })
    });
    if !uses_shim {
        return Err(SubmitError::Execution(
            "Program does not use the Verify VAA Shim (no RESOLVER_PUBKEY_SHIM_VAA_SIGS in \
             resolved instructions). Legacy VAA verification is not yet supported."
                .to_string(),
        ));
    }

    // Step 2: Post guardian signatures
    // TODO: solana::* addresses are all mainnet. it's fine for the shim because
    // it has the same address everywhere.
    let verify_vaa_shim = wormhole_svm_definitions::solana::VERIFY_VAA_SHIM_PROGRAM_ID;
    eprintln!("Posting guardian signatures...");
    let posted = signatures::post_signatures(
        rpc_client,
        payer,
        &verify_vaa_shim,
        guardian_set_index,
        guardian_signatures,
    )?;
    let sigs_pubkey = posted.keypair.pubkey();
    eprintln!("Signatures posted: {}", sigs_pubkey);

    // Steps 3-4 wrapped so we always close signatures even on failure
    let result = (|| -> Result<Vec<Signature>, SubmitError> {
        // Step 3: Execute resolved instructions
        eprintln!("Executing resolved instructions...");
        let tx_sigs = execute::execute_instruction_groups(
            rpc_client,
            payer,
            &resolved.instruction_groups,
            &sigs_pubkey,
            &guardian_set,
        )?;
        for sig in &tx_sigs {
            eprintln!("Executed: {}", sig);
        }

        Ok(tx_sigs)
    })();

    // Step 4: Always close signatures account to reclaim rent
    eprintln!("Closing signatures account...");
    if let Err(e) = signatures::close_signatures(rpc_client, payer, &verify_vaa_shim, &sigs_pubkey)
    {
        eprintln!("Warning: failed to close signatures account: {}", e);
    }
    eprintln!("Done.");

    result
}
