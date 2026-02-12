//! Placeholder substitution and transaction execution for resolved instructions.

use executor_account_resolver_svm::{
    RESOLVER_PUBKEY_GUARDIAN_SET, RESOLVER_PUBKEY_KEYPAIR_00, RESOLVER_PUBKEY_KEYPAIR_01,
    RESOLVER_PUBKEY_KEYPAIR_02, RESOLVER_PUBKEY_KEYPAIR_03, RESOLVER_PUBKEY_KEYPAIR_04,
    RESOLVER_PUBKEY_KEYPAIR_05, RESOLVER_PUBKEY_KEYPAIR_06, RESOLVER_PUBKEY_KEYPAIR_07,
    RESOLVER_PUBKEY_KEYPAIR_08, RESOLVER_PUBKEY_KEYPAIR_09, RESOLVER_PUBKEY_PAYER,
    RESOLVER_PUBKEY_SHIM_VAA_SIGS,
};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signature, Signer},
    transaction::Transaction,
};

use crate::connection::SolanaConnection;
use crate::resolve::{InstructionGroup, SerializableInstruction};
use crate::SubmitError;

const KEYPAIR_PLACEHOLDERS: [Pubkey; 10] = [
    RESOLVER_PUBKEY_KEYPAIR_00,
    RESOLVER_PUBKEY_KEYPAIR_01,
    RESOLVER_PUBKEY_KEYPAIR_02,
    RESOLVER_PUBKEY_KEYPAIR_03,
    RESOLVER_PUBKEY_KEYPAIR_04,
    RESOLVER_PUBKEY_KEYPAIR_05,
    RESOLVER_PUBKEY_KEYPAIR_06,
    RESOLVER_PUBKEY_KEYPAIR_07,
    RESOLVER_PUBKEY_KEYPAIR_08,
    RESOLVER_PUBKEY_KEYPAIR_09,
];

/// Execute resolved instruction groups, substituting placeholder pubkeys.
///
/// Each `InstructionGroup` becomes one transaction. Placeholders are replaced:
/// - `RESOLVER_PUBKEY_PAYER` -> payer
/// - `RESOLVER_PUBKEY_SHIM_VAA_SIGS` -> signatures account
/// - `RESOLVER_PUBKEY_GUARDIAN_SET` -> guardian set PDA
/// - `RESOLVER_PUBKEY_KEYPAIR_00..09` -> freshly generated keypairs (consistent across groups)
pub fn execute_instruction_groups<C: SolanaConnection>(
    conn: &mut C,
    payer: &Keypair,
    groups: &[InstructionGroup],
    signatures_pubkey: &Pubkey,
    guardian_set: &Pubkey,
) -> Result<Vec<Signature>, SubmitError> {
    // Generate keypairs up front so they're consistent across instruction groups.
    let generated_keypairs = discover_keypairs(groups);

    let keypair_map: Vec<(Pubkey, Pubkey)> = generated_keypairs
        .iter()
        .map(|(placeholder, kp)| (*placeholder, kp.pubkey()))
        .collect();

    let mut tx_sigs = Vec::new();

    for group in groups {
        let instructions: Vec<Instruction> = group
            .instructions
            .iter()
            .map(|si| {
                convert_instruction(
                    si,
                    &payer.pubkey(),
                    signatures_pubkey,
                    guardian_set,
                    &keypair_map,
                )
            })
            .collect();

        // Collect signers: payer + any generated keypairs used in this group
        let used_keypairs: Vec<&Keypair> = generated_keypairs
            .iter()
            .filter(|(placeholder, _)| {
                group
                    .instructions
                    .iter()
                    .any(|ix| ix.accounts.iter().any(|a| a.pubkey == *placeholder))
            })
            .map(|(_, kp)| kp)
            .collect();

        let mut signers: Vec<&Keypair> = vec![payer];
        signers.extend(used_keypairs);

        let blockhash = conn
            .get_latest_blockhash()
            .map_err(|e| SubmitError::Connection(e.to_string()))?;
        let tx = Transaction::new_signed_with_payer(
            &instructions,
            Some(&payer.pubkey()),
            &signers,
            blockhash,
        );

        let sig = conn
            .send_and_confirm(&tx)
            .map_err(|e| SubmitError::Execution(e.to_string()))?;
        tx_sigs.push(sig);
    }

    Ok(tx_sigs)
}

/// Scan all instruction groups for keypair placeholders and generate a keypair for each.
fn discover_keypairs(groups: &[InstructionGroup]) -> Vec<(Pubkey, Keypair)> {
    let mut result = Vec::new();
    for placeholder in &KEYPAIR_PLACEHOLDERS {
        let used = groups.iter().any(|group| {
            group
                .instructions
                .iter()
                .any(|ix| ix.accounts.iter().any(|a| a.pubkey == *placeholder))
        });
        if used {
            result.push((*placeholder, Keypair::new()));
        }
    }
    result
}

/// Convert a `SerializableInstruction` to a `solana_sdk::instruction::Instruction`,
/// substituting placeholder pubkeys.
fn convert_instruction(
    si: &SerializableInstruction,
    payer: &Pubkey,
    signatures_pubkey: &Pubkey,
    guardian_set: &Pubkey,
    keypair_map: &[(Pubkey, Pubkey)],
) -> Instruction {
    let accounts: Vec<AccountMeta> = si
        .accounts
        .iter()
        .map(|am| {
            let pubkey = substitute(
                am.pubkey,
                payer,
                signatures_pubkey,
                guardian_set,
                keypair_map,
            );
            if am.is_writable {
                AccountMeta::new(pubkey, am.is_signer)
            } else {
                AccountMeta::new_readonly(pubkey, am.is_signer)
            }
        })
        .collect();

    Instruction {
        program_id: si.program_id,
        accounts,
        data: si.data.clone(),
    }
}

fn substitute(
    pubkey: Pubkey,
    payer: &Pubkey,
    signatures_pubkey: &Pubkey,
    guardian_set: &Pubkey,
    keypair_map: &[(Pubkey, Pubkey)],
) -> Pubkey {
    if pubkey == RESOLVER_PUBKEY_PAYER {
        *payer
    } else if pubkey == RESOLVER_PUBKEY_SHIM_VAA_SIGS {
        *signatures_pubkey
    } else if pubkey == RESOLVER_PUBKEY_GUARDIAN_SET {
        *guardian_set
    } else if let Some((_, actual)) = keypair_map.iter().find(|(ph, _)| *ph == pubkey) {
        *actual
    } else {
        pubkey
    }
}
