use actix::{Actor, Addr};
use alloy::{
    eips::Encodable2718,
    network::TransactionBuilder,
    primitives::{Address, Bytes, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use clap::Parser;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    process::{Command, Output},
    str::FromStr,
    sync::atomic::Ordering,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, BufReader as TokioBufReader};
use tracing::{error, info, warn};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use crate::{
    actors::{consumer::Consumer, producer::Producer, Monitor, RegisterTxnPlan},
    config::{BenchConfig, ContractConfig, WorkloadType},
    eth::{EthHttpCli, TxnBuilder, BENCH_MAX_FEE_PER_GAS, BENCH_MAX_PRIORITY_FEE_PER_GAS},
    txn_plan::{
        addr_pool::AddressPool,
        constructor::{
            delegate_contract_bytecode, FaucetTreePlanBuilder, EIP7702_DELEGATE_DEPLOY_GAS_LIMIT,
        },
        faucet_txn_builder::{Erc20FaucetTxnBuilder, EthFaucetTxnBuilder, FaucetTxnBuilder},
        PlanBuilder, TxnPlan,
    },
    util::gen_account::{AccountGenerator, AccountManager},
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = false)]
    recover: bool,

    #[arg(long, default_value = "bench_config.toml")]
    config: String,

    #[arg(long, default_value_t = false)]
    faucet_only: bool,

    #[arg(long)]
    accounts_output: Option<String>,
}

/// Snapshot persisted between runs for deterministic recovery.
/// Contains the seed for account generation and the faucet nonce
/// at the start of distribution, so that recovery can correctly
/// skip already-completed faucet levels.
#[derive(Serialize, Deserialize, Debug)]
struct Snapshot {
    seed: String,
    faucet_start_nonce: u64,
    /// EIP-7702 delegate contract address (set when workload = eip7702).
    #[serde(default)]
    eip7702_delegate: Option<String>,
}

// mod uniswap;
mod util;
// Module declarations
// mod accounts;
mod actors;
mod config;
// mod engine;
mod eth;
mod txn_plan;

#[allow(unused)]
async fn load_accounts_from_file(
    path: &str,
) -> Result<HashMap<Arc<Address>, Arc<PrivateKeySigner>>> {
    let file = tokio::fs::File::open(path).await?;
    let reader = TokioBufReader::new(file);
    let mut lines = reader.lines();
    let mut accounts = HashMap::new();

    while let Some(line) = lines.next_line().await? {
        let parts: Vec<&str> = line.split(", ").collect();
        if parts.len() == 2 {
            let signer = PrivateKeySigner::from_str(parts[1])?;
            let address = signer.address();
            accounts.insert(Arc::new(address), Arc::new(signer));
        } else {
            return Err(anyhow::anyhow!("Invalid line in accounts file: {}", line));
        }
    }
    Ok(accounts)
}

async fn run_plan(
    plan: Box<dyn TxnPlan>,
    producer: &Addr<Producer>,
) -> Result<tokio::sync::oneshot::Receiver<Result<(), anyhow::Error>>> {
    let (exec_plan, rx) = RegisterTxnPlan::new(plan);
    producer.send(exec_plan).await??;
    Ok(rx)
}

async fn execute_faucet_distribution<T: FaucetTxnBuilder + 'static>(
    faucet_builder: Arc<FaucetTreePlanBuilder<T>>,
    chain_id: u64,
    producer: &Addr<Producer>,
    faucet_name: &str,
    wait_duration_secs: u64,
    init_nonce_map: Arc<HashMap<Address, u64>>,
) -> Result<()> {
    let total_faucet_levels = faucet_builder.total_levels();
    info!("{} faucet distribution will proceed in {} levels.", faucet_name, total_faucet_levels);

    for level in 0..total_faucet_levels {
        info!("Starting {} faucet distribution for LEVEL {}...", faucet_name, level);

        let faucet_level_plan =
            faucet_builder.create_plan_for_level(level, init_nonce_map.clone(), chain_id);

        let rx = run_plan(faucet_level_plan, producer).await;
        match rx {
            Ok(rx) => rx.await??,
            Err(err) => tracing::error!("Failed to run plan: {}", err),
        }
        if wait_duration_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(wait_duration_secs)).await;
        }
        info!("{} faucet distribution for LEVEL {} completed successfully.", faucet_name, level);
    }
    info!("All {} faucet distribution levels are complete.", faucet_name);
    Ok(())
}

