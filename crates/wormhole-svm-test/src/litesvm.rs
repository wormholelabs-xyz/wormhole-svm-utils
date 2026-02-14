//! LiteSVM helpers for setting up Wormhole test environments.

use std::path::PathBuf;

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    hash::Hash,
    instruction::Instruction,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};
use thiserror::Error;
use wormhole_svm_definitions::{
    find_guardian_set_address,
    solana::mainnet::{
        CORE_BRIDGE_CONFIG, CORE_BRIDGE_PROGRAM_ID, POST_MESSAGE_SHIM_PROGRAM_ID,
        VERIFY_VAA_SHIM_PROGRAM_ID,
    },
};
use wormhole_svm_submit::SolanaConnection;

pub use wormhole_svm_submit::signatures::PostedSignatures;

use crate::TestGuardianSet;

/// Bundled Wormhole Verify VAA Shim program binary (mainnet).
#[cfg(feature = "bundled-fixtures")]
pub const VERIFY_VAA_SHIM_BYTES: &[u8] = include_bytes!("../fixtures/verify_vaa_shim.so");

/// Bundled Wormhole Core Bridge program binary (mainnet).
#[cfg(feature = "bundled-fixtures")]
pub const CORE_BRIDGE_BYTES: &[u8] = include_bytes!("../fixtures/core_bridge.so");

/// Bundled Wormhole Post Message Shim program binary (mainnet).
#[cfg(feature = "bundled-fixtures")]
pub const POST_MESSAGE_SHIM_BYTES: &[u8] = include_bytes!("../fixtures/post_message_shim.so");

/// Errors that can occur when setting up Wormhole in LiteSVM.
#[derive(Error, Debug)]
pub enum WormholeTestError {
    #[error("Program binary not found: {program}\nSearched: {searched:?}\n\n{help}")]
    ProgramNotFound {
        program: String,
        searched: Vec<PathBuf>,
        help: String,
    },
    #[error("Failed to read program binary: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Failed to load program: {0}")]
    LoadError(String),
    #[error("VAA verification bypass detected: {0}")]
    VerificationBypass(String),
    #[error("Emitter chain check missing: {0}")]
    EmitterChainBypass(String),
    #[error("Emitter address check missing: {0}")]
    EmitterAddressBypass(String),
    #[error("Replay protection missing: {0}")]
    ReplayProtectionMissing(String),
    #[error("Submit error: {0}")]
    SubmitError(#[from] wormhole_svm_submit::SubmitError),
}

// ReplayProtection is defined in vaa.rs and re-exported from the crate root.

/// Configuration for loading Wormhole programs.
#[derive(Default)]
pub struct WormholeProgramsConfig {
    /// Path to verify_vaa_shim.so (or None to search default locations).
    pub verify_vaa_shim: Option<PathBuf>,
    /// Path to core_bridge.so (or None to search default locations).
    pub core_bridge: Option<PathBuf>,
    /// Path to post_message_shim.so (or None to search default locations).
    pub post_message_shim: Option<PathBuf>,
}

/// Accounts created by setup_wormhole.
pub struct WormholeAccounts {
    /// The guardian set PDA address.
    pub guardian_set: Pubkey,
    /// The guardian set PDA bump seed.
    pub guardian_set_bump: u8,
}

const PROGRAM_NOT_FOUND_HELP: &str = r#"Wormhole program binaries not found.

Enable the `bundled-fixtures` feature to use pre-bundled mainnet binaries:

    wormhole-svm-test = { version = "0.1", features = ["bundled-fixtures"] }

Or dump them from mainnet yourself:

    solana program dump --url https://api.mainnet-beta.solana.com \
        EFaNWErqAtVWufdNb7yofSHHfWFos843DFpu4JBw24at \
        fixtures/verify_vaa_shim.so

    solana program dump --url https://api.mainnet-beta.solana.com \
        worm2ZoG2kUd4vFXhvjh93UUH596ayRfgQ2MgjNMTth \
        fixtures/core_bridge.so

    solana program dump --url https://api.mainnet-beta.solana.com \
        EtZMZM22ViKMo4r5y4Anovs3wKQ2owUmDpjygnMMcdEX \
        fixtures/post_message_shim.so

Or set WORMHOLE_FIXTURES_DIR environment variable to point to existing binaries."#;

/// Search paths for program binaries.
fn search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Environment variable override
    if let Ok(dir) = std::env::var("WORMHOLE_FIXTURES_DIR") {
        paths.push(PathBuf::from(dir));
    }

    // Common locations
    paths.push(PathBuf::from("tests/fixtures"));
    paths.push(PathBuf::from("fixtures"));
    paths.push(PathBuf::from("target/deploy"));

    paths
}

