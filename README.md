# Gravity Bench

`gravity_bench` is a high-performance transaction generator for Ethereum-compatible blockchains. It's designed for benchmarking and stress-testing EVM-based networks by generating a high volume of transactions. The tool uses an actor-based model for concurrency and can be configured to simulate various transaction workloads, such as simple ERC20 transfers or more complex decentralized exchange (DEX) swaps.

## Architecture

The application is built in Rust and leverages the `actix` actor framework for concurrent operations. The core architecture consists of three main actors:

*   **`Producer`**: Manages a pool of pre-funded accounts and generates signed transactions based on a "Transaction Plan".
*   **`Consumer`**: Receives signed transactions from the `Producer` and submits them to the target Ethereum nodes. It can use multiple senders to increase the submission rate.
*   **`Monitor`**: Tracks the status of submitted transactions, monitoring their inclusion in the mempool and final confirmation.

### Transaction Plans

The benchmark workloads are defined using `TxnPlan`s. These are modular definitions of transaction sequences. The tool includes builders for common scenarios:
*   Distributing ETH and ERC20 tokens to test accounts.
*   Approving tokens for spending by a smart contract (e.g., a DEX router).
*   Executing ERC20 token transfers.
*   Swapping tokens on a Uniswap V2-style DEX.

This design allows for creating flexible and complex benchmarking scenarios.

## Prerequisites

*   **Rust**: [Install Rust](https://www.rust-lang.org/tools/install)
*   **Node.js and npm**: [Install Node.js](https://nodejs.org/en/download/)
*   **An Ethereum node with RPC endpoint enabled**: The tool needs an RPC endpoint to connect to.

## Setup

1.  **Clone the repository:**
    ```bash
    git clone <repository-url>
    cd gravity_bench
    ```

2.  **Install contract dependencies:**
    The project relies on external smart contracts (from Uniswap and OpenZeppelin). A setup script is provided to download them.
    ```bash
    bash setup.sh
    ```
    This will clone the required repositories into the `contracts` directory and install npm packages.

    ```bash
    python scripts/refresh_init_code.py
    ```
    and replace the init code in contracts/v2-periphery/contracts/libraries/UniswapV2Library.sol
3.  **Create the configuration file:**
    Copy the template to create your own configuration file.
    ```bash
    cp bench_config.template bench_config.toml
    ```

## Configuration

Edit `bench_config.toml` to set up your benchmark run.

```toml
# Number of accounts to generate for the test
num_accounts = 100
# Target transactions per second
target_tps = 50
# Path for the generated contract configuration
contract_config_path = "contract.json"
# Number of ERC20 tokens to deploy
num_tokens = 5
# Enable swapping tokens on a DEX (if false, it will only do ERC20 transfers)
enable_swap_token = false

# Faucet account used to distribute funds to test accounts
[faucet]
private_key = "YOUR_FAUCET_PRIVATE_KEY"

# Configuration for the generated accounts
[accounts]
num_accounts = 100

# Performance-related settings
[performance]
# Number of concurrent transaction senders
num_senders = 10
# Maximum number of transactions to keep in the mempool tracker
max_pool_size = 10000

# List of Ethereum nodes to connect to
[[nodes]]
rpc_url = "http://localhost:8545"
chain_id = 31337
```

**Key configuration options:**

*   `faucet.private_key`: **IMPORTANT** - This account must have a sufficient balance of the native currency (e.g., ETH) to fund all the generated test accounts.
*   `nodes.rpc_url`: The RPC endpoint of the Ethereum node.
*   `target_tps`: The desired number of transactions per second.
*   `enable_swap_token`: Set to `true` to benchmark Uniswap V2 swaps, or `false` for simple ERC20 transfers.

## Running the Benchmark

Once the configuration is set up, you can run the benchmark using Cargo.

```bash
cargo run --release
```

The application will perform the following steps:
1.  Connect to the specified Ethereum node.
2.  Deploy the necessary ERC20 tokens and (if `enable_swap_token` is true) a Uniswap V2 router and liquidity pools.
3.  Save the addresses of the deployed contracts to `contract.json` (or the path specified in `contract_config_path`).
4.  Generate the specified number of test accounts.
5.  Fund the test accounts with ETH and ERC20 tokens from the faucet account.
6.  Start generating the transaction workload as defined in the configuration. 