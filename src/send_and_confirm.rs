use std::{
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig},
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::Miner;

const RPC_RETRIES: usize = 0;
const SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 10;
const CONFIRM_RETRIES: usize = 10;
const PARALLEL_TRANSACTIONS: usize = 5; // 并行提交的交易数量

const CONFIRM_DELAY: u64 = 1000;
const GATEWAY_DELAY: u64 = 500;

impl Miner {
    pub async fn send_and_confirm(
        &self,
        ixs: &[Instruction],
        dynamic_cus: bool,
        skip_confirm: bool,
    ) -> ClientResult<Signature> {
        let mut stdout = stdout();
        let signer = self.signer();
        let client = self.rpc_client.clone();

        // Return error if balance is zero
        let balance = client.get_balance(&signer.pubkey()).await.unwrap();
        if balance <= 0 {
            return Err(ClientError {
                request: None,
                kind: ClientErrorKind::Custom("Insufficient SOL balance".into()),
            });
        }

        // Build tx
        let (mut hash, mut slot) = client
            .get_latest_blockhash_with_commitment(self.rpc_client.commitment())
            .await
            .unwrap();
        let mut send_cfg = RpcSendTransactionConfig {
            skip_preflight: true,
            preflight_commitment: Some(CommitmentLevel::Finalized),
            encoding: Some(UiTransactionEncoding::Base64),
            max_retries: Some(10),
            min_context_slot: Some(slot),
        };
        let mut tx = Transaction::new_with_payer(ixs, Some(&signer.pubkey()));

        // Simulate if necessary
        if dynamic_cus {
            // ... (Simulation code remains the same)
        }

        // Submit tx in parallel
        tx.sign(&[&signer], hash);
        let mut handles = vec![];
        for _ in 0..PARALLEL_TRANSACTIONS {
            let client = self.rpc_client.clone();
            let signer = self.signer();
            let tx_copy = tx.clone();
            let send_cfg_copy = send_cfg.clone();
            let handle = tokio::spawn(async move {
                let mut attempts = 0;
                loop {
                    match client.send_transaction_with_config(&tx_copy, send_cfg_copy).await {
                        Ok(sig) => {
                            println!("Transaction submitted: {:?}", sig);
                            return Ok(sig);
                        }
                        Err(err) => {
                            println!("Error submitting transaction: {:?}", err);
                            attempts += 1;
                            if attempts > GATEWAY_RETRIES {
                                return Err(ClientError {
                                    request: None,
                                    kind: ClientErrorKind::Custom("Max retries".into()),
                                });
                            }
                            std::thread::sleep(Duration::from_millis(GATEWAY_DELAY));
                        }
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all transactions to complete
        let mut sigs = vec![];
        for handle in handles {
            match handle.await {
                Ok(result) => {
                    if let Ok(sig) = result {
                        sigs.push(sig);
                    }
                }
                Err(err) => {
                    println!("Error in parallel transaction: {:?}", err);
                }
            }
        }

        // Confirm transactions
        if skip_confirm {
            if let Some(sig) = sigs.first() {
                return Ok(*sig);
            } else {
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("No transaction submitted successfully".into()),
                });
            }
        }

        for _ in 0..CONFIRM_RETRIES {
            std::thread::sleep(Duration::from_millis(CONFIRM_DELAY));
            match client.get_signature_statuses(&sigs).await {
                Ok(signature_statuses) => {
                    println!("Confirms: {:?}", signature_statuses.value);
                    for signature_status in signature_statuses.value {
                        if let Some(signature_status) = signature_status.as_ref() {
                            if signature_status.confirmation_status.is_some() {
                                let current_commitment =
                                    signature_status.confirmation_status.as_ref().unwrap();
                                match current_commitment {
                                    TransactionConfirmationStatus::Processed => {}
                                    TransactionConfirmationStatus::Confirmed
                                    | TransactionConfirmationStatus::Finalized => {
                                        println!("Transaction landed!");
                                        if let Some(sig) = sigs.first() {
                                            return Ok(*sig);
                                        }
                                    }
                                }
                            } else {
                                println!("No status");
                            }
                        }
                    }
                }
                Err(err) => {
                    println!("Error: {:?}", err);
                }
            }
        }
        println!("Transactions did not land");
        Err(ClientError {
            request: None,
            kind: ClientErrorKind::Custom("Transactions did not land".into()),
        })
    }
}
