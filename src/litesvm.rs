//! LiteSVM helpers for setting up Wormhole test environments.

use std::path::PathBuf;

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::Instruction,
    pubkey::Pubkey,
    rent::Rent,
    signature::{Keypair, Signer},
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
use wormhole_svm_shim::verify_vaa::{
    CloseSignatures, CloseSignaturesAccounts, PostSignatures, PostSignaturesAccounts,
    PostSignaturesData,
};

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
}

/// Configuration for loading Wormhole programs.
pub struct WormholeProgramsConfig {
    /// Path to verify_vaa_shim.so (or None to search default locations).
    pub verify_vaa_shim: Option<PathBuf>,
    /// Path to core_bridge.so (or None to search default locations).
    pub core_bridge: Option<PathBuf>,
    /// Path to post_message_shim.so (or None to search default locations).
    pub post_message_shim: Option<PathBuf>,
}

impl Default for WormholeProgramsConfig {
    fn default() -> Self {
        Self {
            verify_vaa_shim: None,
            core_bridge: None,
            post_message_shim: None,
        }
    }
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

/// Create a minimal bridge config account in LiteSVM.
pub fn create_bridge_config(svm: &mut LiteSVM, guardian_set_index: u32) {
    // Minimal bridge config data structure:
    // - guardian_set_index: u32 (4 bytes, little-endian)
    // - guardian_set_expiration_time: u32 (4 bytes)
    // - fee: u64 (8 bytes)
    let mut data = Vec::new();
    data.extend_from_slice(&guardian_set_index.to_le_bytes());
    data.extend_from_slice(&86400u32.to_le_bytes()); // 24 hour expiration
    data.extend_from_slice(&0u64.to_le_bytes()); // no fee

    let rent = Rent::default();
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

/// Set up Wormhole in an existing LiteSVM instance.
///
/// This is a convenience function that:
/// 1. Loads Wormhole programs (Core Bridge + Verify VAA Shim)
/// 2. Creates a guardian set account
/// 3. Creates a bridge config account
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

/// Result of posting signatures to the verify shim.
pub struct PostedSignatures {
    /// The keypair for the signatures account (needed for signing close transaction).
    pub keypair: Keypair,
    /// The public key of the signatures account.
    pub pubkey: Pubkey,
}

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
    let guardian_sigs_keypair = Keypair::new();

    let ix = PostSignatures {
        program_id: &VERIFY_VAA_SHIM_PROGRAM_ID,
        accounts: PostSignaturesAccounts {
            payer: &payer.pubkey(),
            guardian_signatures: &guardian_sigs_keypair.pubkey(),
        },
        data: PostSignaturesData::new(guardian_set_index, signatures.len() as u8, signatures),
    }
    .instruction();

    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, &guardian_sigs_keypair],
        blockhash,
    );

    svm.send_transaction(tx)
        .map_err(|e| WormholeTestError::LoadError(format!("post_signatures failed: {:?}", e)))?;

    Ok(PostedSignatures {
        pubkey: guardian_sigs_keypair.pubkey(),
        keypair: guardian_sigs_keypair,
    })
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
    let ix = CloseSignatures {
        program_id: &VERIFY_VAA_SHIM_PROGRAM_ID,
        accounts: CloseSignaturesAccounts {
            guardian_signatures: signatures_pubkey,
            refund_recipient,
        },
    }
    .instruction();

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

/// Build a post_signatures instruction without sending it.
///
/// Useful if you need to combine this with other instructions in a single transaction.
pub fn build_post_signatures_ix(
    payer: &Pubkey,
    guardian_signatures_keypair: &Pubkey,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Instruction {
    PostSignatures {
        program_id: &VERIFY_VAA_SHIM_PROGRAM_ID,
        accounts: PostSignaturesAccounts {
            payer,
            guardian_signatures: guardian_signatures_keypair,
        },
        data: PostSignaturesData::new(guardian_set_index, signatures.len() as u8, signatures),
    }
    .instruction()
}

/// Build a close_signatures instruction without sending it.
///
/// Useful if you need to combine this with other instructions in a single transaction.
pub fn build_close_signatures_ix(
    guardian_signatures: &Pubkey,
    refund_recipient: &Pubkey,
) -> Instruction {
    CloseSignatures {
        program_id: &VERIFY_VAA_SHIM_PROGRAM_ID,
        accounts: CloseSignaturesAccounts {
            guardian_signatures,
            refund_recipient,
        },
    }
    .instruction()
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

        assert!(result.is_ok(), "with_posted_signatures failed: {:?}", result);

        // Verify the signatures account was closed after the bracket
        let pubkey = captured_pubkey.expect("pubkey should have been captured");
        let account = svm.get_account(&pubkey);
        assert!(
            account.is_none(),
            "Signatures account should be closed after bracket"
        );
    }
}
