use actix::{Actor, Addr};
use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use clap::Parser;
use futures::stream::{self, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use std::{
    collections::HashMap,
    process::{Command, Output},
    str::FromStr,
    sync::atomic::Ordering,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, BufReader as TokioBufReader};
use tracing::{error, info, Level};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    actors::{consumer::Consumer, producer::Producer, Monitor, RegisterTxnPlan},
    config::{BenchConfig, ContractConfig},
    eth::EthHttpCli,
    txn_plan::{
        addr_pool::AddressPool,
        constructor::FaucetTreePlanBuilder,
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
    info!(
        "{} faucet distribution will proceed in {} levels.",
        faucet_name, total_faucet_levels
    );

    for level in 0..total_faucet_levels {
        info!(
            "Starting {} faucet distribution for LEVEL {}...",
            faucet_name, level
        );

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
        info!(
            "{} faucet distribution for LEVEL {} completed successfully.",
            faucet_name, level
        );
    }
    info!(
        "All {} faucet distribution levels are complete.",
        faucet_name
    );
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
            info!(
                "Benchmark duration of {} seconds reached. Stopping.",
                duration_secs
            );
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
            info!(
                "Benchmark duration of {} seconds reached. Stopping.",
                duration_secs
            );
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
    init_nonce_map.insert(
        faucet_address,
        eth_client.get_txn_count(faucet_address).await.unwrap(),
    );
    Arc::new(init_nonce_map)
}

async fn start_bench() -> Result<()> {
    let args = Args::parse();
    let benchmark_config = BenchConfig::load(&args.config).unwrap();
    assert!(benchmark_config.accounts.num_accounts >= benchmark_config.target_tps as usize);

    // Initialize tracing
    let log_path = benchmark_config.log_path.trim();

    if log_path.is_empty() || log_path.eq_ignore_ascii_case("console") {
        // Console only
        tracing_subscriber::fmt()
            .with_max_level(Level::INFO)
            .with_file(false)
            .with_line_number(false)
            .with_thread_ids(false)
            .init();
    } else {
        // File logging
        let path = std::path::Path::new(log_path);
        let directory = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let filename = path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("gravity_bench.log"));
        
        // Ensure directory exists
        std::fs::create_dir_all(directory).unwrap();

        let file_appender = tracing_appender::rolling::daily(directory, filename);
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stdout)
                    .with_file(false)
                    .with_line_number(false)
                    .with_thread_ids(false)
            )
            .init();
    }

    let contract_config = if args.recover {
        info!("Starting in recovery mode...");
        let contract_config =
            ContractConfig::load_from_file(&benchmark_config.contract_config_path).unwrap();
        contract_config
    } else {
        info!("Starting in normal mode...");
        let mut command = format!(
            "python scripts/deploy.py --private-key \"{}\" --num-tokens {} --output-file \"{}\" --rpc-url \"{}\"",
            benchmark_config.faucet.private_key,
            benchmark_config.num_tokens,
            benchmark_config.contract_config_path,
            benchmark_config.nodes[0].rpc_url
        );
        if benchmark_config.enable_swap_token {
            command.push_str(" --enable-swap-token");
        }
        let res = run_command(&command).unwrap();
        info!("{}", String::from_utf8_lossy(&res.stdout));
        let contract_config = ContractConfig::load_from_file(
            &benchmark_config.contract_config_path,
        )
        .unwrap_or_else(|e| {
            panic!("Contract config file not found {}", e);
        });
        contract_config
    };
    let mut accout_generator = AccountGenerator::with_capacity(
        PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap(),
    );
    let account_ids = accout_generator
        .gen_account(0, benchmark_config.accounts.num_accounts as u64)
        .unwrap();
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
    let mut start_nonce = contract_config.get_all_token().len() as u64;
    let eth_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
        benchmark_config.faucet.faucet_level as usize,
        benchmark_config.faucet.fauce_eth_balance,
        &benchmark_config.faucet.private_key,
        start_nonce,
        account_addresses.clone(),
        Arc::new(EthFaucetTxnBuilder),
        U256::from(benchmark_config.num_tokens)
            * U256::from(21000)
            * U256::from(1000_000_000_000u64),
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
    )
    .start();

    let tokens = contract_config.get_all_token();
    let mut tokens_plan = Vec::new();
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

    let account_manager = accout_generator.to_manager();

    let address_pool: Arc<dyn AddressPool> = match benchmark_config.address_pool_type {
        config::AddressPoolType::Random => {
            info!("Using RandomAddressPool");
            Arc::new(
                txn_plan::addr_pool::managed_address_pool::RandomAddressPool::new(
                    account_ids.clone(),
                    account_manager.clone(),
                ),
            )
        }
        config::AddressPoolType::Weighted => {
            info!("Using WeightedAddressPool");
            Arc::new(
                txn_plan::addr_pool::weighted_address_pool::WeightedAddressPool::new(
                    account_ids.clone(),
                    account_manager.clone(),
                ),
            )
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

    let producer = Producer::new(
        address_pool.clone(),
        consumer,
        monitor,
        account_manager.clone(),
    )
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

    let tps = benchmark_config.target_tps as usize;
    let duration_secs = benchmark_config.performance.duration_secs;
    if benchmark_config.enable_swap_token {
        info!("bench uniswap");
        test_uniswap(
            address_pool,
            chain_id,
            contract_config,
            &producer,
            tps,
            duration_secs,
        )
        .await?;
    } else {
        info!("bench erc20 transfer");
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
            let mut init_nonce = None;
            {
                for _ in 0..5 {
                    let res = tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        client.get_txn_count(addr),
                    )
                    .await;
                    if res.is_ok() {
                        init_nonce = res.ok();
                        break;
                    }
                }
            }
            match &init_nonce {
                Some(Ok(init_nonce)) => {
                    nonce.store(*init_nonce, Ordering::Relaxed);
                    pb.inc(1);
                }
                _ => {
                    tracing::error!("Failed to get nonce for address: {:?}", init_nonce);
                    panic!("Failed to get nonce for address: {}", addr);
                }
            }
        }
    });

    stream::iter(tasks)
        .buffer_unordered(1024)
        .collect::<Vec<_>>()
        .await;

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
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C signal handler");
        println!("Received Ctrl+C, saving heap profile...");
    };
    tokio::select! {
        _ = res => {},
        _ = ctrl_c => {},
    }
    Ok(())
}
