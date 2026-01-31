//! Integration test demonstrating end-to-end VAA verification.
//!
//! This test shows the complete flow of:
//! 1. Setting up a LiteSVM environment with Wormhole programs
//! 2. Creating test guardians and building VAA body + signatures
//! 3. Posting guardian signatures to the verify shim
//! 4. Calling a program that verifies the VAA body via CPI
//! 5. Closing the signatures account to reclaim rent

#![cfg(feature = "bundled-fixtures")]

use litesvm::LiteSVM;
use solana_sdk::{
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use std::path::Path;
use wormhole_svm_test::{
    close_signatures, emitter_address_from_20, post_signatures, setup_wormhole, TestGuardian,
    TestGuardianSet, TestVaa, WormholeProgramsConfig,
};

const GUARDIAN_SET_INDEX: u32 = 0;

/// Find and load the vaa-verifier-example program binary.
fn load_example_program(svm: &mut LiteSVM) {
    // Search paths relative to workspace root and crate directories
    let search_paths = [
        "target/deploy",
        "../target/deploy",
        "../../target/deploy",
        "../../../target/deploy",
    ];

    for base in &search_paths {
        let path = format!("{}/vaa_verifier_example.so", base);
        if Path::new(&path).exists() {
            let program_bytes = std::fs::read(&path).expect("Failed to read example program");
            svm.add_program(vaa_verifier_example::ID, &program_bytes)
                .expect("Failed to load example program");
            println!(
                "Loaded vaa_verifier_example at {}",
                vaa_verifier_example::ID
            );
            return;
        }
    }

    panic!(
        "vaa_verifier_example.so not found. Build it with: \
        cd programs/vaa-verifier-example && cargo build-sbf"
    );
}

#[test]
fn test_end_to_end_vaa_verification() {
    // Step 1: Create LiteSVM environment
    let mut svm = LiteSVM::new();
    let payer = Keypair::new();

    // Fund the payer
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    // Step 2: Create test guardians
    let guardians = TestGuardianSet::single(TestGuardian::default());

    // Step 3: Set up Wormhole programs and accounts
    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    println!("Wormhole setup complete:");
    println!("  Guardian set: {}", wormhole.guardian_set);
    println!("  Guardian set bump: {}", wormhole.guardian_set_bump);

    // Step 4: Load the example program
    load_example_program(&mut svm);

    // Step 5: Create a test VAA
    let test_payload = b"Hello, Wormhole!".to_vec();
    let emitter = emitter_address_from_20([0xAB; 20]);
    let vaa = TestVaa::new(
        1, // Solana chain ID
        emitter,
        42, // sequence
        test_payload,
    );

    // Get VAA body (just the body bytes, used for digest calculation)
    let vaa_body = vaa.body();
    println!("VAA body length: {}", vaa_body.len());

    // Get guardian signatures for post_signatures
    let guardian_signatures = vaa.guardian_signatures(&guardians);
    println!("Number of signatures: {}", guardian_signatures.len());

    // Step 6: Post signatures to the verify shim
    let posted = post_signatures(&mut svm, &payer, GUARDIAN_SET_INDEX, &guardian_signatures)
        .expect("post_signatures failed");

    println!("Posted signatures to: {}", posted.pubkey);

    // Step 7: Call the example program to verify the VAA body
    let verify_ix = vaa_verifier_example::build_verify_vaa_instruction(
        &payer.pubkey(),
        &wormhole.guardian_set,
        &posted.pubkey,
        wormhole.guardian_set_bump,
        &vaa_body,
    );

    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(
        &[verify_ix],
        Some(&payer.pubkey()),
        &[&payer],
        blockhash,
    );

    let result = svm.send_transaction(tx);
    assert!(
        result.is_ok(),
        "VAA verification failed: {:?}",
        result.err()
    );

    println!("VAA verified successfully!");

    // Step 8: Close signatures account to reclaim rent
    close_signatures(&mut svm, &payer, &posted.pubkey, &payer.pubkey())
        .expect("close_signatures failed");

    println!("Signatures account closed.");
    println!("Test complete!");
}

/// Test using the with_vaa bracket helper (recommended approach).
///
/// This is the cleanest API - just provide the VAA and let the helper
/// handle signing, posting, verification safety check, and cleanup.
#[test]
fn test_with_vaa_helper() {
    use wormhole_svm_test::{with_vaa, ReplayProtection};

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let guardians = TestGuardianSet::single(TestGuardian::default());

    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    load_example_program(&mut svm);

    // Create the VAA
    let vaa = TestVaa::new(
        1,
        emitter_address_from_20([0xEF; 20]),
        999,
        b"with_vaa helper test".to_vec(),
    );

    // with_vaa:
    // 1. Clones SVM, runs with wrong signatures (should fail - verifies program checks)
    // 2. Runs on original SVM with correct signatures (should succeed)
    // Note: Using Replayable since vaa-verifier-example doesn't have replay protection
    let result = with_vaa(
        &mut svm,
        &payer,
        &guardians,
        GUARDIAN_SET_INDEX,
        &vaa,
        ReplayProtection::Replayable, // Example program doesn't have replay protection
        |svm, sigs_pubkey, vaa_body| {
            let verify_ix = vaa_verifier_example::build_verify_vaa_instruction(
                &payer.pubkey(),
                &wormhole.guardian_set,
                sigs_pubkey,
                wormhole.guardian_set_bump,
                vaa_body,
            );

            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[verify_ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );

            svm.send_transaction(tx)
                .map_err(|e| format!("tx failed: {:?}", e))
        },
    );

    assert!(result.is_ok(), "with_vaa test failed: {:?}", result);
    println!("with_vaa helper test complete!");
}

/// Test that with_vaa catches programs that skip VAA verification.
///
/// This test uses the insecure `skip_verify` instruction which parses
/// the VAA body but does NOT call the verify_hash CPI. The `with_vaa` helper
/// should detect this and return a VerificationBypass error.
#[test]
fn test_with_vaa_catches_unverified_program() {
    use wormhole_svm_test::{with_vaa, ReplayProtection, WormholeTestError};

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let guardians = TestGuardianSet::single(TestGuardian::default());

    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    load_example_program(&mut svm);

    let vaa = TestVaa::new(
        1,
        emitter_address_from_20([0xBA; 20]),
        777,
        b"This VAA will not be verified!".to_vec(),
    );

    // Use the INSECURE skip_verify instruction
    // Note: Using Replayable since we expect VerificationBypass error before replay check
    let result = with_vaa(
        &mut svm,
        &payer,
        &guardians,
        GUARDIAN_SET_INDEX,
        &vaa,
        ReplayProtection::Replayable,
        |svm, sigs_pubkey, vaa_body| {
            // Use the insecure instruction that skips verification
            let skip_ix = vaa_verifier_example::build_skip_verify_instruction(
                &payer.pubkey(),
                &wormhole.guardian_set,
                sigs_pubkey,
                wormhole.guardian_set_bump,
                vaa_body,
            );

            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[skip_ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );

            svm.send_transaction(tx)
                .map_err(|e| format!("tx failed: {:?}", e))
        },
    );

    // with_vaa should detect the bypass with the specific error variant
    let err = result.expect_err("with_vaa should have detected verification bypass");

    assert!(
        matches!(err, WormholeTestError::VerificationBypass(_)),
        "Expected VerificationBypass error, got: {:?}",
        err
    );

    println!("Correctly caught: {}", err);
    println!("with_vaa correctly detected the insecure program!");
}

/// Test that with_vaa catches programs that lack replay protection.
///
/// The vaa-verifier-example program correctly verifies VAA signatures but
/// does NOT implement replay protection. When using `ReplayProtection::NonReplayable`,
/// `with_vaa` should detect that the same VAA can be processed twice and return
/// a `ReplayProtectionMissing` error.
#[test]
fn test_with_vaa_catches_missing_replay_protection() {
    use wormhole_svm_test::{with_vaa, ReplayProtection, WormholeTestError};

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let guardians = TestGuardianSet::single(TestGuardian::default());

    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    load_example_program(&mut svm);

    let vaa = TestVaa::new(
        1,
        emitter_address_from_20([0xDE; 20]),
        555,
        b"This VAA should not be replayable!".to_vec(),
    );

    // Use NonReplayable to verify replay protection
    // The vaa-verifier-example program does NOT have replay protection,
    // so with_vaa should detect this and fail
    let result = with_vaa(
        &mut svm,
        &payer,
        &guardians,
        GUARDIAN_SET_INDEX,
        &vaa,
        ReplayProtection::NonReplayable,
        |svm, sigs_pubkey, vaa_body| {
            let verify_ix = vaa_verifier_example::build_verify_vaa_instruction(
                &payer.pubkey(),
                &wormhole.guardian_set,
                sigs_pubkey,
                wormhole.guardian_set_bump,
                vaa_body,
            );

            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[verify_ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );

            svm.send_transaction(tx)
                .map_err(|e| format!("tx failed: {:?}", e))
        },
    );

    // with_vaa should detect the missing replay protection with the specific error variant
    let err = result.expect_err("with_vaa should have detected missing replay protection");

    assert!(
        matches!(err, WormholeTestError::ReplayProtectionMissing(_)),
        "Expected ReplayProtectionMissing error, got: {:?}",
        err
    );

    println!("Correctly caught: {}", err);
    println!("with_vaa correctly detected the program lacks replay protection!");
}

/// Test using the with_posted_signatures bracket helper (lower-level).
#[test]
fn test_with_posted_signatures_pattern() {
    use wormhole_svm_test::with_posted_signatures;

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let guardians = TestGuardianSet::single(TestGuardian::default());

    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    load_example_program(&mut svm);

    let vaa = TestVaa::new(
        1,
        emitter_address_from_20([0xCD; 20]),
        123,
        b"Bracket pattern test".to_vec(),
    );

    let vaa_body = vaa.body();
    let guardian_signatures = vaa.guardian_signatures(&guardians);

    // Use the bracket pattern - signatures are automatically posted and closed
    let result = with_posted_signatures(
        &mut svm,
        &payer,
        GUARDIAN_SET_INDEX,
        &guardian_signatures,
        |svm, sigs_pubkey| -> Result<(), &'static str> {
            let verify_ix = vaa_verifier_example::build_verify_vaa_instruction(
                &payer.pubkey(),
                &wormhole.guardian_set,
                sigs_pubkey,
                wormhole.guardian_set_bump,
                &vaa_body,
            );

            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[verify_ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );

            svm.send_transaction(tx)
                .map_err(|_| "VAA verification failed")?;

            Ok(())
        },
    );

    assert!(result.is_ok(), "Bracket pattern test failed: {:?}", result);
    println!("Bracket pattern test complete!");
}