/// Find a program binary file in default search locations.
fn find_program_file(filename: &str) -> Result<PathBuf, WormholeTestError> {
    let search = search_paths();
    for dir in &search {
        let path = dir.join(filename);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(WormholeTestError::ProgramNotFound {
        program: filename.to_string(),
        searched: search,
        help: PROGRAM_NOT_FOUND_HELP.to_string(),
    })
}

/// Load Wormhole programs into an existing LiteSVM instance.
///
/// With the `bundled-fixtures` feature enabled, programs are loaded from
/// bundled binaries by default. You can still override with explicit paths.
pub fn load_wormhole_programs(
    svm: &mut LiteSVM,
    config: WormholeProgramsConfig,
) -> Result<(), WormholeTestError> {
    // Load Verify VAA Shim
    let shim_bytes = get_program_bytes(
        "verify_vaa_shim.so",
        config.verify_vaa_shim.as_ref(),
        #[cfg(feature = "bundled-fixtures")]
        Some(VERIFY_VAA_SHIM_BYTES),
        #[cfg(not(feature = "bundled-fixtures"))]
        None,
    )?;
    svm.add_program(VERIFY_VAA_SHIM_PROGRAM_ID, &shim_bytes)
        .map_err(|e| WormholeTestError::LoadError(format!("verify_vaa_shim: {}", e)))?;

    // Load Core Bridge
    let bridge_bytes = get_program_bytes(
        "core_bridge.so",
        config.core_bridge.as_ref(),
        #[cfg(feature = "bundled-fixtures")]
        Some(CORE_BRIDGE_BYTES),
        #[cfg(not(feature = "bundled-fixtures"))]
        None,
    )?;
    svm.add_program(CORE_BRIDGE_PROGRAM_ID, &bridge_bytes)
        .map_err(|e| WormholeTestError::LoadError(format!("core_bridge: {}", e)))?;

    // Load Post Message Shim
    let post_shim_bytes = get_program_bytes(
        "post_message_shim.so",
        config.post_message_shim.as_ref(),
        #[cfg(feature = "bundled-fixtures")]
        Some(POST_MESSAGE_SHIM_BYTES),
        #[cfg(not(feature = "bundled-fixtures"))]
        None,
    )?;
    svm.add_program(POST_MESSAGE_SHIM_PROGRAM_ID, &post_shim_bytes)
        .map_err(|e| WormholeTestError::LoadError(format!("post_message_shim: {}", e)))?;

    Ok(())
}

/// Get program bytes from explicit path, bundled bytes, or file search.
fn get_program_bytes(
    filename: &str,
    explicit_path: Option<&PathBuf>,
    bundled: Option<&'static [u8]>,
) -> Result<Vec<u8>, WormholeTestError> {
    // Explicit path takes priority
    if let Some(path) = explicit_path {
        if path.exists() {
            return Ok(std::fs::read(path)?);
        }
        return Err(WormholeTestError::ProgramNotFound {
            program: filename.to_string(),
            searched: vec![path.clone()],
            help: PROGRAM_NOT_FOUND_HELP.to_string(),
        });
    }

    // Try bundled bytes if available
    if let Some(bytes) = bundled {
        return Ok(bytes.to_vec());
    }

    // Fall back to file search
    let path = find_program_file(filename)?;
    Ok(std::fs::read(&path)?)
}

/// Create a guardian set account in LiteSVM.
///
/// Returns the PDA address and bump of the created account.
pub fn create_guardian_set_account(
    svm: &mut LiteSVM,
    guardians: &TestGuardianSet,
    index: u32,
) -> (Pubkey, u8) {
    let (address, bump) = find_guardian_set_address(index.to_be_bytes(), &CORE_BRIDGE_PROGRAM_ID);
    let data = build_guardian_set_data(guardians, index);

    let rent = Rent::default();
    let lamports = rent.minimum_balance(data.len());

    let account = Account {
        lamports,
        data,
        owner: CORE_BRIDGE_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };

    svm.set_account(address, account).unwrap();

    (address, bump)
}

/// Create a bridge config account in LiteSVM.
///
/// This creates a full bridge config that supports both VAA verification and message posting.
///
/// Bridge config data structure (BridgeData, borsh-serialized):
/// - guardian_set_index: u32 (4 bytes, little-endian)
/// - last_lamports: u64 (8 bytes) - required for post_message fee tracking
/// - guardian_set_expiration_time: u32 (4 bytes) - BridgeConfig.guardian_set_expiration_time
/// - fee: u64 (8 bytes) - BridgeConfig.fee
pub fn create_bridge_config(svm: &mut LiteSVM, guardian_set_index: u32) {
    // Match the fee collector's initial balance so the core bridge fee check works.
    let rent = Rent::default();
    let fee_collector_lamports = rent.minimum_balance(0);

    let mut data = Vec::new();
    data.extend_from_slice(&guardian_set_index.to_le_bytes());
    data.extend_from_slice(&fee_collector_lamports.to_le_bytes()); // last_lamports
    data.extend_from_slice(&86400u32.to_le_bytes()); // 24 hour expiration
    data.extend_from_slice(&10u64.to_le_bytes()); // 10 lamport fee

    let lamports = rent.minimum_balance(data.len());

    let account = Account {
        lamports,
        data,
        owner: CORE_BRIDGE_PROGRAM_ID,
        executable: false,
        rent_epoch: 0,
    };

    svm.set_account(CORE_BRIDGE_CONFIG, account).unwrap();
}

/// The default bridge fee set by [`create_bridge_config`] (in lamports).
pub const DEFAULT_BRIDGE_FEE: u64 = 10;

/// Build a system transfer instruction that pays the Wormhole bridge fee.
///
/// The Post Message Shim does NOT transfer the fee itself. Callers must
/// include this instruction in the same transaction, before the
/// `post_message` instruction, so that `fee_collector.lamports - last_lamports >= fee`
/// when the core bridge checks.
pub fn build_bridge_fee_ix(payer: &Pubkey) -> Instruction {
    use wormhole_svm_definitions::solana::mainnet::CORE_BRIDGE_FEE_COLLECTOR;
    solana_sdk::system_instruction::transfer(payer, &CORE_BRIDGE_FEE_COLLECTOR, DEFAULT_BRIDGE_FEE)
}

/// Create the Wormhole fee collector account in LiteSVM.
///
/// The fee collector is needed for posting Wormhole messages.
/// It's a simple system-owned account that receives bridge fees.
pub fn create_fee_collector(svm: &mut LiteSVM) {
    use wormhole_svm_definitions::solana::mainnet::CORE_BRIDGE_FEE_COLLECTOR;

    let rent = Rent::default();
    let account = Account {
        lamports: rent.minimum_balance(0),
        data: vec![],
        owner: solana_sdk::system_program::ID,
        executable: false,
        rent_epoch: 0,
    };

    svm.set_account(CORE_BRIDGE_FEE_COLLECTOR, account).unwrap();
}

/// Set up Wormhole in an existing LiteSVM instance.
///
/// This is a convenience function that:
/// 1. Loads Wormhole programs (Core Bridge + Verify VAA Shim + Post Message Shim)
/// 2. Creates a guardian set account
/// 3. Creates a bridge config account (with full support for message posting)
/// 4. Creates the fee collector account
pub fn setup_wormhole(
    svm: &mut LiteSVM,
    guardians: &TestGuardianSet,
    guardian_set_index: u32,
    config: WormholeProgramsConfig,
) -> Result<WormholeAccounts, WormholeTestError> {
    load_wormhole_programs(svm, config)?;

    let (guardian_set, guardian_set_bump) =
        create_guardian_set_account(svm, guardians, guardian_set_index);

    create_bridge_config(svm, guardian_set_index);
    create_fee_collector(svm);

    Ok(WormholeAccounts {
        guardian_set,
        guardian_set_bump,
    })
}

/// Build guardian set account data.
///
/// Format (from Wormhole core bridge):
/// - index: u32 (4 bytes, little-endian)
/// - keys_len: u32 (4 bytes, little-endian)
/// - keys: [EthAddress; keys_len] where EthAddress is [u8; 20]
/// - creation_time: u32 (4 bytes, little-endian)
/// - expiration_time: u32 (4 bytes, little-endian) - 0 means never expires
pub fn build_guardian_set_data(guardians: &TestGuardianSet, index: u32) -> Vec<u8> {
    let mut data = Vec::new();

    // Guardian set index
    data.extend_from_slice(&index.to_le_bytes());

    // Number of keys
    data.extend_from_slice(&(guardians.len() as u32).to_le_bytes());

    // Guardian Ethereum addresses
    for addr in guardians.eth_addresses() {
        data.extend_from_slice(&addr);
    }

    // Creation time (0 for testing)
    data.extend_from_slice(&0u32.to_le_bytes());

    // Expiration time (0 = never expires)
    data.extend_from_slice(&0u32.to_le_bytes());

    data
}

// =============================================================================
// LiteSVM ↔ SolanaConnection adapter
// =============================================================================

/// Error type for the LiteSVM connection adapter.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct LiteSvmError(pub String);

/// Adapter that implements [`SolanaConnection`] for LiteSVM.
pub struct LiteSvmConnection<'a>(pub &'a mut LiteSVM);

