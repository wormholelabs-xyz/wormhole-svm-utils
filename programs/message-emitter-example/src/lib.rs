//! Minimal example program demonstrating Wormhole message emission.
//!
//! This program shows the complete flow of posting a Wormhole message via
//! the Post Message Shim:
//!
//! 1. Receive payload, nonce, and finality from the instruction
//! 2. Build the post_message CPI instruction
//! 3. CPI to the Post Message Shim
//! 4. The shim emits a MessageEvent that can be captured
//!
//! ## Account Layout
//!
//! The instruction expects (same as Post Message Shim plus emitter PDA):
//! 0.  `[writable]` Core Bridge config
//! 1.  `[writable]` Message account (PDA of shim)
//! 2.  `[signer]`   Emitter (our program's PDA that signs the message)
//! 3.  `[writable]` Sequence account (PDA of Core Bridge)
//! 4.  `[signer, writable]` Payer
//! 5.  `[writable]` Fee collector
//! 6.  `[]` Clock sysvar
//! 7.  `[]` System program
//! 8.  `[]` Core Bridge program
//! 9.  `[]` Event authority (shim's event authority PDA)
//! 10. `[]` Post Message Shim program
//!
//! ## Instruction Data
//!
//! - `nonce: u32` (4 bytes, little-endian)
//! - `finality: u8` (1 byte)
//! - `payload_len: u32` (4 bytes, little-endian)
//! - `payload: [u8]` (variable length)

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    instruction::{AccountMeta, Instruction},
    msg,
    program::invoke_signed,
    program_error::ProgramError,
    pubkey::Pubkey,
};

#[cfg(not(feature = "no-entrypoint"))]
use solana_program::entrypoint;

use wormhole_svm_definitions::solana::mainnet::POST_MESSAGE_SHIM_PROGRAM_ID;

// Declare program ID - this is a placeholder, actual ID is set at deploy time
solana_program::declare_id!("26g7Z38n86MGtturwtHuWKG3hr4QhvnaBfinaFKVaz4x");

/// Seed for the emitter PDA
pub const EMITTER_SEED: &[u8] = b"emitter";

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

/// Find the emitter PDA for this program.
pub fn find_emitter_address() -> (Pubkey, u8) {
    Pubkey::find_program_address(&[EMITTER_SEED], &crate::ID)
}

/// Process instructions.
pub fn process_instruction(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    msg!("Message Emitter: Processing emit_message instruction");

    // Parse instruction data
    if instruction_data.len() < 9 {
        msg!("Error: Instruction data too short");
        return Err(ProgramError::InvalidInstructionData);
    }

    let nonce = u32::from_le_bytes(
        instruction_data[0..4]
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    );
    let finality = instruction_data[4];
    let payload_len = u32::from_le_bytes(
        instruction_data[5..9]
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    ) as usize;

    if instruction_data.len() < 9 + payload_len {
        msg!("Error: Instruction data too short for payload");
        return Err(ProgramError::InvalidInstructionData);
    }

    let payload = &instruction_data[9..9 + payload_len];

    msg!(
        "Emitting message: nonce={}, finality={}, payload_len={}",
        nonce,
        finality,
        payload.len()
    );

    // Parse accounts
    let account_iter = &mut accounts.iter();
    let core_bridge_config = next_account_info(account_iter)?;
    let message = next_account_info(account_iter)?;
    let emitter = next_account_info(account_iter)?;
    let sequence = next_account_info(account_iter)?;
    let payer = next_account_info(account_iter)?;
    let fee_collector = next_account_info(account_iter)?;
    let clock = next_account_info(account_iter)?;
    let system_program = next_account_info(account_iter)?;
    let core_bridge_program = next_account_info(account_iter)?;
    let event_authority = next_account_info(account_iter)?;
    let post_message_shim = next_account_info(account_iter)?;

    // Verify emitter is our PDA
    let (expected_emitter, emitter_bump) = find_emitter_address();
    if emitter.key != &expected_emitter {
        msg!("Error: Invalid emitter PDA");
        return Err(ProgramError::InvalidSeeds);
    }

    // Verify Post Message Shim program ID
    if post_message_shim.key != &POST_MESSAGE_SHIM_PROGRAM_ID {
        msg!("Error: Invalid Post Message Shim program ID");
        return Err(ProgramError::IncorrectProgramId);
    }

    // Build the post_message instruction data manually
    // Anchor discriminator for post_message
    let discriminator: [u8; 8] =
        wormhole_svm_shim::post_message::PostMessageShimInstruction::<u8>::POST_MESSAGE_SELECTOR;

    let mut ix_data = Vec::with_capacity(8 + 4 + 1 + 4 + payload.len());
    ix_data.extend_from_slice(&discriminator);
    ix_data.extend_from_slice(&nonce.to_le_bytes());
    ix_data.push(finality);
    ix_data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    ix_data.extend_from_slice(payload);

    // Build the post_message instruction
    let post_message_ix = Instruction {
        program_id: POST_MESSAGE_SHIM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*core_bridge_config.key, false),
            AccountMeta::new(*message.key, false),
            AccountMeta::new_readonly(*emitter.key, true),
            AccountMeta::new(*sequence.key, false),
            AccountMeta::new(*payer.key, true),
            AccountMeta::new(*fee_collector.key, false),
            AccountMeta::new_readonly(*clock.key, false),
            AccountMeta::new_readonly(*system_program.key, false),
            AccountMeta::new_readonly(*core_bridge_program.key, false),
            AccountMeta::new_readonly(*event_authority.key, false),
            AccountMeta::new_readonly(*post_message_shim.key, false),
        ],
        data: ix_data,
    };

    // Execute CPI with emitter PDA as signer
    let emitter_seeds: &[&[u8]] = &[EMITTER_SEED, &[emitter_bump]];

    invoke_signed(
        &post_message_ix,
        &[
            core_bridge_config.clone(),
            message.clone(),
            emitter.clone(),
            sequence.clone(),
            payer.clone(),
            fee_collector.clone(),
            clock.clone(),
            system_program.clone(),
            core_bridge_program.clone(),
            event_authority.clone(),
            post_message_shim.clone(),
        ],
        &[emitter_seeds],
    )?;

    msg!("Message emitted successfully!");
    Ok(())
}

