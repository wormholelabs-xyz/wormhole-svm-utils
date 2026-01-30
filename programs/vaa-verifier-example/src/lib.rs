//! Minimal example program demonstrating VAA body verification.
//!
//! This program shows the complete flow of verifying a Wormhole VAA body:
//!
//! 1. Receive the VAA body bytes (not the full signed VAA)
//! 2. Compute the body digest (double keccak256)
//! 3. CPI to the Wormhole Verify VAA Shim to verify guardian signatures
//! 4. Process the verified payload
//!
//! ## Account Layout
//!
//! The instruction expects:
//! 0. `[signer]` Payer (for logging, not used for payment)
//! 1. `[]` Guardian set account (Wormhole Core Bridge PDA)
//! 2. `[]` Guardian signatures account (from post_signatures)
//! 3. `[]` Wormhole Verify VAA Shim program
//!
//! ## Instruction Data
//!
//! - `discriminator: u8` (1 byte) - 0 for verify, 1 for skip_verify
//! - `guardian_set_bump: u8` (1 byte)
//! - `vaa_body: Vec<u8>` (4-byte length prefix + body data)

use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    keccak, msg,
    program::invoke,
    program_error::ProgramError,
    pubkey::Pubkey,
};

#[cfg(not(feature = "no-entrypoint"))]
use solana_program::entrypoint;
use wormhole_svm_definitions::solana::mainnet::VERIFY_VAA_SHIM_PROGRAM_ID;

// Declare program ID - this is a placeholder, actual ID is set at deploy time
solana_program::declare_id!("VAAVerifier11111111111111111111111111111111");

#[cfg(not(feature = "no-entrypoint"))]
entrypoint!(process_instruction);

/// Instruction discriminator byte.
const IX_VERIFY_VAA: u8 = 0;
const IX_SKIP_VERIFY: u8 = 1;

/// VAA body layout offsets (all big-endian)
const BODY_EMITTER_CHAIN_OFFSET: usize = 8; // after timestamp (4) + nonce (4)
const BODY_SEQUENCE_OFFSET: usize = 42; // after emitter_chain (2) + emitter_address (32)
const BODY_PAYLOAD_OFFSET: usize = 51; // after sequence (8) + consistency_level (1)

/// Process instructions.
pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    if instruction_data.is_empty() {
        msg!("Error: Empty instruction data");
        return Err(ProgramError::InvalidInstructionData);
    }

    match instruction_data[0] {
        IX_VERIFY_VAA => process_verify_vaa(program_id, accounts, &instruction_data[1..]),
        IX_SKIP_VERIFY => process_skip_verify(program_id, accounts, &instruction_data[1..]),
        _ => {
            msg!("Error: Unknown instruction");
            Err(ProgramError::InvalidInstructionData)
        }
    }
}

