//! Post and close guardian signatures.
//!
//! The [`post_signatures`] and [`close_signatures`] functions are generic over
//! [`SolanaConnection`], so they work with both `RpcClient` (production) and
//! `LiteSvmConnection` (testing).
//!
//! Instruction builders ([`build_post_signatures_ix`], [`build_close_signatures_ix`])
//! are also provided for callers that want to compose transactions manually.

use solana_sdk::{
    instruction::Instruction,
    pubkey::Pubkey,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use wormhole_svm_shim::verify_vaa::{
    CloseSignatures, CloseSignaturesAccounts, PostSignatures, PostSignaturesAccounts,
    PostSignaturesData,
};

use crate::connection::SolanaConnection;
use crate::SubmitError;

/// Result of posting guardian signatures.
pub struct PostedSignatures {
    /// The keypair for the signatures account (needed for close).
    pub keypair: Keypair,
    /// The public key of the signatures account.
    pub pubkey: Pubkey,
}

/// Build a `PostSignatures` instruction without sending it.
pub fn build_post_signatures_ix(
    payer: &Pubkey,
    guardian_signatures_keypair: &Pubkey,
    verify_vaa_shim: &Pubkey,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Instruction {
    PostSignatures {
        program_id: verify_vaa_shim,
        accounts: PostSignaturesAccounts {
            payer,
            guardian_signatures: guardian_signatures_keypair,
        },
        data: PostSignaturesData::new(guardian_set_index, signatures.len() as u8, signatures),
    }
    .instruction()
}

/// Build a `CloseSignatures` instruction without sending it.
pub fn build_close_signatures_ix(
    verify_vaa_shim: &Pubkey,
    guardian_signatures: &Pubkey,
    refund_recipient: &Pubkey,
) -> Instruction {
    CloseSignatures {
        program_id: verify_vaa_shim,
        accounts: CloseSignaturesAccounts {
            guardian_signatures,
            refund_recipient,
        },
    }
    .instruction()
}

/// Post guardian signatures to the Wormhole Verify VAA Shim.
///
/// Creates a temporary account containing the guardian signatures,
/// which is then used during resolver execution for VAA verification.
pub fn post_signatures<C: SolanaConnection>(
    conn: &mut C,
    payer: &Keypair,
    verify_vaa_shim: &Pubkey,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Result<PostedSignatures, SubmitError> {
    let guardian_sigs_keypair = Keypair::new();

    let ix = build_post_signatures_ix(
        &payer.pubkey(),
        &guardian_sigs_keypair.pubkey(),
        verify_vaa_shim,
        guardian_set_index,
        signatures,
    );

    let blockhash = conn
        .get_latest_blockhash()
        .map_err(|e| SubmitError::Connection(e.to_string()))?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, &guardian_sigs_keypair],
        blockhash,
    );

    conn.send_and_confirm(&tx)
        .map_err(|e| SubmitError::Connection(e.to_string()))?;

    let pubkey = guardian_sigs_keypair.pubkey();
    Ok(PostedSignatures {
        keypair: guardian_sigs_keypair,
        pubkey,
    })
}

/// Close a guardian signatures account to reclaim rent.
pub fn close_signatures<C: SolanaConnection>(
    conn: &mut C,
    payer: &Keypair,
    verify_vaa_shim: &Pubkey,
    signatures_pubkey: &Pubkey,
) -> Result<(), SubmitError> {
    let ix = build_close_signatures_ix(verify_vaa_shim, signatures_pubkey, &payer.pubkey());

    let blockhash = conn
        .get_latest_blockhash()
        .map_err(|e| SubmitError::Connection(e.to_string()))?;
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);

    conn.send_and_confirm(&tx)
        .map_err(|e| SubmitError::Connection(e.to_string()))?;

    Ok(())
}
