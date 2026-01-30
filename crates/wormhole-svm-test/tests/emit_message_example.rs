//! Integration tests for the Wormhole message emission and capture flow.
//!
//! These tests demonstrate how to:
//! 1. Emit a Wormhole message via the Post Message Shim
//! 2. Capture the message from the transaction's inner instructions
//! 3. Construct a VAA from the captured message
//! 4. Sign and verify the VAA
//!
//! The `message-emitter-example` program is used as a test fixture.

#![cfg(feature = "bundled-fixtures")]

use litesvm::LiteSVM;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use std::path::Path;
use wormhole_svm_definitions::{
    find_core_bridge_config_address, find_emitter_sequence_address, find_event_authority_address,
    find_fee_collector_address, find_shim_message_address,
    solana::mainnet::{CORE_BRIDGE_PROGRAM_ID, POST_MESSAGE_SHIM_PROGRAM_ID},
};
use wormhole_svm_test::{
    extract_posted_message_info_from_tx, read_emitter_sequence, setup_wormhole,
    with_posted_signatures, TestGuardian, TestGuardianSet, WormholeProgramsConfig,
};

// Message emitter example program ID (from the program's declare_id!)
const MESSAGE_EMITTER_ID: Pubkey =
    solana_sdk::pubkey!("26g7Z38n86MGtturwtHuWKG3hr4QhvnaBfinaFKVaz4x");

/// Load the message-emitter-example program from the build directory.
fn load_message_emitter(svm: &mut LiteSVM) {
    let search_paths = [
        "target/deploy/message_emitter_example.so",
        "../target/deploy/message_emitter_example.so",
        "../../target/deploy/message_emitter_example.so",
        "../../../target/deploy/message_emitter_example.so",
    ];
    for path in &search_paths {
        if Path::new(path).exists() {
            let bytes = std::fs::read(path).expect("read program");
            svm.add_program(MESSAGE_EMITTER_ID, &bytes)
                .expect("load program");
            return;
        }
    }
    panic!(
        "message_emitter_example.so not found. Build it with: cargo build-sbf -p message-emitter-example"
    );
}

/// Find the emitter PDA for the message-emitter-example program.
fn find_emitter_address() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[b"emitter"], &MESSAGE_EMITTER_ID)
}

/// Build an emit_message instruction for the message-emitter-example program.
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

/// Test emitting a message and capturing it from the transaction.
///
/// This test demonstrates the full round-trip:
/// 1. Emit a message via CPI using the message-emitter-example program
/// 2. Extract the PostedMessageInfo from the transaction's inner instructions
/// 3. Create a signed VAA from the captured message
/// 4. Verify the VAA structure is correct
#[test]
fn test_emit_message_and_capture() {
    let mut svm = LiteSVM::new();
    let guardians = TestGuardianSet::single(TestGuardian::default());
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();
    load_message_emitter(&mut svm);

    // Emit a message
    let payload = b"Hello, Wormhole!";
    let nonce = 42u32;
    let finality = 1u8;

    let ix = build_emit_ix(&payer.pubkey(), nonce, finality, payload);
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let tx_meta = svm.send_transaction(tx).expect("emit should succeed");

    // Extract everything from the transaction
    let message_info = extract_posted_message_info_from_tx(&tx_meta)
        .into_iter()
        .next()
        .expect("should extract PostedMessageInfo");

    // Verify the extracted data matches what we sent
    let (expected_emitter, _) = find_emitter_address();
    assert_eq!(message_info.emitter, expected_emitter);
    assert_eq!(message_info.sequence, 0); // First message
    assert_eq!(message_info.payload, payload);
    assert_eq!(message_info.nonce, nonce);
    assert_eq!(message_info.consistency_level, finality);
    assert_eq!(message_info.emitter_chain, 1); // Solana

    // Create a signed VAA from the message info
    let test_vaa = message_info.to_test_vaa();
    let signed_vaa = test_vaa.sign(&guardians);

    // Verify the signed VAA structure
    assert_eq!(signed_vaa[0], 1); // Version
    assert_eq!(signed_vaa[5], 1); // 1 signature

    // Check emitter in VAA body
    let body = test_vaa.body();
    let chain = u16::from_be_bytes(body[8..10].try_into().unwrap());
    assert_eq!(chain, 1); // Solana

    let emitter_in_body: [u8; 32] = body[10..42].try_into().unwrap();
    assert_eq!(emitter_in_body, expected_emitter.to_bytes());

    let seq = u64::from_be_bytes(body[42..50].try_into().unwrap());
    assert_eq!(seq, 0);

    println!("Successfully emitted message and captured PostedMessageInfo");
    println!("  Emitter: {}", message_info.emitter);
    println!("  Sequence: {}", message_info.sequence);
    println!(
        "  Payload: {:?}",
        String::from_utf8_lossy(&message_info.payload)
    );
    println!("  Signed VAA length: {} bytes", signed_vaa.len());
}

