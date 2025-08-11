use actix::{Actor, Addr};
use alloy::{
    primitives::{Address, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::Result;
use std::{
    process::{Command, Output},
    str::FromStr,
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
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
    util::gen_account::gen_account,
};

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
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
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
    loop {
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
}

async fn test_erc20_transfer(
    account_addresses: Arc<Vec<Arc<Address>>>,
    chain_id: u64,
    contract_config: ContractConfig,
    producer: &Addr<Producer>,
    tps: usize,
) -> Result<()> {
    loop {
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
    let benchmark_config = BenchConfig::load("bench_config.toml").unwrap();
    assert!(benchmark_config.accounts.num_accounts >= benchmark_config.target_tps as usize);
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_file(true)
        .with_line_number(true)
        .with_thread_ids(true)
        .init();
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
        ContractConfig::load_from_file(&benchmark_config.contract_config_path).unwrap();
    let accounts = gen_account(benchmark_config.accounts.num_accounts).unwrap();
    let accounts_clone = accounts.clone();
    tokio::spawn(async move {
        let mut file = tokio::fs::File::create("accounts.txt").await.unwrap();
        for account in accounts_clone.iter() {
            file.write(
                format!(
                    "{}, {}\n",
                    account.0.to_string(),
                    hex::encode(account.1.credential().to_bytes().as_slice())
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        }
    });
    let account_addresses = Arc::new(
        accounts
            .iter()
            .map(|account| account.0.clone())
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
    let faucet_address = PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap();
    let faucet_start_nonce = eth_clients[0]
        .get_transaction_count(faucet_address.address())
        .await
        .unwrap();
    let faucet_balance = eth_clients[0]
        .get_balance(&faucet_address.address())
        .await
        .unwrap();
    // 1e20 is the gas fee
    let monitor = Monitor::new_with_clients(
        eth_clients.clone(),
        benchmark_config.performance.max_pool_size,
    )
    .start();
    let eth_providers: Vec<EthHttpCli> = eth_clients
        .iter()
        .map(|client| EthHttpCli::new(client.rpc().as_ref(), client.chain_id()).unwrap())
        .collect();

    let consumer = Consumer::new_with_providers(
        eth_providers,
        benchmark_config.performance.num_senders,
        monitor.clone(),
        benchmark_config.performance.max_pool_size,
        Some(benchmark_config.target_tps as u32),
    )
    .start();
    let producer = Producer::new(accounts, consumer, monitor).unwrap().start();
    let chain_id = benchmark_config.nodes[0].chain_id;

    info!("Initializing Faucet constructor...");
    let eth_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
        benchmark_config.faucet.faucet_level as usize,
        faucet_balance,
        &benchmark_config.faucet.private_key,
        faucet_start_nonce,
        account_addresses.clone(),
        Arc::new(EthFaucetTxnBuilder),
        U256::from(benchmark_config.num_tokens)
            * U256::from(21000)
            * U256::from(1000_000_000_000u64),
    )
    .unwrap();
    execute_faucet_distribution(eth_faucet_builder, chain_id, &producer, "ETH").await?;

    let all_token_addresses = contract_config.get_all_token_addresses();
    let faucet_signer_for_token =
        PrivateKeySigner::from_str(&benchmark_config.faucet.private_key).unwrap();

    for token in &all_token_addresses {
        info!("distributing token: {}", token);

        let faucet_current_nonce = eth_clients[0]
            .get_transaction_count(faucet_signer_for_token.address())
            .await
            .unwrap();
        let balance = IERC20::new(*token, eth_clients[0].provider())
            .balanceOf(faucet_signer_for_token.address())
            .call()
            .await
            .unwrap();

        info!("balance of token: {}", balance);

        let token_faucet_builder = PlanBuilder::create_faucet_tree_plan_builder(
            benchmark_config.faucet.faucet_level as usize,
            balance,
            &benchmark_config.faucet.private_key,
            faucet_current_nonce,
            account_addresses.clone(),
            Arc::new(Erc20FaucetTxnBuilder::new(*token)),
            U256::ZERO,
        )
        .unwrap();

        execute_faucet_distribution(
            token_faucet_builder,
            chain_id,
            &producer,
            &format!("Token {}", token),
        )
        .await?;
    }
    let tps = benchmark_config.target_tps as usize;
    if benchmark_config.enable_swap_token {
        info!("bench uniswap");
        test_uniswap(account_addresses, chain_id, contract_config, &producer, tps).await?;
    } else {
        info!("bench erc20 transfer");
        test_erc20_transfer(account_addresses, chain_id, contract_config, &producer, tps).await?;
    }
    Ok(())
}