impl SolanaConnection for LiteSvmConnection<'_> {
    type Error = LiteSvmError;

    fn get_latest_blockhash(&self) -> Result<Hash, Self::Error> {
        Ok(self.0.latest_blockhash())
    }

    fn simulate_return_data(&self, tx: &Transaction) -> Result<Option<Vec<u8>>, Self::Error> {
        let result = self
            .0
            .simulate_transaction(tx.clone())
            .map_err(|e| LiteSvmError(format!("Simulation failed: {:?}", e)))?;

        let data = &result.meta.return_data.data;
        if data.is_empty() {
            Ok(None)
        } else {
            Ok(Some(data.clone()))
        }
    }

    fn send_and_confirm(&mut self, tx: &Transaction) -> Result<Signature, Self::Error> {
        self.0
            .send_transaction(tx.clone())
            .map(|_| tx.signatures[0])
            .map_err(|e| LiteSvmError(format!("Transaction failed: {:?}", e)))
    }

    fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, Self::Error> {
        Ok(self.0.get_account(pubkey))
    }
}

// =============================================================================
// Signature posting (delegates to wormhole-svm-submit generic functions)
// =============================================================================

/// Post guardian signatures to the verify VAA shim.
///
/// This creates a new signatures account containing the guardian signatures,
/// which can then be used with `verify_hash` CPI in your program.
///
/// Returns the keypair for the signatures account, which you'll need to close it later.
pub fn post_signatures(
    svm: &mut LiteSVM,
    payer: &Keypair,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Result<PostedSignatures, WormholeTestError> {
    let mut conn = LiteSvmConnection(svm);
    wormhole_svm_submit::signatures::post_signatures(
        &mut conn,
        payer,
        &VERIFY_VAA_SHIM_PROGRAM_ID,
        guardian_set_index,
        signatures,
    )
    .map_err(WormholeTestError::from)
}

/// Close a guardian signatures account to reclaim rent.
///
/// The refund is sent to the specified recipient.
pub fn close_signatures(
    svm: &mut LiteSVM,
    payer: &Keypair,
    signatures_pubkey: &Pubkey,
    refund_recipient: &Pubkey,
) -> Result<(), WormholeTestError> {
    let ix = wormhole_svm_submit::build_close_signatures_ix(
        &VERIFY_VAA_SHIM_PROGRAM_ID,
        signatures_pubkey,
        refund_recipient,
    );

    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);

    svm.send_transaction(tx)
        .map_err(|e| WormholeTestError::LoadError(format!("close_signatures failed: {:?}", e)))?;

    Ok(())
}