/// Build instruction data for emit_message.
pub fn build_instruction_data(nonce: u32, finality: u8, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(9 + payload.len());
    data.extend_from_slice(&nonce.to_le_bytes());
    data.push(finality);
    data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    data.extend_from_slice(payload);
    data
}

/// Build an emit_message instruction.
///
/// This instruction will CPI to the Post Message Shim to emit a Wormhole message.
pub fn build_emit_message_instruction(
    payer: &Pubkey,
    nonce: u32,
    finality: u8,
    payload: &[u8],
) -> Instruction {
    use wormhole_svm_definitions::{
        find_core_bridge_config_address, find_emitter_sequence_address,
        find_event_authority_address, find_fee_collector_address, find_shim_message_address,
        solana::mainnet::CORE_BRIDGE_PROGRAM_ID,
    };

    let (emitter, _) = find_emitter_address();
    let (core_bridge_config, _) = find_core_bridge_config_address(&CORE_BRIDGE_PROGRAM_ID);
    let (message, _) = find_shim_message_address(&emitter, &POST_MESSAGE_SHIM_PROGRAM_ID);
    let (sequence, _) = find_emitter_sequence_address(&emitter, &CORE_BRIDGE_PROGRAM_ID);
    let (fee_collector, _) = find_fee_collector_address(&CORE_BRIDGE_PROGRAM_ID);
    let (event_authority, _) = find_event_authority_address(&POST_MESSAGE_SHIM_PROGRAM_ID);

    let data = build_instruction_data(nonce, finality, payload);

    Instruction {
        program_id: crate::ID,
        accounts: vec![
            AccountMeta::new(core_bridge_config, false),
            AccountMeta::new(message, false),
            AccountMeta::new_readonly(emitter, false), // Signed by CPI
            AccountMeta::new(sequence, false),
            AccountMeta::new(*payer, true),
            AccountMeta::new(fee_collector, false),
            AccountMeta::new_readonly(solana_program::sysvar::clock::id(), false),
            AccountMeta::new_readonly(solana_program::system_program::id(), false),
            AccountMeta::new_readonly(CORE_BRIDGE_PROGRAM_ID, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(POST_MESSAGE_SHIM_PROGRAM_ID, false),
        ],
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_data_roundtrip() {
        let nonce = 42u32;
        let finality = 1u8;
        let payload = vec![1, 2, 3, 4, 5];

        let data = build_instruction_data(nonce, finality, &payload);

        assert_eq!(u32::from_le_bytes(data[0..4].try_into().unwrap()), nonce);
        assert_eq!(data[4], finality);
        let len = u32::from_le_bytes(data[5..9].try_into().unwrap());
        assert_eq!(len, 5);
        assert_eq!(&data[9..], &payload);
    }

    #[test]
    fn test_emitter_pda() {
        let (emitter, bump) = find_emitter_address();
        assert!(bump <= 255);

        // Verify it's a valid PDA
        let derived = Pubkey::create_program_address(&[EMITTER_SEED, &[bump]], &crate::ID);
        assert_eq!(derived.unwrap(), emitter);
    }
}