#[allow(unused)]
async fn test_uniswap(
    address_pool: Arc<dyn AddressPool>,
    chain_id: u64,
    contract_config: ContractConfig,
    producer: &Addr<Producer>,
    tps: usize,
    duration_secs: u64,
) -> Result<()> {
    let mut rx_vec = Vec::new();
    for token in contract_config.get_all_token_addresses() {
        info!("approving token: {}", token);
        let approve_token = PlanBuilder::approve_token(
            chain_id,
            token,
            contract_config.get_router_address().unwrap(),
        );
        let rx = run_plan(approve_token, producer).await?;
        rx_vec.push(rx);
    }
    for rx in rx_vec {
        rx.await??;
    }
    let start_time = Instant::now();
    loop {
        if duration_secs > 0 && start_time.elapsed() >= Duration::from_secs(duration_secs) {
            info!("Benchmark duration of {} seconds reached. Stopping.", duration_secs);
            break;
        }
        let plan = PlanBuilder::swap_token_to_token(
            chain_id,
            U256::from(1000),
            contract_config.get_liquidity_pairs().clone(),
            address_pool.clone(),
            contract_config.get_router_address().unwrap(),
            tps,
        );
        let rx = match run_plan(plan, producer).await {
            Ok(rx) => rx,
            Err(e) => {
                info!("Failed to submit plan: {}. Retrying...", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        // Spawn waiting in background to allow parallel plan execution
        tokio::spawn(async move {
            if let Err(e) = rx.await {
                error!("Plan execution failed: {:?}", e);
            }
        });
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn test_erc20_transfer(
    address_pool: Arc<dyn AddressPool>,
    chain_id: u64,
    contract_config: ContractConfig,
    producer: &Addr<Producer>,
    tps: usize,
    duration_secs: u64,
) -> Result<()> {
    let start_time = Instant::now();
    loop {
        if duration_secs > 0 && start_time.elapsed() >= Duration::from_secs(duration_secs) {
            info!("Benchmark duration of {} seconds reached. Stopping.", duration_secs);
            break;
        }
        // bench erc20 transfer
        let erc20_transfer = PlanBuilder::erc20_transfer(
            chain_id,
            contract_config.get_all_token_addresses().clone(),
            U256::from(1000),
            address_pool.clone(),
            tps,
        );
        let rx = match run_plan(erc20_transfer, producer).await {
            Ok(rx) => rx,
            Err(e) => {
                info!("Failed to submit plan: {}. Retrying...", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        // Spawn waiting in background to allow parallel plan execution
        tokio::spawn(async move {
            if let Err(e) = rx.await {
                error!("Plan execution failed: {:?}", e);
            }
        });
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn test_eip7702(
    chain_id: u64,
    delegate: Address,
    address_pool: Arc<dyn AddressPool>,
    producer: &Addr<Producer>,
    tps: usize,
    duration_secs: u64,
) -> Result<()> {
    let start_time = Instant::now();
    loop {
        if duration_secs > 0 && start_time.elapsed() >= Duration::from_secs(duration_secs) {
            info!("Benchmark duration of {} seconds reached. Stopping.", duration_secs);
            break;
        }
        let plan = PlanBuilder::eip7702_set_code(chain_id, delegate, address_pool.clone(), tps);
        let rx = match run_plan(plan, producer).await {
            Ok(rx) => rx,
            Err(e) => {
                info!("Failed to submit plan: {}. Retrying...", e);
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };
        tokio::spawn(async move {
            if let Err(e) = rx.await {
                error!("Plan execution failed: {:?}", e);
            }
        });
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

/// Deploy the BatchExecutor EIP-7702 delegate target from the faucet account.
/// Returns `(delegate_address, next_faucet_nonce)`.
async fn deploy_eip7702_delegate(
    eth_client: &EthHttpCli,
    faucet_key: &str,
    chain_id: u64,
    faucet_nonce: u64,
) -> Result<(Address, u64)> {
    let signer = PrivateKeySigner::from_str(faucet_key)
        .context("invalid faucet private key for delegate deploy")?;
    let faucet_addr = signer.address();
    let bytecode = Bytes::from(delegate_contract_bytecode());

    let tx_request = alloy::rpc::types::TransactionRequest::default()
        .with_from(faucet_addr)
        .with_deploy_code(bytecode)
        .with_nonce(faucet_nonce)
        .with_chain_id(chain_id)
        .with_max_priority_fee_per_gas(BENCH_MAX_PRIORITY_FEE_PER_GAS)
        .with_max_fee_per_gas(BENCH_MAX_FEE_PER_GAS)
        .with_gas_limit(EIP7702_DELEGATE_DEPLOY_GAS_LIMIT);

    let envelope = TxnBuilder::build_and_sign_transaction(tx_request, &signer)?;
    let tx_hash = eth_client.send_raw_tx(envelope.encoded_2718()).await?;
    info!("EIP-7702 delegate deploy submitted: {:?}", tx_hash);

    let receipt = eth_client
        .wait_for_receipt(tx_hash, Duration::from_secs(120), Duration::from_millis(500))
        .await
        .context("waiting for EIP-7702 delegate deploy receipt")?;

    let status = receipt.status();
    if !status {
        return Err(anyhow::anyhow!(
            "EIP-7702 delegate deploy failed (status=0), tx={:?}",
            tx_hash
        ));
    }

    let contract_address = receipt.contract_address.ok_or_else(|| {
        anyhow::anyhow!("EIP-7702 delegate deploy receipt missing contractAddress")
    })?;

    // Sanity: expected CREATE address from (sender, nonce)
    let expected = faucet_addr.create(faucet_nonce);
    if contract_address != expected {
        warn!(
            "delegate address from receipt ({:#x}) != CREATE prediction ({:#x}); using receipt",
            contract_address, expected
        );
    }

    let code = eth_client.get_code_at(contract_address).await?;
    if code.is_empty() {
        return Err(anyhow::anyhow!("no code at deployed delegate {:#x}", contract_address));
    }

    info!("EIP-7702 delegate deployed at {:#x}", contract_address);
    Ok((contract_address, faucet_nonce + 1))
}

/// Recover a previously deployed EIP-7702 delegate from snapshot or faucet CREATE history.
async fn recover_eip7702_delegate(
    eth_client: &EthHttpCli,
    snapshot_delegate: Option<&str>,
    faucet_addr: Address,
    faucet_start_nonce: u64,
) -> Result<Address> {
    if let Some(addr_str) = snapshot_delegate {
        let addr = Address::from_str(addr_str)
            .with_context(|| format!("invalid eip7702_delegate in snapshot: {}", addr_str))?;
        let code = eth_client.get_code_at(addr).await?;
        if code.is_empty() {
            return Err(anyhow::anyhow!(
                "snapshot eip7702_delegate {:#x} has no code; re-run without --recover",
                addr
            ));
        }
        info!("Recovered EIP-7702 delegate from snapshot: {:#x}", addr);
        return Ok(addr);
    }

    // Fallback: assume delegate was the last CREATE from faucet before cascade
    // (deploy uses faucet_start_nonce - 1 if we deployed once then saved snapshot
    // after deploy... we always save snapshot with the address when possible).
    if faucet_start_nonce == 0 {
        return Err(anyhow::anyhow!(
            "cannot recover EIP-7702 delegate: no snapshot address and faucet nonce is 0"
        ));
    }
    let addr = faucet_addr.create(faucet_start_nonce - 1);
    let code = eth_client.get_code_at(addr).await?;
    if code.is_empty() {
        return Err(anyhow::anyhow!(
            "no code at predicted delegate {:#x}; re-run faucet without --recover",
            addr
        ));
    }
    info!("Recovered EIP-7702 delegate via CREATE prediction: {:#x}", addr);
    Ok(addr)
}

fn run_command(command: &str) -> Result<Output> {
    let output = Command::new("bash").arg("-c").arg(command).output()?; // ? will return Err early if there's an error

    if output.status.success() {
        Ok(output)
    } else {
        panic!("command failed: {:?}", output);
    }
}

async fn get_init_nonce_map(
    accout_generator: AccountManager,
    faucet_private_key: &str,
    eth_client: Arc<EthHttpCli>,
) -> Arc<HashMap<Address, u64>> {
    let mut init_nonce_map = accout_generator.init_nonce_map();
    let faucet_signer = PrivateKeySigner::from_str(faucet_private_key).unwrap();
    let faucet_address = faucet_signer.address();
    init_nonce_map
        .insert(faucet_address, eth_client.get_pending_txn_count(faucet_address).await.unwrap());
    Arc::new(init_nonce_map)
}

async fn start_bench() -> Result<()> {
    let args = Args::parse();
    let benchmark_config = BenchConfig::load(&args.config).unwrap();
    assert!(benchmark_config.accounts.num_accounts >= benchmark_config.target_tps as usize);
    let workload = benchmark_config.resolved_workload();

    // Initialize tracing
    let log_path = benchmark_config.log_path.trim();
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let _guard = if log_path.is_empty() || log_path.eq_ignore_ascii_case("console") {
        // Console only
        tracing_subscriber::fmt()
            .with_env_filter(env_filter)
            .with_file(false)
            .with_line_number(false)
            .with_thread_ids(false)
            .init();
        None
    } else {
        // File logging
        let path = std::path::Path::new(log_path);
        let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let file_stem = path.file_stem().unwrap_or_else(|| std::ffi::OsStr::new("gravity_bench"));
        let extension = path.extension().unwrap_or_else(|| std::ffi::OsStr::new("log"));

        // Ensure directory exists
        std::fs::create_dir_all(directory).unwrap();

        let timestamp = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
        let new_filename = format!(
            "{}.{}.{}",
            file_stem.to_string_lossy(),
            timestamp,
            extension.to_string_lossy()
        );
        let full_path = directory.join(new_filename);

        let file = std::fs::File::create(&full_path).unwrap();
        let (non_blocking, guard) = tracing_appender::non_blocking(file);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().with_writer(non_blocking).with_ansi(false))
            .init();

        println!("Logging to file: {:?}", full_path);
        Some(guard)
    };

    info!("Resolved workload: {:?}", workload);

    // contract_config is only required for erc20 / swap workloads.
    // eip7702 only needs ETH funding + a delegate target contract.
    let (contract_config, seed, snapshot_start_nonce, snapshot_eip7702_delegate) = if args.recover {
        info!("Starting in recovery mode...");
        let snapshot_json = std::fs::read_to_string("snapshot.json").unwrap_or_else(|e| {
            panic!("Failed to read snapshot.json in recovery mode: {}", e);
        });
        let snapshot: Snapshot = serde_json::from_str(&snapshot_json).unwrap_or_else(|e| {
            panic!("Failed to parse snapshot.json: {}", e);
        });
        let seed_bytes =
            hex::decode(snapshot.seed.trim()).expect("Invalid hex seed in snapshot.json");
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&seed_bytes);
        info!("Recovered faucet_start_nonce: {}", snapshot.faucet_start_nonce);

        let contract_config = match workload {
            WorkloadType::Eip7702 => None,
            WorkloadType::Erc20 | WorkloadType::Swap => Some(
                ContractConfig::load_from_file(&benchmark_config.contract_config_path).unwrap(),
            ),
        };
        (contract_config, seed, Some(snapshot.faucet_start_nonce), snapshot.eip7702_delegate)
    } else {
        info!("Starting in normal mode...");
        let contract_config = match workload {
            WorkloadType::Eip7702 => {
                info!("Skipping deploy.py (EIP-7702 workload only needs ETH + delegate)");
                None
            }
            WorkloadType::Erc20 | WorkloadType::Swap => {
                let mut command = format!(
                    "python scripts/deploy.py --private-key \"{}\" --num-tokens {} --output-file \"{}\" --rpc-url \"{}\"",
                    benchmark_config.faucet.private_key,
                    benchmark_config.num_tokens,
                    benchmark_config.contract_config_path,
                    benchmark_config.nodes[0].rpc_url
                );
                if matches!(workload, WorkloadType::Swap) {
                    command.push_str(" --enable-swap-token");
                }
                let res = run_command(&command).unwrap();
                info!("{}", String::from_utf8_lossy(&res.stdout));
                Some(
                    ContractConfig::load_from_file(&benchmark_config.contract_config_path)
                        .unwrap_or_else(|e| {
                            panic!("Contract config file not found {}", e);
                        }),
                )
            }
        };

        let seed: [u8; 32] = rand::random();
        (contract_config, seed, None, None)
    };
    tracing::info!("Account generator seed: 0x{}", hex::encode(seed));

    let mut accout_generator = AccountGenerator::with_capacity(
        PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap(),
        seed,
    );
    let account_ids =
        accout_generator.gen_account(0, benchmark_config.accounts.num_accounts as u64).unwrap();
    let account_addresses = Arc::new({
        account_ids
            .iter()
            .map(|&id| Arc::new(accout_generator.get_address_by_id(id)))
            .collect::<Vec<_>>()
    });
    // Create EthHttpCli instance
    let eth_clients: Vec<Arc<EthHttpCli>> = benchmark_config
        .nodes
        .iter()
        .map(|node| {
            let client = EthHttpCli::new(&node.rpc_url, node.chain_id).unwrap();
            Arc::new(client)
        })
        .collect();

    let chain_id = benchmark_config.nodes[0].chain_id;

    info!("Initializing Faucet constructor...");
    let faucet_address =
        PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap().address();
    let on_chain_nonce = eth_clients[0].get_pending_txn_count(faucet_address).await.unwrap();
    // Clamp configured faucet_eth_balance to the actual on-chain balance to prevent
    // U256 underflow inside FaucetTreePlanBuilder::new when (gas_budget * total_txns)
    // exceeds the configured amount. Underflow produces ~2^256 funding values that
    // permanently strip ENOUGH_BALANCE in the txpool and silently deadlock cascade.
    // retry_with_backoff retries 3× with 5s/10s/10s — up to ~25s wall time on
    // a dead RPC before this expect() fires.
    let on_chain_faucet_balance = eth_clients[0]
        .get_balance(&faucet_address)
        .await
        .expect("failed to query faucet on-chain balance");
    let configured_faucet_eth = benchmark_config.faucet.faucet_eth_balance;
    // Pick an on-chain-aware faucet ceiling for the cascade math:
    //   - If on-chain balance < configured: clamp DOWN to on-chain reality so the
    //     cascade can't be sized for funds we don't have (avoids U256 underflow in
    //     FaucetTreePlanBuilder when configured exceeds the real balance).
    //   - If on-chain balance >= configured: use 99% of on-chain (1% headroom for
    //     fees outside the cascade) to scale the cascade UP to what's actually
    //     available, rather than under-using a well-funded faucet.
    // The FaucetTreePlanBuilder assert is the final backstop for absurd inputs.
    let effective_faucet_eth = if on_chain_faucet_balance < configured_faucet_eth {
        tracing::warn!(
            "faucet_eth_balance ({}) exceeds on-chain balance ({}); clamping down.",
            configured_faucet_eth,
            on_chain_faucet_balance
        );
        on_chain_faucet_balance
    } else if configured_faucet_eth < on_chain_faucet_balance {
        // Configured is smaller than on-chain. Scale the cascade up to (on-chain - 1%)
        // so a well-funded faucet isn't under-used; the 1% headroom covers misc fees
        // outside the cascade.
        // Note: if on_chain == 0, headroom == 0 and usable == 0 — the cascade builder's
        // assert will catch this with a clear panic before any U256 underflow.
        let headroom = on_chain_faucet_balance / U256::from(100);
        let usable = on_chain_faucet_balance - headroom;
        usable
    } else {
        configured_faucet_eth
    };
    info!(
        "Faucet ETH plan: configured={}, on_chain={}, effective={}",
        configured_faucet_eth, on_chain_faucet_balance, effective_faucet_eth
    );
    // In recover mode, use the saved start_nonce so that the skip logic
    // correctly identifies already-completed Level 0 transactions.
    // In normal mode, use the current on-chain nonce.
    let mut start_nonce = snapshot_start_nonce.unwrap_or(on_chain_nonce);
    info!("Faucet on-chain nonce: {}, using start_nonce: {}", on_chain_nonce, start_nonce);

    // EIP-7702: deploy (or recover) the shared delegate target before the ETH
    // cascade so the cascade's faucet_start_nonce is exclusive of the deploy tx.
    let eip7702_delegate = if matches!(workload, WorkloadType::Eip7702) {
        let delegate = if args.recover {
            recover_eip7702_delegate(
                eth_clients[0].as_ref(),
                snapshot_eip7702_delegate.as_deref(),
                faucet_address,
                start_nonce,
            )
            .await?
        } else {
            let (addr, next_nonce) = deploy_eip7702_delegate(
                eth_clients[0].as_ref(),
                &benchmark_config.faucet.private_key,
                chain_id,
                start_nonce,
            )
            .await?;
            start_nonce = next_nonce;
            addr
        };
        Some(delegate)
    } else {
        None
    };

    // Save snapshot in normal mode (after we know the cascade start_nonce and
    // any EIP-7702 delegate address).
    if !args.recover {
        let snapshot = Snapshot {
            seed: hex::encode(seed),
            faucet_start_nonce: start_nonce,
            eip7702_delegate: eip7702_delegate.map(|a| format!("{:#x}", a)),
        };
        let snapshot_json = serde_json::to_string_pretty(&snapshot).unwrap();
        std::fs::write("snapshot.json", &snapshot_json).unwrap_or_else(|e| {
            panic!("Failed to write snapshot.json: {}", e);
        });
        info!("Snapshot saved to snapshot.json");
    }
    // ERC20/swap need extra gas headroom for later token ops.
    // EIP-7702 multiSend circulates tiny ETH among workers; leave 0 so the
    // cascade pushes full share to leaves (gas + multiSend principal).
    let remained_eth = match workload {
        WorkloadType::Eip7702 => U256::ZERO,
        WorkloadType::Erc20 | WorkloadType::Swap => {
            U256::from(benchmark_config.num_tokens)
                * U256::from(21000)
                * U256::from(1000_000_000_000u64)
        }
    };
    let eth_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
        benchmark_config.faucet.faucet_level as usize,
        effective_faucet_eth,
        &benchmark_config.faucet.private_key,
        start_nonce,
        account_addresses.clone(),
        Arc::new(EthFaucetTxnBuilder),
        remained_eth,
        &mut accout_generator,
    )
    .await
    .unwrap();
    if args.recover {
        init_nonce(&mut accout_generator, eth_clients[0].clone()).await;
    }
    let monitor = Monitor::new_with_clients(
        eth_clients.clone(),
        benchmark_config.performance.max_pool_size,
        benchmark_config.performance.sampling,
    )
    .start();

    let tokens = contract_config.as_ref().map(|c| c.get_all_token()).unwrap_or_default();
    let mut tokens_plan = Vec::new();
    // Skip ERC20 token cascade for EIP-7702 (workers only need ETH for gas).
    if !matches!(workload, WorkloadType::Eip7702) {
        for token in &tokens {
            start_nonce += benchmark_config.faucet.faucet_level as u64;
            info!("distributing token: {}", token.address);
            let token_address = Address::from_str(&token.address).unwrap();
            let faucet_token_balance = U256::from_str(&token.faucet_balance).unwrap();
            info!("balance of token: {}", faucet_token_balance);
            let token_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
                benchmark_config.faucet.faucet_level as usize,
                faucet_token_balance,
                &benchmark_config.faucet.private_key,
                start_nonce,
                account_addresses.clone(),
                Arc::new(Erc20FaucetTxnBuilder::new(token_address)),
                U256::ZERO,
                &mut accout_generator,
            )
            .await
            .unwrap();
            tokens_plan.push(token_faucet_builder);
        }
    }

    let account_manager = accout_generator.to_manager();

    if let Some(path) = &args.accounts_output {
        save_accounts(&account_manager, path).await?;
    }

    let address_pool: Arc<dyn AddressPool> = match benchmark_config.address_pool_type {
        config::AddressPoolType::Random => {
            info!("Using RandomAddressPool");
            Arc::new(txn_plan::addr_pool::managed_address_pool::RandomAddressPool::new(
                account_ids.clone(),
                account_manager.clone(),
            ))
        }
        config::AddressPoolType::Weighted => {
            info!("Using WeightedAddressPool");
            Arc::new(txn_plan::addr_pool::weighted_address_pool::WeightedAddressPool::new(
                account_ids.clone(),
                account_manager.clone(),
            ))
        }
    };

    // Use the same client instances for Consumer to share metrics
    let eth_providers: Vec<EthHttpCli> = eth_clients
        .iter()
        .map(|client| (**client).clone()) // Clone the actual EthHttpCli instead of creating new ones
        .collect();

    let consumer = Consumer::new_with_providers(
        eth_providers,
        benchmark_config.performance.num_senders,
        monitor.clone(),
        benchmark_config.performance.max_pool_size,
        Some(benchmark_config.target_tps as u32),
    )
    .start();
    let init_nonce_map = get_init_nonce_map(
        account_manager.clone(),
        benchmark_config.faucet.private_key.as_str(),
        eth_clients[0].clone(),
    )
    .await;

    let producer = Producer::new(address_pool.clone(), consumer, monitor, account_manager.clone())
        .await
        .unwrap()
        .start();
    execute_faucet_distribution(
        eth_faucet_builder,
        chain_id,
        &producer,
        "ETH",
        benchmark_config.faucet.wait_duration_secs,
        init_nonce_map.clone(),
    )
    .await?;

    for (token_plan, token) in tokens_plan.into_iter().zip(tokens.iter()) {
        execute_faucet_distribution(
            token_plan,
            chain_id,
            &producer,
            &format!("Token {}", token.symbol),
            benchmark_config.faucet.wait_duration_secs,
            init_nonce_map.clone(),
        )
        .await?;
    }

    if args.faucet_only {
        info!("Faucet distribution complete. Faucet-only mode enabled, exiting.");
        return Ok(());
    }

    let tps = benchmark_config.target_tps as usize;
    let duration_secs = benchmark_config.performance.duration_secs;
    match workload {
        WorkloadType::Swap => {
            info!("bench uniswap");
            let contract_config = contract_config.expect("swap workload requires contract config");
            test_uniswap(address_pool, chain_id, contract_config, &producer, tps, duration_secs)
                .await?;
        }
        WorkloadType::Erc20 => {
            info!("bench erc20 transfer");
            let contract_config = contract_config.expect("erc20 workload requires contract config");
            test_erc20_transfer(
                address_pool,
                chain_id,
                contract_config,
                &producer,
                tps,
                duration_secs,
            )
            .await?;
        }
        WorkloadType::Eip7702 => {
            let delegate = eip7702_delegate.expect("eip7702 delegate must be set");
            info!(
                "bench EIP-7702 SetCode + ETH multiSend (delegate={:#x}, batch={})",
                delegate,
                txn_plan::constructor::EIP7702_DEFAULT_BATCH_SIZE
            );
            test_eip7702(chain_id, delegate, address_pool, &producer, tps, duration_secs).await?;
        }
    }
    Ok(())
}

async fn init_nonce(accout_generator: &mut AccountGenerator, eth_client: Arc<EthHttpCli>) {
    tracing::info!("Initializing nonce...");

    // Collect all accounts first to get total count
    let accounts: Vec<_> = accout_generator.accouts_nonce_iter().collect();
    let total_accounts = accounts.len() as u64;

    // Create progress bar
    let pb = ProgressBar::new(total_accounts);
    pb.set_style(
        ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({per_sec}, ETA: {eta})")
            .unwrap()
            .progress_chars("#>-"),
    );

    let pb = Arc::new(pb);
    let start_time = Instant::now();

    let tasks = accounts.into_iter().map(|(account, nonce)| {
        let client = eth_client.clone();
        let addr = account.clone();
        let pb = pb.clone();
        async move {
            // Retry up to 5 times. The previous version checked `res.is_ok()` which
            // is the tokio timeout outer Result — it stays Ok even when the inner
            // RPC call returns Err (connection drop, etc.), so the loop would break
            // on the first RPC error and panic. Match both layers so we actually
            // retry on RPC errors and timeouts.
            let mut init_nonce: Option<u64> = None;
            for attempt in 0..5 {
                // 60s per nonce fetch (vs default 10s): nonce init runs early when the
                // chain may still be initializing; tolerating slow first responses avoids
                // spurious panics during startup.
                match tokio::time::timeout(
                    std::time::Duration::from_secs(60),
                    client.get_pending_txn_count(addr),
                )
                .await
                {
                    Ok(Ok(n)) => {
                        init_nonce = Some(n);
                        break;
                    }
                    Ok(Err(e)) => {
                        tracing::warn!(
                            "nonce init RPC error for {} (attempt {}/5): {}",
                            addr,
                            attempt + 1,
                            e
                        );
                    }
                    Err(_) => {
                        tracing::warn!(
                            "nonce init timeout for {} (attempt {}/5)",
                            addr,
                            attempt + 1
                        );
                    }
                }
                if attempt < 4 {
                    tokio::time::sleep(std::time::Duration::from_millis(
                        500 * (attempt as u64 + 1),
                    ))
                    .await;
                }
            }
            match init_nonce {
                Some(n) => {
                    nonce.store(n, Ordering::Relaxed);
                    pb.inc(1);
                }
                None => {
                    tracing::error!("Failed to get nonce for {} after 5 retries", addr);
                    panic!("Failed to get nonce for address: {}", addr);
                }
            }
        }
    });

    // 16 concurrent nonce fetches: high enough to saturate RPC parallelism on
    // typical clusters, low enough to avoid overwhelming a single PFN's RPC
    // handler thread pool when num_accounts is large.
    stream::iter(tasks).buffer_unordered(16).collect::<Vec<_>>().await;

    pb.finish_with_message("Done");
    let elapsed = start_time.elapsed();
    let rate = total_accounts as f64 / elapsed.as_secs_f64();
    tracing::info!(
        "Nonce initialized: {} accounts in {:.2}s ({:.2} accounts/sec)",
        total_accounts,
        elapsed.as_secs_f64(),
        rate
    );
}

const HEADER: &str = "Address, Private Key";
async fn save_accounts(account_manager: &AccountManager, path: &str) -> Result<()> {
    let file = tokio::fs::File::create(path).await?;
    let mut buffer = tokio::io::BufWriter::new(file);

    use tokio::io::AsyncWriteExt; // Import AsyncWriteExt trait
    buffer.write_all(HEADER.as_bytes()).await?;
    buffer.write_all(b"\n").await?;

    for (id, _) in account_manager.account_ids_with_nonce() {
        let signer = account_manager.get_signer_by_id(id);
        let address = signer.address();
        let private_key = hex::encode(signer.to_bytes());
        let line = format!("{:?}, {}\n", address, private_key);
        buffer.write_all(line.as_bytes()).await?;
    }

    buffer.flush().await?;
    info!("Accounts saved to {}", path);
    Ok(())
}

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[actix::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat-heap")]
    let _profiler = {
        println!("starting heap profiler...");
        dhat::Profiler::new_heap()
    };
    let res = async { start_bench().await };
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Failed to install CTRL+C signal handler");
        println!("Received Ctrl+C, saving heap profile...");
    };
    tokio::select! {
        _ = res => {},
        _ = ctrl_c => {},
    }
    Ok(())
}
