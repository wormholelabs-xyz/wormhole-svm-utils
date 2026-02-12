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
    pubkey::Pubkey,
    signature::{Keypair, Signature},
};

use crate::litesvm::{LiteSvmConnection, ReplayProtection, WormholeTestError};
use crate::TestGuardianSet;

// Re-export types consumers need for inspecting resolved instructions.
pub use wormhole_svm_submit::resolve::{
    InstructionGroup, ResolverResult, SerializableAccountMeta, SerializableInstruction,
};
pub use wormhole_svm_submit::{
    SubmitError, RESOLVER_PUBKEY_GUARDIAN_SET, RESOLVER_PUBKEY_PAYER, RESOLVER_PUBKEY_SHIM_VAA_SIGS,
};

/// Maximum resolver iterations before giving up.
const MAX_RESOLVER_ITERATIONS: usize = 10;

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

/// Submit a signed VAA to a program via the resolver-executor flow, with full
/// safety checks (negative test + optional replay protection).
///
/// This is the test-crate equivalent of [`wormhole_svm_submit::broadcast_vaa`]:
/// it runs the resolve → post-signatures → execute → close-signatures flow, but
/// wraps it in [`with_vaa`](crate::with_vaa) so that:
///
/// 1. A **negative test** verifies the program rejects mismatched signatures.
/// 2. A **positive test** executes the VAA against the real SVM state.
/// 3. An optional **replay test** verifies the program rejects the same VAA twice.
///
/// # Arguments
///
/// * `svm` - LiteSVM instance (must have the target program and Wormhole loaded)
/// * `payer` - Keypair that pays for transactions
/// * `program_id` - The program implementing `resolve_execute_vaa_v1`
/// * `guardians` - Test guardian set for signing
/// * `guardian_set_index` - On-chain guardian set index
/// * `vaa` - The test VAA to submit
/// * `replay_protection` - Whether to verify replay protection
pub fn broadcast_vaa(
    svm: &mut LiteSVM,
    payer: &Keypair,
    program_id: &Pubkey,
    guardians: &TestGuardianSet,
    guardian_set_index: u32,
    vaa: &crate::TestVaa,
    replay_protection: ReplayProtection,
) -> Result<Vec<Signature>, WormholeTestError> {
    use wormhole_svm_definitions::find_guardian_set_address;
    use wormhole_svm_definitions::solana::mainnet::CORE_BRIDGE_PROGRAM_ID;

    let (guardian_set, _bump) =
        find_guardian_set_address(guardian_set_index.to_be_bytes(), &CORE_BRIDGE_PROGRAM_ID);

    let program_id = *program_id;

    crate::with_vaa(
        svm,
        payer,
        guardians,
        guardian_set_index,
        vaa,
        replay_protection,
        |svm, sigs_pubkey, vaa_body| -> Result<Vec<Signature>, String> {
            // Step 1: Resolve accounts
            let resolved = resolve_execute_vaa_v1(
                svm,
                &program_id,
                payer,
                vaa_body,
                &guardian_set,
                MAX_RESOLVER_ITERATIONS,
            )?;

            // Step 2: Execute resolved instructions
            let mut conn = LiteSvmConnection(svm);
            wormhole_svm_submit::execute::execute_instruction_groups(
                &mut conn,
                payer,
                &resolved.instruction_groups,
                sigs_pubkey,
                &guardian_set,
            )
            .map_err(|e| e.to_string())
        },
    )
}