/// Test that emitting multiple messages increments the sequence number.
#[test]
fn test_emit_multiple_messages_increments_sequence() {
    let mut svm = LiteSVM::new();
    let guardians = TestGuardianSet::single(TestGuardian::default());
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();
    load_message_emitter(&mut svm);

    let (emitter, _) = find_emitter_address();

    // Emit multiple messages and verify sequence increments
    for expected_seq in 0u64..3 {
        let payload = format!("Message {}", expected_seq);
        let ix = build_emit_ix(&payer.pubkey(), 0, 1, payload.as_bytes());
        let blockhash = svm.latest_blockhash();
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
        let tx_meta = svm.send_transaction(tx).expect("emit should succeed");

        let message_info = extract_posted_message_info_from_tx(&tx_meta)
            .into_iter()
            .next()
            .expect("should extract PostedMessageInfo");

        assert_eq!(
            message_info.sequence, expected_seq,
            "sequence should be {}, got {}",
            expected_seq, message_info.sequence
        );

        // Verify sequence account was updated
        let seq_after =
            read_emitter_sequence(&svm, &emitter).expect("sequence account should exist");
        assert_eq!(
            seq_after,
            expected_seq + 1,
            "sequence account should be incremented"
        );
    }

    println!("Successfully emitted 3 messages with incrementing sequence numbers");
}

/// Test that an emitted message creates a verifiable VAA.
#[test]
fn test_emitted_message_creates_verifiable_vaa() {
    let mut svm = LiteSVM::new();
    let guardians = TestGuardianSet::single(TestGuardian::default());
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();
    load_message_emitter(&mut svm);

    // Emit a message
    let payload = b"Test payload for VAA verification";
    let ix = build_emit_ix(&payer.pubkey(), 0, 1, payload);
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let tx_meta = svm.send_transaction(tx).expect("emit should succeed");

    let message_info = extract_posted_message_info_from_tx(&tx_meta)
        .into_iter()
        .next()
        .expect("should extract PostedMessageInfo");

    // Create a TestVaa from the captured info
    let test_vaa = message_info.to_test_vaa();
    let signatures = test_vaa.guardian_signatures(&guardians);

    // Verify we can post signatures for this VAA
    let post_result = with_posted_signatures(
        &mut svm,
        &payer,
        0,
        &signatures,
        |svm, sigs_pubkey| -> Result<(), String> {
            // Verify the signatures account exists
            svm.get_account(sigs_pubkey)
                .ok_or_else(|| "signatures account not found".to_string())?;
            Ok(())
        },
    );

    assert!(
        post_result.is_ok(),
        "should be able to post signatures for the VAA: {:?}",
        post_result
    );

    println!("Successfully verified that emitted message creates verifiable VAA");
    println!("  Emitter: {}", message_info.emitter);
    println!("  Sequence: {}", message_info.sequence);
}

/// Test that extract_posted_message_info_from_tx correctly extracts everything.
#[test]
fn test_extract_posted_message_info_from_tx() {
    let mut svm = LiteSVM::new();
    let guardians = TestGuardianSet::single(TestGuardian::default());
    let payer = Keypair::new();

    svm.airdrop(&payer.pubkey(), 10_000_000_000).unwrap();
    setup_wormhole(&mut svm, &guardians, 0, WormholeProgramsConfig::default()).unwrap();
    load_message_emitter(&mut svm);

    // Emit a message with specific values
    let payload = b"Extract test payload";
    let nonce = 999u32;
    let finality = 1u8;

    let ix = build_emit_ix(&payer.pubkey(), nonce, finality, payload);
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[&payer], blockhash);
    let tx_meta = svm.send_transaction(tx).expect("should succeed");

    // Extract using the combined function
    let message_info = extract_posted_message_info_from_tx(&tx_meta)
        .into_iter()
        .next()
        .expect("should extract message info");

    // Verify the extracted data matches what we sent
    assert_eq!(message_info.payload, payload);
    assert_eq!(message_info.nonce, nonce);
    assert_eq!(message_info.consistency_level, finality);
    assert_eq!(message_info.sequence, 0); // First message
    assert_eq!(message_info.emitter_chain, 1); // Solana

    // Verify the emitter is the expected PDA
    let (expected_emitter, _) = find_emitter_address();
    assert_eq!(message_info.emitter, expected_emitter);

    println!("Successfully extracted PostedMessageInfo from transaction:");
    println!(
        "  Payload: {:?}",
        String::from_utf8_lossy(&message_info.payload)
    );
    println!("  Nonce: {}", message_info.nonce);
    println!("  Finality: {}", message_info.consistency_level);
    println!("  Sequence: {}", message_info.sequence);
}