/// Execute a closure with posted signatures, automatically handling post and close.
///
/// This is a "bracket" pattern that:
/// 1. Posts the guardian signatures to the verify shim
/// 2. Calls your closure with the signatures account pubkey
/// 3. Closes the signatures account to reclaim rent
///
/// # Example
///
/// ```ignore
/// with_posted_signatures(
///     &mut svm,
///     &payer,
///     0,
///     &signatures,
///     |sigs_pubkey| {
///         // Build and send your transaction that uses verify_hash CPI
///         let ix = build_my_instruction(&sigs_pubkey);
///         let tx = Transaction::new_signed_with_payer(...);
///         svm.send_transaction(tx)
///     },
/// )?;
/// ```
pub fn with_posted_signatures<F, T, E>(
    svm: &mut LiteSVM,
    payer: &Keypair,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
    f: F,
) -> Result<T, WormholeTestError>
where
    F: FnOnce(&mut LiteSVM, &Pubkey) -> Result<T, E>,
    E: std::fmt::Display,
{
    // Step 1: Post signatures
    let posted = post_signatures(svm, payer, guardian_set_index, signatures)?;

    // Step 2: Run user's closure
    let result = f(svm, &posted.pubkey)
        .map_err(|e| WormholeTestError::LoadError(format!("user closure failed: {}", e)))?;

    // Step 3: Close signatures account
    close_signatures(svm, payer, &posted.pubkey, &payer.pubkey())?;

    Ok(result)
}

/// Execute a closure that verifies a VAA, with automatic verification and replay safety checks.
///
/// This helper ensures your program actually verifies VAAs and (optionally) has replay protection:
///
/// 1. **Negative test (on cloned SVM)**: Clones the SVM, posts mismatched signatures,
///    and runs your closure. If it succeeds, your program doesn't verify VAAs -
///    returns `VerificationBypass` error. The clone is discarded, so no state persists.
///
/// 2. **Positive test (on original SVM)**: Posts correct signatures and runs your
///    closure, committing state changes.
///
/// 3. **Replay test (if `NonReplayable`)**: Clones the SVM after success, attempts to
///    run the closure again with the same VAA. If it succeeds, returns `ReplayProtectionMissing`.
///    The clone is discarded on failure, restoring the original successful state.
///
/// The closure receives `(svm, guardian_signatures_pubkey, vaa_body)` where `vaa_body`
/// is just the body bytes (used for digest calculation). Using clone + discard
/// ensures the negative test behaves identically to real execution.
///
/// # Arguments
///
/// * `replay_protection` - Whether to verify replay protection:
///   - `NonReplayable` (default): Verify the operation cannot be replayed
///   - `Replayable`: Skip replay protection check (for idempotent operations)
///
/// # Example
///
/// ```ignore
/// use wormhole_svm_test::{with_vaa, TestVaa, emitter_address_from_20, ReplayProtection};
///
/// let vaa = TestVaa::new(1, emitter_address_from_20([0xAB; 20]), 42, payload);
///
/// let result = with_vaa(
///     &mut svm,
///     &payer,
///     &guardians,
///     0, // guardian_set_index
///     &vaa,
///     ReplayProtection::NonReplayable, // Verify replay protection
///     |svm, sigs_pubkey, vaa_body| {
///         let ix = build_my_verify_instruction(sigs_pubkey, vaa_body);
///         let tx = Transaction::new_signed_with_payer(...);
///         svm.send_transaction(tx).map_err(|e| format!("{:?}", e))
///     },
/// )?;
/// ```
///
/// # See Also
///
/// - [`with_vaa_unchecked`] - Skip all automatic tests (use sparingly)
pub fn with_vaa<F, T, E>(
    svm: &mut LiteSVM,
    payer: &Keypair,
    guardians: &TestGuardianSet,
    guardian_set_index: u32,
    vaa: &crate::TestVaa,
    mut f: F,
) -> Result<T, WormholeTestError>
where
    F: FnMut(&mut LiteSVM, &Pubkey, &[u8]) -> Result<T, E>,
    E: std::fmt::Display,
{
    // Get just the body bytes - this is what the program needs for digest calculation
    let vaa_body = vaa.body();

    // === NEGATIVE TEST (on cloned SVM - discarded after) ===
    // Clone the SVM so any state changes from the negative test don't persist
    let mut svm_clone = svm.clone();

    // Sign a DIFFERENT VAA (modified sequence) - signatures are valid but for wrong data
    let modified_vaa = crate::TestVaa {
        sequence: vaa.sequence.wrapping_add(1),
        ..vaa.clone()
    };
    let wrong_signatures = modified_vaa.guardian_signatures(guardians);

    // Post wrong signatures to the CLONE
    let wrong_posted =
        post_signatures(&mut svm_clone, payer, guardian_set_index, &wrong_signatures)?;

    // Run closure on the clone with ORIGINAL body but WRONG signatures
    // If the program verifies, this should fail (digest won't match)
    let negative_result = f(&mut svm_clone, &wrong_posted.pubkey, &vaa_body);

    // Clone is discarded here (dropped) - no state changes persist

    // If negative test succeeded, the program doesn't verify VAAs!
    if negative_result.is_ok() {
        return Err(WormholeTestError::VerificationBypass(
            "SECURITY: Program accepted VAA with mismatched signatures! \
             This means your program is not actually verifying VAAs. \
             Ensure you call verify_hash CPI before processing the VAA."
                .to_string(),
        ));
    }

    // === NEGATIVE TEST: Wrong emitter chain (on cloned SVM) ===
    if vaa.checks.emitter_chain {
        let mut svm_clone = svm.clone();
        let wrong_chain_vaa = crate::TestVaa {
            emitter_chain: vaa.emitter_chain.wrapping_add(1),
            ..vaa.clone()
        };
        let wrong_chain_body = wrong_chain_vaa.body();
        let wrong_chain_sigs = wrong_chain_vaa.guardian_signatures(guardians);
        let posted = post_signatures(&mut svm_clone, payer, guardian_set_index, &wrong_chain_sigs)?;
        let result = f(&mut svm_clone, &posted.pubkey, &wrong_chain_body);
        if result.is_ok() {
            return Err(WormholeTestError::EmitterChainBypass(
                "SECURITY: Program accepted VAA with wrong emitter chain! \
                 Ensure you validate the emitter_chain field before processing."
                    .to_string(),
            ));
        }
    }

    // === NEGATIVE TEST: Wrong emitter address (on cloned SVM) ===
    if vaa.checks.emitter_address {
        let mut svm_clone = svm.clone();
        let mut wrong_addr = vaa.emitter_address;
        wrong_addr[31] ^= 0xFF;
        let wrong_addr_vaa = crate::TestVaa {
            emitter_address: wrong_addr,
            ..vaa.clone()
        };
        let wrong_addr_body = wrong_addr_vaa.body();
        let wrong_addr_sigs = wrong_addr_vaa.guardian_signatures(guardians);
        let posted = post_signatures(&mut svm_clone, payer, guardian_set_index, &wrong_addr_sigs)?;
        let result = f(&mut svm_clone, &posted.pubkey, &wrong_addr_body);
        if result.is_ok() {
            return Err(WormholeTestError::EmitterAddressBypass(
                "SECURITY: Program accepted VAA with wrong emitter address! \
                 Ensure you validate the emitter_address field before processing."
                    .to_string(),
            ));
        }
    }

    // === POSITIVE TEST (on original SVM - commits state) ===
    let correct_signatures = vaa.guardian_signatures(guardians);
    let posted = post_signatures(svm, payer, guardian_set_index, &correct_signatures)?;

    // Run closure on original SVM with correct signatures
    let result = f(svm, &posted.pubkey, &vaa_body)
        .map_err(|e| WormholeTestError::LoadError(format!("VAA verification failed: {}", e)))?;

    close_signatures(svm, payer, &posted.pubkey, &payer.pubkey())?;

    // === REPLAY TEST (if NonReplayable) ===
    if vaa.checks.replay == crate::ReplayProtection::NonReplayable {
        // Clone the SVM after successful execution
        let mut svm_replay_clone = svm.clone();

        // Post signatures again on the clone
        let replay_posted = post_signatures(
            &mut svm_replay_clone,
            payer,
            guardian_set_index,
            &correct_signatures,
        )?;

        // Try to run the closure again with the same VAA
        let replay_result = f(&mut svm_replay_clone, &replay_posted.pubkey, &vaa_body);

        // Clone is discarded regardless - we only care about the result

        // If replay succeeded, the program lacks replay protection!
        if replay_result.is_ok() {
            return Err(WormholeTestError::ReplayProtectionMissing(
                "SECURITY: Program accepted the same VAA twice! \
                 This means your program lacks replay protection. \
                 Ensure you mark VAAs as used (e.g., via solana-noreplay) \
                 before processing them."
                    .to_string(),
            ));
        }
        // Replay failed as expected - replay protection is working
        // The clone is dropped, original SVM state (after first successful call) is preserved
    }

    Ok(result)
}

