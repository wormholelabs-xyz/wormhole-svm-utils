//! LiteSVM adapter for the [`SolanaConnection`] trait and convenience wrapper
//! around the generic resolver.
//!
//! # Example
//!
//! ```ignore
//! use wormhole_svm_test::resolve_execute_vaa_v1;
//!
//! let result = resolve_execute_vaa_v1(
//!     &mut svm,
//!     &my_program::ID,
//!     &payer,
//!     &vaa_body,
//!     &guardian_set_pubkey,
//!     10,
//! ).expect("resolution should succeed");
//!
//! assert_eq!(result.iterations, 2);
//! ```

use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    hash::Hash,
    pubkey::Pubkey,
    signature::{Keypair, Signature},
    transaction::Transaction,
};

use wormhole_svm_submit::SolanaConnection;

// Re-export types consumers need for inspecting resolved instructions.
pub use wormhole_svm_submit::resolve::{
    InstructionGroup, ResolverResult, SerializableAccountMeta, SerializableInstruction,
};
pub use wormhole_svm_submit::{
    SubmitError, RESOLVER_PUBKEY_GUARDIAN_SET, RESOLVER_PUBKEY_PAYER, RESOLVER_PUBKEY_SHIM_VAA_SIGS,
};

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

/// Convenience wrapper around [`wormhole_svm_submit::resolve::resolve_execute_vaa_v1`]
/// for LiteSVM.
///
/// # Arguments
/// * `svm` - LiteSVM instance (must have the target program loaded)
/// * `program_id` - The program implementing `resolve_execute_vaa_v1`
/// * `payer` - Keypair for signing simulation transactions
/// * `vaa_body` - The VAA body bytes to resolve
/// * `guardian_set` - The actual guardian set pubkey to substitute for the placeholder
/// * `max_iterations` - Safety limit on resolution rounds
pub fn resolve_execute_vaa_v1(
    svm: &mut LiteSVM,
    program_id: &Pubkey,
    payer: &Keypair,
    vaa_body: &[u8],
    guardian_set: &Pubkey,
    max_iterations: usize,
) -> Result<ResolverResult, String> {
    let conn = LiteSvmConnection(svm);
    wormhole_svm_submit::resolve::resolve_execute_vaa_v1(
        &conn,
        program_id,
        payer,
        vaa_body,
        guardian_set,
        max_iterations,
    )
    .map_err(|e| e.to_string())
}
