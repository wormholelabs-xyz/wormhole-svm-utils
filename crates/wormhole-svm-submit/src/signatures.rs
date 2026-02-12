//! Post and close guardian signatures via RPC.

use solana_client::rpc_client::RpcClient;
use solana_sdk::{
    commitment_config::CommitmentConfig,
    signature::{Keypair, Signer},
    transaction::Transaction,
};
use wormhole_svm_shim::verify_vaa::{
    CloseSignatures, CloseSignaturesAccounts, PostSignatures, PostSignaturesAccounts,
    PostSignaturesData,
};

use crate::SubmitError;

/// Result of posting guardian signatures.
pub struct PostedSignatures {
    /// The keypair for the signatures account (needed for close).
    pub keypair: Keypair,
}

/// Post guardian signatures to the Wormhole Verify VAA Shim.
///
/// Creates a temporary account containing the guardian signatures,
/// which is then used during resolver execution for VAA verification.
pub fn post_signatures(
    client: &RpcClient,
    payer: &Keypair,
    verify_vaa_shim: &solana_sdk::pubkey::Pubkey,
    guardian_set_index: u32,
    signatures: &[[u8; 66]],
) -> Result<PostedSignatures, SubmitError> {
    let guardian_sigs_keypair = Keypair::new();

    let ix = PostSignatures {
        program_id: verify_vaa_shim,
        accounts: PostSignaturesAccounts {
            payer: &payer.pubkey(),
            guardian_signatures: &guardian_sigs_keypair.pubkey(),
        },
        data: PostSignaturesData::new(guardian_set_index, signatures.len() as u8, signatures),
    }
    .instruction();

    let blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[payer, &guardian_sigs_keypair],
        blockhash,
    );

    client.send_and_confirm_transaction_with_spinner_and_commitment(
        &tx,
        CommitmentConfig::confirmed(),
    )?;

    Ok(PostedSignatures {
        keypair: guardian_sigs_keypair,
    })
}

/// Close a guardian signatures account to reclaim rent.
pub fn close_signatures(
    client: &RpcClient,
    payer: &Keypair,
    verify_vaa_shim: &solana_sdk::pubkey::Pubkey,
    signatures_pubkey: &solana_sdk::pubkey::Pubkey,
) -> Result<(), SubmitError> {
    let ix = CloseSignatures {
        program_id: verify_vaa_shim,
        accounts: CloseSignaturesAccounts {
            guardian_signatures: signatures_pubkey,
            refund_recipient: &payer.pubkey(),
        },
    }
    .instruction();

    let blockhash = client.get_latest_blockhash()?;
    let tx = Transaction::new_signed_with_payer(&[ix], Some(&payer.pubkey()), &[payer], blockhash);

    client.send_and_confirm_transaction_with_spinner_and_commitment(
        &tx,
        CommitmentConfig::confirmed(),
    )?;

    Ok(())
}