/// Test the full emit → capture → verify cycle.
///
/// This demonstrates the complete guardian workflow:
/// 1. A program emits a Wormhole message via the Post Message Shim
/// 2. We capture the MessageEvent from the transaction's inner instructions
/// 3. We construct a VAA from the captured event (simulating guardian signing)
/// 4. Another program verifies that VAA
///
/// This is the core workflow for cross-chain messaging: messages are emitted
/// on the source chain, guardians observe and sign them, and the resulting
/// VAAs are verified on the destination chain.
///
/// NOTE: This test manually loads the message-emitter-example program to demonstrate
/// the generic `extract_posted_message_info_from_tx` API that integrators should use
/// with their own programs. This single function extracts both the MessageEvent and
/// the post_message instruction data (payload, nonce, finality) automatically.
#[test]
fn test_emit_capture_verify_roundtrip() {
    use solana_sdk::instruction::{AccountMeta, Instruction};
    use solana_sdk::pubkey::Pubkey;
    use wormhole_svm_definitions::{
        find_core_bridge_config_address, find_emitter_sequence_address,
        find_event_authority_address, find_fee_collector_address, find_shim_message_address,
        solana::mainnet::{CORE_BRIDGE_PROGRAM_ID, POST_MESSAGE_SHIM_PROGRAM_ID},
    };
    use wormhole_svm_test::{extract_posted_message_info_from_tx, with_posted_signatures};

    // Message emitter example program ID (from the program's declare_id!)
    const MESSAGE_EMITTER_ID: Pubkey =
        solana_sdk::pubkey!("26g7Z38n86MGtturwtHuWKG3hr4QhvnaBfinaFKVaz4x");

    // Helper to find emitter PDA for the message-emitter-example program
    fn find_emitter_address() -> (Pubkey, u8) {
        Pubkey::find_program_address(&[b"emitter"], &MESSAGE_EMITTER_ID)
    }

    // Helper to load the message-emitter-example program
    fn load_message_emitter(svm: &mut LiteSVM) {
        let search_paths = [
            "target/deploy",
            "../target/deploy",
            "../../target/deploy",
            "../../../target/deploy",
        ];
        for base in &search_paths {
            let path = format!("{}/message_emitter_example.so", base);
            if Path::new(&path).exists() {
                let bytes = std::fs::read(&path).expect("read program");
                svm.add_program(MESSAGE_EMITTER_ID, &bytes)
                    .expect("load program");
                return;
            }
        }
        panic!("message_emitter_example.so not found");
    }

    // Helper to build emit_message instruction
    fn build_emit_ix(payer: &Pubkey, nonce: u32, finality: u8, payload: &[u8]) -> Instruction {
        let (emitter, _) = find_emitter_address();
        let (core_bridge_config, _) = find_core_bridge_config_address(&CORE_BRIDGE_PROGRAM_ID);
        let (message, _) = find_shim_message_address(&emitter, &POST_MESSAGE_SHIM_PROGRAM_ID);
        let (sequence, _) = find_emitter_sequence_address(&emitter, &CORE_BRIDGE_PROGRAM_ID);
        let (fee_collector, _) = find_fee_collector_address(&CORE_BRIDGE_PROGRAM_ID);
        let (event_authority, _) = find_event_authority_address(&POST_MESSAGE_SHIM_PROGRAM_ID);

        let mut data = Vec::with_capacity(9 + payload.len());
        data.extend_from_slice(&nonce.to_le_bytes());
        data.push(finality);
        data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        data.extend_from_slice(payload);

        Instruction {
            program_id: MESSAGE_EMITTER_ID,
            accounts: vec![
                AccountMeta::new(core_bridge_config, false),
                AccountMeta::new(message, false),
                AccountMeta::new_readonly(emitter, false),
                AccountMeta::new(sequence, false),
                AccountMeta::new(*payer, true),
                AccountMeta::new(fee_collector, false),
                AccountMeta::new_readonly(solana_sdk::sysvar::clock::id(), false),
                AccountMeta::new_readonly(solana_sdk::system_program::id(), false),
                AccountMeta::new_readonly(CORE_BRIDGE_PROGRAM_ID, false),
                AccountMeta::new_readonly(event_authority, false),
                AccountMeta::new_readonly(POST_MESSAGE_SHIM_PROGRAM_ID, false),
            ],
            data,
        }
    }

    let mut svm = LiteSVM::new();
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();

    let guardians = TestGuardianSet::single(TestGuardian::default());

    // Step 1: Set up Wormhole programs and accounts
    let wormhole = setup_wormhole(
        &mut svm,
        &guardians,
        GUARDIAN_SET_INDEX,
        WormholeProgramsConfig::default(),
    )
    .expect("Failed to setup Wormhole");

    // Step 2: Load both example programs
    load_example_program(&mut svm); // vaa-verifier-example
    load_message_emitter(&mut svm); // message-emitter-example

    println!("=== Emit → Capture → Verify Round-trip Test ===\n");

    // Step 3: Emit a Wormhole message via the Post Message Shim
    let payload = b"Cross-chain message from Solana!";
    let nonce = 12345u32;
    let finality = 1u8; // Confirmed

    println!("Step 1: Emitting message via Post Message Shim...");
    let emit_ix = build_emit_ix(&payer.pubkey(), nonce, finality, payload);
    let blockhash = svm.latest_blockhash();
    let tx =
        Transaction::new_signed_with_payer(&[emit_ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let tx_meta = svm.send_transaction(tx).expect("emit should succeed");

    // Step 4: Capture everything automatically using the combined extraction helper
    // This extracts both the MessageEvent AND the post_message instruction data
    // (payload, nonce, finality) from the transaction's inner instructions
    let message_info = extract_posted_message_info_from_tx(&tx_meta)
        .into_iter()
        .next()
        .expect("PostedMessageInfo should be extractable from transaction");

    println!("  Captured from CPI inner instructions:");
    println!("    Emitter: {}", message_info.emitter);
    println!("    Sequence: {}", message_info.sequence);
    println!(
        "    Payload: {:?}",
        String::from_utf8_lossy(&message_info.payload)
    );
    println!("    Nonce: {}", message_info.nonce);
    println!("    Finality: {}", message_info.consistency_level);

    // Verify the emitter is the expected PDA
    let (expected_emitter, _) = find_emitter_address();
    assert_eq!(message_info.emitter, expected_emitter);

    // Verify the extracted values match what we sent
    assert_eq!(message_info.payload, payload);
    assert_eq!(message_info.nonce, nonce);
    assert_eq!(message_info.consistency_level, finality);

    // Step 5: Construct a VAA from the captured info (simulating guardian signing)
    println!("\nStep 2: Constructing VAA from captured message info...");
    let test_vaa = message_info.to_test_vaa();

    println!("  VAA body:");
    println!("    Emitter chain: {}", test_vaa.emitter_chain);
    println!(
        "    Emitter address: {}",
        hex::encode(test_vaa.emitter_address)
    );
    println!("    Sequence: {}", test_vaa.sequence);
    println!("    Nonce: {}", test_vaa.nonce);
    println!("    Consistency level: {}", test_vaa.consistency_level);
    println!(
        "    Payload: {:?}",
        String::from_utf8_lossy(&test_vaa.payload)
    );

    // Step 6: Sign the VAA with guardians
    let guardian_signatures = test_vaa.guardian_signatures(&guardians);
    println!(
        "\nStep 3: Signed VAA with {} guardian(s)",
        guardian_signatures.len()
    );

    // Step 7: Verify the VAA using the vaa-verifier-example program
    println!("\nStep 4: Verifying VAA in destination program...");
    let vaa_body = test_vaa.body();

    let verify_result = with_posted_signatures(
        &mut svm,
        &payer,
        GUARDIAN_SET_INDEX,
        &guardian_signatures,
        |svm, sigs_pubkey| -> Result<(), String> {
            let verify_ix = vaa_verifier_example::build_verify_vaa_instruction(
                &payer.pubkey(),
                &wormhole.guardian_set,
                sigs_pubkey,
                wormhole.guardian_set_bump,
                &vaa_body,
            );

            let blockhash = svm.latest_blockhash();
            let tx = Transaction::new_signed_with_payer(
                &[verify_ix],
                Some(&payer.pubkey()),
                &[&payer],
                blockhash,
            );

            svm.send_transaction(tx)
                .map_err(|e| format!("VAA verification failed: {:?}", e))?;

            Ok(())
        },
    );

    assert!(
        verify_result.is_ok(),
        "VAA verification should succeed: {:?}",
        verify_result
    );

    println!("  VAA verified successfully!");
    println!("\n=== Round-trip Complete ===");
    println!(
        "Message emitted on source chain → Captured by guardians → VAA verified on destination chain"
    );
}
