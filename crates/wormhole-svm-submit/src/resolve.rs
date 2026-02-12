//! Generic resolver loop for the executor-account-resolver protocol.
//!
//! Iteratively simulates the `resolve_execute_vaa_v1` instruction to discover
//! all accounts required for execution, accumulating missing accounts each round.

use borsh::BorshDeserialize;
use executor_account_resolver_svm::{
    InstructionGroups, MissingAccounts, Resolver, RESOLVER_EXECUTE_VAA_V1,
    RESOLVER_PUBKEY_GUARDIAN_SET, RESOLVER_PUBKEY_PAYER,
};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};

use crate::connection::SolanaConnection;
use crate::SubmitError;

pub use executor_account_resolver_svm::{
    InstructionGroup, SerializableAccountMeta, SerializableInstruction,
    RESOLVER_PUBKEY_SHIM_VAA_SIGS,
};

/// Result of running the resolver.
pub struct ResolverResult {
    /// The resolved instruction groups.
    pub instruction_groups: Vec<InstructionGroup>,
    /// How many iterations it took to resolve.
    pub iterations: usize,
}

/// Run the executor-account-resolver `resolve_execute_vaa_v1` loop.
///
/// Iteratively simulates the resolver instruction against `program_id` until
/// the program returns `Resolved(InstructionGroups)`, accumulating missing
/// accounts each round.
///
/// Placeholder pubkeys are automatically substituted:
/// - `RESOLVER_PUBKEY_PAYER` -> `payer.pubkey()`
/// - `RESOLVER_PUBKEY_GUARDIAN_SET` -> `guardian_set`
/// - `RESOLVER_PUBKEY_SHIM_VAA_SIGS` -> left as-is (substituted at execution time)
pub fn resolve_execute_vaa_v1<C: SolanaConnection>(
    conn: &C,
    program_id: &Pubkey,
    payer: &Keypair,
    vaa_body: &[u8],
    guardian_set: &Pubkey,
    max_iterations: usize,
) -> Result<ResolverResult, SubmitError> {
    let mut remaining_accounts: Vec<AccountMeta> = Vec::new();

    for iteration in 1..=max_iterations {
        // Build the resolver instruction data:
        // 8-byte discriminator + borsh Vec<u8> (4-byte LE length + bytes)
        let mut ix_data = Vec::with_capacity(8 + 4 + vaa_body.len());
        ix_data.extend_from_slice(&RESOLVER_EXECUTE_VAA_V1);
        ix_data.extend_from_slice(&(vaa_body.len() as u32).to_le_bytes());
        ix_data.extend_from_slice(vaa_body);

        let ix = Instruction {
            program_id: *program_id,
            accounts: remaining_accounts.clone(),
            data: ix_data,
        };

        let blockhash = conn
            .get_latest_blockhash()
            .map_err(|e| SubmitError::Connection(e.to_string()))?;
        let tx =
            Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);

        let return_data = conn
            .simulate_return_data(&tx)
            .map_err(|e| {
                SubmitError::ResolverSimulation(format!(
                    "Resolver simulation failed on iteration {}: {}",
                    iteration, e
                ))
            })?
            .ok_or_else(|| {
                SubmitError::ResolverSimulation(format!(
                    "No return data from resolver on iteration {}",
                    iteration
                ))
            })?;

        let resolver: Resolver<InstructionGroups> =
            BorshDeserialize::deserialize(&mut return_data.as_slice()).map_err(|e| {
                SubmitError::ResolverSimulation(format!(
                    "Failed to deserialize resolver return data: {}",
                    e
                ))
            })?;

        match resolver {
            Resolver::Resolved(groups) => {
                return Ok(ResolverResult {
                    instruction_groups: groups.0,
                    iterations: iteration,
                });
            }
            Resolver::Missing(MissingAccounts {
                accounts: missing,
                address_lookup_tables: _,
            }) => {
                for pubkey in &missing {
                    let actual = substitute_placeholder(*pubkey, &payer.pubkey(), guardian_set);
                    remaining_accounts.push(AccountMeta::new_readonly(actual, false));
                }
            }
            Resolver::Account() => {
                return Err(SubmitError::ResolverSimulation(
                    "Resolver returned Account() -- not supported".to_string(),
                ));
            }
        }
    }

    Err(SubmitError::ResolverSimulation(format!(
        "Resolver did not resolve after {} iterations. \
         Remaining accounts: {:?}",
        max_iterations,
        remaining_accounts
            .iter()
            .map(|a| a.pubkey.to_string())
            .collect::<Vec<_>>()
    )))
}

/// Substitute well-known placeholder pubkeys with actual values.
fn substitute_placeholder(pubkey: Pubkey, payer: &Pubkey, guardian_set: &Pubkey) -> Pubkey {
    if pubkey == RESOLVER_PUBKEY_PAYER {
        *payer
    } else if pubkey == RESOLVER_PUBKEY_GUARDIAN_SET {
        *guardian_set
    } else {
        // RESOLVER_PUBKEY_SHIM_VAA_SIGS and others are left as-is;
        // they are substituted at execution time, not resolve time.
        pubkey
    }
}