/// Process the verify_vaa instruction (SECURE - actually verifies).
fn process_verify_vaa(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    msg!("VAA Verifier: Processing verify_vaa instruction");

    // Parse accounts
    let account_iter = &mut accounts.iter();
    let _payer = next_account_info(account_iter)?;
    let guardian_set = next_account_info(account_iter)?;
    let guardian_signatures = next_account_info(account_iter)?;
    let shim_program = next_account_info(account_iter)?;

    // Verify the shim program ID
    if shim_program.key != &VERIFY_VAA_SHIM_PROGRAM_ID {
        msg!("Error: Invalid shim program ID");
        return Err(ProgramError::IncorrectProgramId);
    }

    // Parse instruction data: [bump (1), body_len (4), body_bytes...]
    if instruction_data.len() < 5 {
        msg!("Error: Instruction data too short");
        return Err(ProgramError::InvalidInstructionData);
    }

    let guardian_set_bump = instruction_data[0];
    let body_len = u32::from_le_bytes(
        instruction_data[1..5]
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    ) as usize;

    if instruction_data.len() < 5 + body_len {
        msg!("Error: Instruction data too short for body");
        return Err(ProgramError::InvalidInstructionData);
    }

    let vaa_body = &instruction_data[5..5 + body_len];

    // Log VAA info from body
    if vaa_body.len() >= BODY_PAYLOAD_OFFSET {
        let emitter_chain = u16::from_be_bytes(
            vaa_body[BODY_EMITTER_CHAIN_OFFSET..BODY_EMITTER_CHAIN_OFFSET + 2]
                .try_into()
                .unwrap(),
        );
        let sequence = u64::from_be_bytes(
            vaa_body[BODY_SEQUENCE_OFFSET..BODY_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        msg!("VAA body: chain={}, sequence={}", emitter_chain, sequence);
    }

    // Compute the VAA digest (double keccak256)
    let message_hash = keccak::hashv(&[vaa_body]);
    let digest = keccak::hash(&message_hash.to_bytes());

    msg!("VAA digest: {:?}", &digest.to_bytes()[..8]);

    // Build the verify_hash CPI instruction using wormhole_svm_shim types
    use wormhole_svm_shim::verify_vaa::{VerifyHash, VerifyHashAccounts, VerifyHashData};

    let verify_ix = VerifyHash {
        program_id: &VERIFY_VAA_SHIM_PROGRAM_ID,
        accounts: VerifyHashAccounts {
            guardian_set: guardian_set.key,
            guardian_signatures: guardian_signatures.key,
        },
        data: VerifyHashData::new(guardian_set_bump, digest.into()),
    }
    .instruction();

    // Execute CPI to verify the VAA
    invoke(
        &verify_ix,
        &[guardian_set.clone(), guardian_signatures.clone()],
    )?;

    msg!("VAA verified successfully!");

    // Extract and log payload info
    if vaa_body.len() > BODY_PAYLOAD_OFFSET {
        let payload = &vaa_body[BODY_PAYLOAD_OFFSET..];
        msg!("Payload length: {} bytes", payload.len());

        if payload.len() >= 4 {
            msg!(
                "Payload prefix: {:02x}{:02x}{:02x}{:02x}",
                payload[0],
                payload[1],
                payload[2],
                payload[3]
            );
        }
    }

    msg!("VAA Verifier: Success");
    Ok(())
}

/// Process the skip_verify instruction (INSECURE - does NOT verify VAA).
///
/// This instruction is intentionally insecure for testing purposes.
/// It demonstrates what happens when a program accepts VAA data without
/// actually verifying the guardian signatures via CPI.
fn process_skip_verify(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    msg!("VAA Verifier: Processing skip_verify instruction (INSECURE!)");

    // Parse accounts (same layout, but we won't actually use the shim)
    let account_iter = &mut accounts.iter();
    let _payer = next_account_info(account_iter)?;
    let _guardian_set = next_account_info(account_iter)?;
    let _guardian_signatures = next_account_info(account_iter)?;
    let _shim_program = next_account_info(account_iter)?;

    // Parse instruction data
    if instruction_data.len() < 5 {
        msg!("Error: Instruction data too short");
        return Err(ProgramError::InvalidInstructionData);
    }

    let _guardian_set_bump = instruction_data[0];
    let body_len = u32::from_le_bytes(
        instruction_data[1..5]
            .try_into()
            .map_err(|_| ProgramError::InvalidInstructionData)?,
    ) as usize;

    if instruction_data.len() < 5 + body_len {
        msg!("Error: Instruction data too short for body");
        return Err(ProgramError::InvalidInstructionData);
    }

    let vaa_body = &instruction_data[5..5 + body_len];

    // Log VAA info from body (without verification)
    if vaa_body.len() >= BODY_PAYLOAD_OFFSET {
        let emitter_chain = u16::from_be_bytes(
            vaa_body[BODY_EMITTER_CHAIN_OFFSET..BODY_EMITTER_CHAIN_OFFSET + 2]
                .try_into()
                .unwrap(),
        );
        let sequence = u64::from_be_bytes(
            vaa_body[BODY_SEQUENCE_OFFSET..BODY_SEQUENCE_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        msg!(
            "VAA body: chain={}, sequence={} (NOT VERIFIED!)",
            emitter_chain,
            sequence
        );
    }

    // SECURITY BUG: We're NOT calling verify_hash CPI!
    // This means anyone can submit a VAA with forged signatures.
    msg!("SKIPPING VERIFICATION - this is a security vulnerability!");

    msg!("VAA Verifier: Success (but not actually verified!)");
    Ok(())
}

/// Build instruction data for the verify_vaa instruction.
pub fn build_instruction_data(guardian_set_bump: u8, vaa_body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(6 + vaa_body.len());
    data.push(IX_VERIFY_VAA); // discriminator
    data.push(guardian_set_bump);
    data.extend_from_slice(&(vaa_body.len() as u32).to_le_bytes());
    data.extend_from_slice(vaa_body);
    data
}

/// Build instruction data for the skip_verify instruction (INSECURE).
pub fn build_skip_verify_instruction_data(guardian_set_bump: u8, vaa_body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(6 + vaa_body.len());
    data.push(IX_SKIP_VERIFY); // discriminator
    data.push(guardian_set_bump);
    data.extend_from_slice(&(vaa_body.len() as u32).to_le_bytes());
    data.extend_from_slice(vaa_body);
    data
}

/// Build a verify_vaa instruction.
pub fn build_verify_vaa_instruction(
    payer: &Pubkey,
    guardian_set: &Pubkey,
    guardian_signatures: &Pubkey,
    guardian_set_bump: u8,
    vaa_body: &[u8],
) -> solana_program::instruction::Instruction {
    let data = build_instruction_data(guardian_set_bump, vaa_body);

    solana_program::instruction::Instruction {
        program_id: crate::ID,
        accounts: vec![
            solana_program::instruction::AccountMeta::new_readonly(*payer, true),
            solana_program::instruction::AccountMeta::new_readonly(*guardian_set, false),
            solana_program::instruction::AccountMeta::new_readonly(*guardian_signatures, false),
            solana_program::instruction::AccountMeta::new_readonly(
                VERIFY_VAA_SHIM_PROGRAM_ID,
                false,
            ),
        ],
        data,
    }
}

/// Build an INSECURE skip_verify instruction (for testing only).
///
/// This instruction does NOT verify the VAA signatures and should be used
/// only to test that `with_vaa` detects programs that skip verification.
pub fn build_skip_verify_instruction(
    payer: &Pubkey,
    guardian_set: &Pubkey,
    guardian_signatures: &Pubkey,
    guardian_set_bump: u8,
    vaa_body: &[u8],
) -> solana_program::instruction::Instruction {
    let data = build_skip_verify_instruction_data(guardian_set_bump, vaa_body);

    solana_program::instruction::Instruction {
        program_id: crate::ID,
        accounts: vec![
            solana_program::instruction::AccountMeta::new_readonly(*payer, true),
            solana_program::instruction::AccountMeta::new_readonly(*guardian_set, false),
            solana_program::instruction::AccountMeta::new_readonly(*guardian_signatures, false),
            solana_program::instruction::AccountMeta::new_readonly(
                VERIFY_VAA_SHIM_PROGRAM_ID,
                false,
            ),
        ],
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instruction_data_roundtrip() {
        let bump = 255;
        let body = vec![1, 2, 3, 4, 5];

        let data = build_instruction_data(bump, &body);

        assert_eq!(data[0], IX_VERIFY_VAA); // discriminator
        assert_eq!(data[1], bump);
        let len = u32::from_le_bytes(data[2..6].try_into().unwrap());
        assert_eq!(len, 5);
        assert_eq!(&data[6..], &body);
    }
}
