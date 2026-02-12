//! The [`SolanaConnection`] trait and its implementation for [`RpcClient`].

use solana_sdk::{
    account::Account, hash::Hash, pubkey::Pubkey, signature::Signature, transaction::Transaction,
};

/// Abstraction over Solana connectivity for resolver and executor logic.
///
/// Implemented for [`RpcClient`] (production) and for LiteSVM adapters (testing).
pub trait SolanaConnection {
    type Error: std::error::Error + Send + 'static;

    fn get_latest_blockhash(&self) -> Result<Hash, Self::Error>;

    /// Simulate a transaction and return the program return data bytes, if any.
    fn simulate_return_data(&self, tx: &Transaction) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Send a transaction and wait for confirmation.
    fn send_and_confirm(&mut self, tx: &Transaction) -> Result<Signature, Self::Error>;

    /// Fetch an account, returning `None` if it doesn't exist.
    fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, Self::Error>;
}

#[cfg(feature = "rpc")]
mod rpc_impl {
    use solana_client::rpc_client::RpcClient;
    use solana_client::rpc_config::RpcSimulateTransactionConfig;
    use solana_sdk::{
        account::Account, commitment_config::CommitmentConfig, hash::Hash, pubkey::Pubkey,
        signature::Signature, transaction::Transaction,
    };

    use super::SolanaConnection;

    impl SolanaConnection for RpcClient {
        type Error = solana_client::client_error::ClientError;

        fn get_latest_blockhash(&self) -> Result<Hash, Self::Error> {
            RpcClient::get_latest_blockhash(self)
        }

        fn simulate_return_data(&self, tx: &Transaction) -> Result<Option<Vec<u8>>, Self::Error> {
            let sim_result = self.simulate_transaction_with_config(
                tx,
                RpcSimulateTransactionConfig {
                    sig_verify: false,
                    replace_recent_blockhash: true,
                    commitment: Some(CommitmentConfig::confirmed()),
                    ..Default::default()
                },
            )?;

            let sim_value = sim_result.value;

            if let Some(err) = &sim_value.err {
                // Log simulation error details for debugging
                if let Some(logs) = &sim_value.logs {
                    for log in logs {
                        if log.contains("Error") || log.contains("error") || log.contains("failed")
                        {
                            eprintln!("  SIM LOG: {}", log);
                        }
                    }
                }
                return Err(solana_client::client_error::ClientError::from(
                    solana_client::rpc_request::RpcError::ForUser(format!(
                        "Simulation error: {:?}",
                        err
                    )),
                ));
            }

            match sim_value.return_data {
                Some(rd) => {
                    let data_bytes = base64::Engine::decode(
                        &base64::engine::general_purpose::STANDARD,
                        &rd.data.0,
                    )
                    .map_err(|e| {
                        solana_client::client_error::ClientError::from(
                            solana_client::rpc_request::RpcError::ForUser(format!(
                                "Failed to decode base64 return data: {}",
                                e
                            )),
                        )
                    })?;

                    if data_bytes.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(data_bytes))
                    }
                }
                None => Ok(None),
            }
        }

        fn send_and_confirm(&mut self, tx: &Transaction) -> Result<Signature, Self::Error> {
            self.send_and_confirm_transaction_with_spinner_and_commitment(
                tx,
                CommitmentConfig::confirmed(),
            )
        }

        fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, Self::Error> {
            match RpcClient::get_account(self, pubkey) {
                Ok(account) => Ok(Some(account)),
                Err(e) => {
                    // "AccountNotFound" is a normal case, not an error
                    let err_str = e.to_string();
                    if err_str.contains("AccountNotFound")
                        || err_str.contains("could not find account")
                    {
                        Ok(None)
                    } else {
                        Err(e)
                    }
                }
            }
        }
    }
}