/// Execute a closure that verifies a VAA, WITHOUT automatic verification check.
///
/// This is the unchecked version of [`with_vaa`] that skips the automatic negative
/// test. Use this only when you have a specific reason to skip the safety check,
/// such as testing error handling paths or when you need full control over execution.
///
/// **Prefer [`with_vaa`] in most cases** - it automatically ensures your program
/// actually verifies VAAs.
///
/// # Example
///
/// ```ignore
/// // Only use this if you have a specific reason to skip the safety check
/// with_vaa_unchecked(&mut svm, &payer, &guardians, 0, &vaa, |svm, sigs_pubkey, vaa_body| {
///     let tx = Transaction::new_signed_with_payer(...);
///     svm.send_transaction(tx).map_err(|e| format!("{:?}", e))
/// })?;
/// ```
pub fn with_vaa_unchecked<F, T, E>(
    svm: &mut LiteSVM,
    payer: &Keypair,
    guardians: &TestGuardianSet,
    guardian_set_index: u32,
    vaa: &crate::TestVaa,
    f: F,
) -> Result<T, WormholeTestError>
where
    F: FnOnce(&mut LiteSVM, &Pubkey, &[u8]) -> Result<T, E>,
    E: std::fmt::Display,
{
    let vaa_body = vaa.body();
    let signatures = vaa.guardian_signatures(guardians);

    let posted = post_signatures(svm, payer, guardian_set_index, &signatures)?;

    let result = f(svm, &posted.pubkey, &vaa_body)
        .map_err(|e| WormholeTestError::LoadError(format!("closure failed: {}", e)))?;

    close_signatures(svm, payer, &posted.pubkey, &payer.pubkey())?;

    Ok(result)
}

