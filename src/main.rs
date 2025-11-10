use actix::{Actor, Addr};
use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use clap::Parser;
use futures::future::join_all;
use std::{
    collections::HashMap,
    process::{Command, Output},
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{info, Level};

use crate::{
    actors::{consumer::Consumer, producer::Producer, Monitor, RegisterTxnPlan},
    config::{BenchConfig, ContractConfig, IERC20},
    eth::EthHttpCli,
    txn_plan::{
        constructor::FaucetTreePlanBuilder,
        faucet_txn_builder::{Erc20FaucetTxnBuilder, EthFaucetTxnBuilder, FaucetTxnBuilder},
        PlanBuilder, TxnPlan,
    },
    util::account_generator::AccountGenerator,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
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

        let faucet_level_plan = faucet_builder.create_plan_for_level(level, chain_id);

        let rx = run_plan(faucet_level_plan, producer).await?;
        rx.await??;
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
    account_addresses: Arc<Vec<Arc<Address>>>,
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
            account_addresses.clone(),
            contract_config.get_router_address().unwrap(),
            tps,
        );
        let _rx = run_plan(plan, producer).await?;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(())
}

async fn test_erc20_transfer(
    account_addresses: Arc<Vec<Arc<Address>>>,
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
            account_addresses.clone(),
            tps,
        );
        let _rx = run_plan(erc20_transfer, producer).await?;
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

#[actix::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let benchmark_config = BenchConfig::load(&args.config).unwrap();
    assert!(benchmark_config.accounts.num_accounts >= benchmark_config.target_tps as usize);
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_file(false)
        .with_line_number(false)
        .with_thread_ids(false)
        .init();

    let mut generator = AccountGenerator::new();

    info!("Starting contract deployment and account generation...");
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

    let contract_config =
        ContractConfig::load_from_file(&benchmark_config.contract_config_path).unwrap_or_else(|e| {
            panic!("Contract config file not found {}", e);
        });

    let accounts = generator
        .gen(benchmark_config.accounts.num_accounts)
        .unwrap();

    let account_addresses = Arc::new(
        accounts
            .keys()
            .map(|account| account.clone())
            .collect::<Vec<_>>(),
    );
    // Create EthHttpCli instance
    let eth_clients: Vec<Arc<EthHttpCli>> = benchmark_config
        .nodes
        .iter()
        .map(|node| {
            let client = EthHttpCli::new(&node.rpc_url, node.chain_id).unwrap();
            Arc::new(client)
        })
        .collect();

    let monitor = Monitor::new_with_clients(
        eth_clients.clone(),
        benchmark_config.performance.max_pool_size,
    )
    .start();

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

    info!("Initializing nonces for {} accounts...", accounts.len());
    let nonce_map = init_nonce(&accounts, eth_clients[0].clone()).await;
    let producer = Producer::new(accounts.clone(), nonce_map, consumer, monitor)
        .unwrap()
        .start();
    let chain_id = benchmark_config.nodes[0].chain_id;

    // Faucet distribution: always run this, the builder will handle recovery internally
    let faucet_signer =
        PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap();
    let faucet_balance = eth_clients[0]
        .get_balance(&faucet_signer.address())
        .await
        .unwrap();

    info!("Initializing ETH Faucet distribution...");
    let eth_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
        benchmark_config.faucet.faucet_level as usize,
        faucet_balance,
        &benchmark_config.faucet.private_key,
        account_addresses.clone(),
        Arc::new(EthFaucetTxnBuilder),
        U256::from(benchmark_config.num_tokens)
            * U256::from(21000)
            * U256::from(1000_000_000_000u64),
        eth_clients[0].clone(),
        &mut generator,
    )
    .await?;
    execute_faucet_distribution(
        eth_faucet_builder,
        chain_id,
        &producer,
        "ETH",
        benchmark_config.faucet.wait_duration_secs,
    )
    .await?;

    let all_token_addresses = contract_config.get_all_token_addresses();

    for token in &all_token_addresses {
        info!("distributing token: {}", token);

        let balance = IERC20::new(*token, eth_clients[0].provider())
            .balanceOf(faucet_signer.address())
            .call()
            .await
            .unwrap();

        info!("balance of token: {}", balance);

        let token_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
            benchmark_config.faucet.faucet_level as usize,
            balance,
            &benchmark_config.faucet.private_key,
            account_addresses.clone(),
            Arc::new(Erc20FaucetTxnBuilder::new(*token)),
            U256::ZERO,
            eth_clients[0].clone(),
            &mut generator,
        )
        .await?;

        execute_faucet_distribution(
            token_faucet_builder,
            chain_id,
            &producer,
            &format!("Token {}", token),
            benchmark_config.faucet.wait_duration_secs,
        )
        .await?;
    }

    let tps = benchmark_config.target_tps as usize;
    let duration_secs = benchmark_config.performance.duration_secs;
    if benchmark_config.enable_swap_token {
        info!("bench uniswap");
        test_uniswap(
            account_addresses,
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
            account_addresses,
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

async fn init_nonce(
    accounts: &HashMap<Arc<Address>, Arc<PrivateKeySigner>>,
    eth_client: Arc<EthHttpCli>,
) -> HashMap<Arc<Address>, u32> {
    let nonce_futs = accounts.keys().map(|account| {
        let client = eth_client.clone();
        async move {
            let nonce = client
                .get_transaction_count(*account.clone())
                .await
                .unwrap();
            (account.clone(), nonce as u32)
        }
    });

    let results = join_all(nonce_futs).await;
    results.into_iter().collect()
}
