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
    use wormhole_svm_test::with_vaa;

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
    let result = with_vaa(
        &mut svm,
        &payer,
        &guardians,
        GUARDIAN_SET_INDEX,
        &vaa,
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
    use wormhole_svm_test::{with_vaa, WormholeTestError};

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
    let result = with_vaa(
        &mut svm,
        &payer,
        &guardians,
        GUARDIAN_SET_INDEX,
        &vaa,
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

    // with_vaa should detect the bypass and return an error
    assert!(
        result.is_err(),
        "with_vaa should have detected verification bypass"
    );

    match result {
        Err(WormholeTestError::VerificationBypass(msg)) => {
            println!("Correctly caught verification bypass: {}", msg);
            assert!(msg.contains("SECURITY"));
        }
        Err(e) => panic!("Expected VerificationBypass error, got: {:?}", e),
        Ok(_) => panic!("Expected error but got success"),
    }

    println!("with_vaa correctly detected the insecure program!");
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