/// Build a post_signatures instruction without sending it.
///
/// Useful if you need to combine this with other instructions in a single transaction.
pub fn build_post_signatures_ix(
    payer: &Pubkey,
    guardian_signatures_keypair: &Pubkey,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Instruction {
    wormhole_svm_submit::build_post_signatures_ix(
        payer,
        guardian_signatures_keypair,
        &VERIFY_VAA_SHIM_PROGRAM_ID,
        guardian_set_index,
        signatures,
    )
}

/// Build a close_signatures instruction without sending it.
///
/// Useful if you need to combine this with other instructions in a single transaction.
pub fn build_close_signatures_ix(
    guardian_signatures: &Pubkey,
    refund_recipient: &Pubkey,
) -> Instruction {
    wormhole_svm_submit::build_close_signatures_ix(
        &VERIFY_VAA_SHIM_PROGRAM_ID,
        guardian_signatures,
        refund_recipient,
    )
}

// =============================================================================
// Posted Message Capture and VAA Construction
// =============================================================================

/// Discriminator for the MessageEvent anchor event.
/// This is sha256("event:MessageEvent")[0..8]
const MESSAGE_EVENT_DISCRIMINATOR: [u8; 8] = wormhole_svm_definitions::MESSAGE_EVENT_DISCRIMINATOR;

/// Discriminator for the post_message instruction.
/// This is sha256("global:post_message")[0..8]
const POST_MESSAGE_SELECTOR: [u8; 8] =
    wormhole_svm_shim::post_message::PostMessageShimInstruction::<u8>::POST_MESSAGE_SELECTOR;

// Internal: Parsed post_message instruction data.
#[derive(Clone, Debug)]
struct PostMessageData {
    nonce: u32,
    finality: u8,
    payload: Vec<u8>,
}

impl PostMessageData {
    fn parse(data: &[u8]) -> Option<Self> {
        // Format: 8 (disc) + 4 (nonce) + 1 (finality) + 4 (len) + payload
        if data.len() < 17 || data[..8] != POST_MESSAGE_SELECTOR {
            return None;
        }
        let nonce = u32::from_le_bytes(data[8..12].try_into().ok()?);
        let finality = data[12];
        let payload_len = u32::from_le_bytes(data[13..17].try_into().ok()?) as usize;
        if data.len() < 17 + payload_len {
            return None;
        }
        Some(Self {
            nonce,
            finality,
            payload: data[17..17 + payload_len].to_vec(),
        })
    }
}

// Internal: Parsed MessageEvent from the Post Message Shim's self-CPI.
#[derive(Clone, Debug)]
struct MessageEvent {
    emitter: Pubkey,
    sequence: u64,
    submission_time: u32,
}

impl MessageEvent {
    fn parse(data: &[u8]) -> Option<Self> {
        // Format: 8 (cpi disc) + 8 (event disc) + 32 (emitter) + 8 (seq) + 4 (time)
        if data.len() < 60 || data[8..16] != MESSAGE_EVENT_DISCRIMINATOR {
            return None;
        }
        let event_data = &data[16..];
        Some(Self {
            emitter: Pubkey::new_from_array(event_data[..32].try_into().ok()?),
            sequence: u64::from_le_bytes(event_data[32..40].try_into().ok()?),
            submission_time: u32::from_le_bytes(event_data[40..44].try_into().ok()?),
        })
    }
}

/// Information about a posted Wormhole message.
///
/// This struct captures all the data needed to construct a VAA from a
/// message that was posted via the Wormhole Post Message Shim.
///
/// It combines the MessageEvent (captured from CPI) with the original
/// call parameters (payload, nonce, finality).
#[derive(Clone, Debug)]
pub struct PostedMessageInfo {
    /// The emitter address (the PDA that signed the message).
    pub emitter: Pubkey,
    /// The emitter chain ID (1 for Solana).
    pub emitter_chain: u16,
    /// The sequence number of the message.
    pub sequence: u64,
    /// The message payload.
    pub payload: Vec<u8>,
    /// The nonce.
    pub nonce: u32,
    /// The consistency level (finality).
    pub consistency_level: u8,
    /// The timestamp (submission time from the event).
    pub timestamp: u32,
}

impl PostedMessageInfo {
    // Internal: Create from parsed event and post_message data.
    fn from_event(
        event: &MessageEvent,
        payload: Vec<u8>,
        nonce: u32,
        consistency_level: u8,
    ) -> Self {
        Self {
            emitter: event.emitter,
            emitter_chain: 1, // Solana
            sequence: event.sequence,
            payload,
            nonce,
            consistency_level,
            timestamp: event.submission_time,
        }
    }

    /// Convert this posted message to a TestVaa.
    ///
    /// The resulting TestVaa can be signed by guardians to produce a verifiable VAA.
    pub fn to_test_vaa(&self) -> crate::TestVaa {
        crate::TestVaa {
            emitter_chain: self.emitter_chain,
            emitter_address: self.emitter.to_bytes(),
            sequence: self.sequence,
            payload: self.payload.clone(),
            timestamp: self.timestamp,
            nonce: self.nonce,
            consistency_level: self.consistency_level,
            guardian_set_index: 0,
            checks: Default::default(),
        }
    }
}

/// Extract all Wormhole messages from transaction inner instructions.
///
/// A single transaction can emit multiple Wormhole messages. This function extracts
/// all of them by parsing:
/// - The `post_message` CPI instructions (for nonce, finality, payload)
/// - The `MessageEvent` self-CPI events (for emitter, sequence, timestamp)
///
/// These are paired up in order - the Nth post_message corresponds to the Nth MessageEvent.
///
/// # Example
///
/// ```ignore
/// // Send a transaction that emits one or more Wormhole messages
/// let tx_meta = svm.send_transaction(tx)?;
///
/// // Extract all messages
/// let messages = extract_posted_message_info_from_tx(&tx_meta);
///
/// for message_info in messages {
///     let vaa = message_info.to_test_vaa();
///     let signed_vaa = vaa.sign(&guardians);
/// }
/// ```
///
/// Returns an empty Vec if no messages are found.
pub fn extract_posted_message_info_from_tx(
    meta: &litesvm::types::TransactionMetadata,
) -> Vec<PostedMessageInfo> {
    // Collect all post_message instructions and MessageEvents from inner instructions
    let mut post_messages = Vec::new();
    let mut events = Vec::new();

    for inner_list in &meta.inner_instructions {
        for inner in inner_list {
            if let Some(data) = PostMessageData::parse(&inner.instruction.data) {
                post_messages.push(data);
            }
            if let Some(event) = MessageEvent::parse(&inner.instruction.data) {
                events.push(event);
            }
        }
    }

    // Pair them up - they should appear in the same order
    post_messages
        .into_iter()
        .zip(events)
        .map(|(post_msg, event)| {
            PostedMessageInfo::from_event(
                &event,
                post_msg.payload,
                post_msg.nonce,
                post_msg.finality,
            )
        })
        .collect()
}

/// Read the current sequence number for an emitter from its sequence account.
///
/// Returns `None` if the sequence account doesn't exist yet (first message not posted).
pub fn read_emitter_sequence(svm: &LiteSVM, emitter: &Pubkey) -> Option<u64> {
    use wormhole_svm_definitions::find_emitter_sequence_address;

    let (sequence_addr, _) = find_emitter_sequence_address(emitter, &CORE_BRIDGE_PROGRAM_ID);
    let account = svm.get_account(&sequence_addr)?;

    // Sequence account data is just a u64 (little-endian)
    if account.data.len() >= 8 {
        Some(u64::from_le_bytes(account.data[0..8].try_into().ok()?))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestGuardian;
    use std::path::Path;

    #[test]
    fn test_guardian_set_data_structure() {
        let guardians = TestGuardianSet::single(TestGuardian::default());
        let data = build_guardian_set_data(&guardians, 0);

        // Index (4) + Len (4) + 1 address (20) + Creation (4) + Expiration (4) = 36
        assert_eq!(data.len(), 36);

        // Check index
        let index = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(index, 0);

        // Check length
        let len = u32::from_le_bytes(data[4..8].try_into().unwrap());
        assert_eq!(len, 1);
    }

    #[test]
    fn test_multi_guardian_set_data() {
        let guardians = TestGuardianSet::generate(3, 789);
        let data = build_guardian_set_data(&guardians, 5);

        // Index (4) + Len (4) + 3 addresses (60) + Creation (4) + Expiration (4) = 76
        assert_eq!(data.len(), 76);

        // Check index
        let index = u32::from_le_bytes(data[0..4].try_into().unwrap());
        assert_eq!(index, 5);

        // Check length
        let len = u32::from_le_bytes(data[4..8].try_into().unwrap());
        assert_eq!(len, 3);
    }

    #[test]
    fn test_search_paths_includes_env_var() {
        std::env::set_var("WORMHOLE_FIXTURES_DIR", "/custom/path");
        let paths = search_paths();
        assert!(paths.iter().any(|p| p == Path::new("/custom/path")));
        std::env::remove_var("WORMHOLE_FIXTURES_DIR");
    }

    #[cfg(feature = "bundled-fixtures")]
    #[test]
    fn test_setup_wormhole_with_bundled_fixtures() {
        let mut svm = LiteSVM::new();
        let guardians = TestGuardianSet::single(TestGuardian::default());

        let result = setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default());

        assert!(result.is_ok(), "setup_wormhole failed: {:?}", result.err());

        let accounts = result.unwrap();

        // Verify guardian set account was created
        let guardian_set_account = svm.get_account(&accounts.guardian_set);
        assert!(
            guardian_set_account.is_some(),
            "Guardian set account not found"
        );

        // Verify bridge config was created
        let config_account = svm.get_account(&CORE_BRIDGE_CONFIG);
        assert!(config_account.is_some(), "Bridge config account not found");

        // Verify programs were loaded
        let shim_account = svm.get_account(&VERIFY_VAA_SHIM_PROGRAM_ID);
        assert!(shim_account.is_some(), "Verify VAA shim not loaded");

        let bridge_account = svm.get_account(&CORE_BRIDGE_PROGRAM_ID);
        assert!(bridge_account.is_some(), "Core bridge not loaded");

        let post_shim_account = svm.get_account(&POST_MESSAGE_SHIM_PROGRAM_ID);
        assert!(post_shim_account.is_some(), "Post message shim not loaded");
    }

    #[cfg(feature = "bundled-fixtures")]
    #[test]
    fn test_bundled_fixtures_are_valid_elf() {
        // Verify the bundled fixtures are valid ELF binaries
        // ELF magic number: 0x7f 'E' 'L' 'F'
        assert_eq!(&VERIFY_VAA_SHIM_BYTES[0..4], &[0x7f, b'E', b'L', b'F']);
        assert_eq!(&CORE_BRIDGE_BYTES[0..4], &[0x7f, b'E', b'L', b'F']);
        assert_eq!(&POST_MESSAGE_SHIM_BYTES[0..4], &[0x7f, b'E', b'L', b'F']);
    }

    #[cfg(feature = "bundled-fixtures")]
    #[test]
    fn test_post_and_close_signatures() {
        use crate::TestVaa;

        let mut svm = LiteSVM::new();
        let guardians = TestGuardianSet::single(TestGuardian::default());
        let payer = Keypair::new();

        // Fund payer
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        // Setup wormhole
        setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();

        // Create a test VAA and get signatures
        let vaa = TestVaa::new(1, [0xAB; 32], 42, vec![1, 2, 3, 4]);
        let signatures = vaa.guardian_signatures(&guardians);

        // Convert to array format
        let sig_arrays: Vec<[u8; 66]> = signatures;

        // Post signatures
        let posted = post_signatures(&mut svm, &payer, 0, &sig_arrays).unwrap();

        // Verify signatures account exists
        let sigs_account = svm.get_account(&posted.pubkey);
        assert!(sigs_account.is_some(), "Signatures account should exist");

        // Close signatures
        close_signatures(&mut svm, &payer, &posted.pubkey, &payer.pubkey()).unwrap();

        // Verify signatures account is closed
        let sigs_account = svm.get_account(&posted.pubkey);
        assert!(
            sigs_account.is_none(),
            "Signatures account should be closed"
        );
    }

    #[cfg(feature = "bundled-fixtures")]
    #[test]
    fn test_with_posted_signatures_bracket() {
        use crate::TestVaa;

        let mut svm = LiteSVM::new();
        let guardians = TestGuardianSet::single(TestGuardian::default());
        let payer = Keypair::new();

        // Fund payer
        svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();

        // Setup wormhole
        setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();

        // Create a test VAA and get signatures
        let vaa = TestVaa::new(1, [0xAB; 32], 42, vec![1, 2, 3, 4]);
        let signatures = vaa.guardian_signatures(&guardians);
        let sig_arrays: Vec<[u8; 66]> = signatures;

        // Track the signatures pubkey for verification after closure
        let mut captured_pubkey: Option<Pubkey> = None;

        // Use bracket pattern
        let result = with_posted_signatures(
            &mut svm,
            &payer,
            0,
            &sig_arrays,
            |svm, sigs_pubkey| -> Result<(), &'static str> {
                // Capture the pubkey for later verification
                captured_pubkey = Some(*sigs_pubkey);

                // Verify the signatures account exists inside the closure
                let account = svm.get_account(sigs_pubkey);
                if account.is_some() {
                    Ok(())
                } else {
                    Err("signatures account not found inside closure")
                }
            },
        );

        assert!(
            result.is_ok(),
            "with_posted_signatures failed: {:?}",
            result
        );

        // Verify the signatures account was closed after the bracket
        let pubkey = captured_pubkey.expect("pubkey should have been captured");
        let account = svm.get_account(&pubkey);
        assert!(
            account.is_none(),
            "Signatures account should be closed after bracket"
        );
    }

    // Note: with_vaa, with_vaa_unchecked, and message emission are tested in
    // integration tests (tests/verify_vaa_example.rs and tests/emit_message_example.rs).

    #[test]
    fn test_message_event_parsing() {
        // Test parsing MessageEvent from raw bytes
        let emitter = Pubkey::new_unique();
        let sequence = 42u64;
        let submission_time = 1234567890u32;

        // Build the event data manually in the Anchor self-CPI format:
        // - 8 bytes: Anchor self-CPI instruction discriminator (any value)
        // - 8 bytes: MessageEvent event discriminator
        // - borsh-encoded event data
        let mut data = Vec::new();
        // Outer CPI discriminator (any 8 bytes - Anchor's internal discriminator)
        data.extend_from_slice(&[228, 69, 165, 46, 81, 203, 154, 29]);
        // MessageEvent discriminator
        data.extend_from_slice(&MESSAGE_EVENT_DISCRIMINATOR);
        // Event data
        data.extend_from_slice(&emitter.to_bytes());
        data.extend_from_slice(&sequence.to_le_bytes());
        data.extend_from_slice(&submission_time.to_le_bytes());

        // Parse it back
        let event = MessageEvent::parse(&data).expect("should parse");
        assert_eq!(event.emitter, emitter);
        assert_eq!(event.sequence, sequence);
        assert_eq!(event.submission_time, submission_time);

        // Test with wrong event discriminator (corrupt bytes 8-15)
        let mut bad_data = data.clone();
        bad_data[8] = 0xFF;
        assert!(MessageEvent::parse(&bad_data).is_none());

        // Test with truncated data
        assert!(MessageEvent::parse(&data[..20]).is_none());
    }

    #[test]
    fn test_post_message_data_parsing() {
        // Test parsing PostMessageData from raw instruction data
        let nonce = 12345u32;
        let finality = 1u8;
        let payload = b"Test payload for parsing";

        // Build the instruction data manually:
        // - 8 bytes: POST_MESSAGE_SELECTOR discriminator
        // - 4 bytes: nonce (LE)
        // - 1 byte: finality
        // - 4 bytes: payload_len (LE)
        // - N bytes: payload
        let mut data = Vec::new();
        data.extend_from_slice(&POST_MESSAGE_SELECTOR);
        data.extend_from_slice(&nonce.to_le_bytes());
        data.push(finality);
        data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend_from_slice(payload);

        // Parse it back
        let parsed = PostMessageData::parse(&data).expect("should parse");
        assert_eq!(parsed.nonce, nonce);
        assert_eq!(parsed.finality, finality);
        assert_eq!(parsed.payload, payload);

        // Test with wrong discriminator
        let mut bad_data = data.clone();
        bad_data[0] = 0xFF;
        assert!(PostMessageData::parse(&bad_data).is_none());

        // Test with truncated data (no payload)
        assert!(PostMessageData::parse(&data[..17]).is_none());

        // Test with truncated header
        assert!(PostMessageData::parse(&data[..10]).is_none());
    }
}
