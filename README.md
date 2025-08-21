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

*   **An Ethereum node with RPC endpoint enabled**: The tool needs an RPC endpoint to connect to.

**Optional (will be auto-installed by setup script if missing):**
*   **Rust**: [Install Rust](https://www.rust-lang.org/tools/install)
*   **Node.js and npm**: [Install Node.js](https://nodejs.org/en/download/)
*   **Python 3 and pip**: Required for setup scripts and contract deployment tools.

## Setup

1.  **Clone the repository:**
    ```bash
    git clone <repository-url>
    cd gravity_bench
    ```

2.  **Run the setup script:**
    The setup script will automatically handle all dependencies and environment setup, including:
    - Checking and installing missing tools (Node.js, Python, Rust)
    - Creating a Python virtual environment
    - Installing Python and Node.js dependencies
    - Cloning required contract repositories
    
    **Recommended way (keeps environments active):**
    ```bash
    source setup.sh
    ```
    
    **Alternative way (requires manual environment activation):**
    ```bash
    bash setup.sh
    # Then manually activate environments:
    source venv/bin/activate
    source ~/.cargo/env  # If Rust was installed
    ```

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

Edit `bench_config.toml` to set up your benchmark run. This is the default configuration file, but you can specify a different one using the `--config` command-line argument when running the benchmark.

```toml
# Gravity Bench Configuration File

# Uniswap configuration file path
contract_config_path = "deploy.json"
target_tps = 10000
nodes = [
    { rpc_url = "http://localhost:8545", chain_id = 7771625 },
]
num_tokens=2
enable_swap_token = false
# Faucet and deployer account configuration
[faucet]
# Private key (example, please replace with real private key)
private_key = "xxxxx"
# Faucet Level: Controls the depth of cascading faucet levels
# Value 10: Enables cascade mode with progression 1 -> 10 -> 100 (amplification chain)
# Value 0: Disables cascading, processes data directly
faucet_level = 10 

# Load testing account configuration
[accounts]
# Number of load testing accounts to generate
num_accounts = 10000

# Performance and stress configuration
[performance]
# Number of concurrent transaction sending tasks inside TxnConsumer
num_senders = 1000
# Maximum capacity of the transaction pool inside Consumer
max_pool_size = 100000
```

**Key configuration options:**

*   `faucet.private_key`: **IMPORTANT** - This account must have a sufficient balance of the native currency (e.g., ETH) to fund all the generated test accounts.
*   `nodes.rpc_url`: The RPC endpoint of the Ethereum node.
*   `target_tps`: The desired number of transactions per second.
*   `enable_swap_token`: Set to `true` to benchmark Uniswap V2 swaps, or `false` for simple ERC20 transfers.

## Running the Benchmark

Once the configuration is set up, you can run the benchmark using `cargo run`.

### Command-Line Arguments

When using `cargo run`, arguments for `gravity_bench` itself must be passed after a `--` separator. Arguments before the separator are for Cargo (e.g., `--release` to build with optimizations).

**Syntax:**
`cargo run [<cargo_args>] -- [<application_args>]`

**Available Arguments:**

*   `--config <PATH>`: Specifies the path to the configuration file. Defaults to `bench_config.toml`.
    ```bash
    cargo run --release -- --config custom_config.toml
    ```
*   `--recover`: Enables recovery mode. This is useful for re-running a benchmark without repeating the initial setup. See the "Recovery Mode" section for details.
    ```bash
    cargo run --release -- --recover
    ```

You can combine arguments:
```bash
cargo run --release -- --config bench_config.toml --recover
```

### Normal Workflow

By default (without the `--recover` flag), the application will perform the following setup steps:
1.  Connect to the specified Ethereum node.
2.  Deploy the necessary ERC20 tokens and (if `enable_swap_token` is true) a Uniswap V2 router and liquidity pools.
3.  Save the addresses of the deployed contracts to the file specified in `contract_config_path` (e.g., `contract.json`).
4.  Generate the specified number of test accounts.
5.  Fund the test accounts with ETH and ERC20 tokens from the faucet account. This step can take some time.
6.  Save the generated accounts (including private keys) to `accounts.txt`.
7.  Start generating the transaction workload as defined in the configuration.

### Recovery Mode

The setup process, especially the faucet distribution, can be time-consuming and costly. If this process has already been completed once, you can use **recovery mode** to skip it and jump directly to generating the transaction workload.

To use recovery mode, run the benchmark with the `--recover` flag:
```bash
cargo run --release -- --recover
```

In recovery mode, the application will:
1.  Skip contract deployment, account generation, and faucet distribution.
2.  Load the existing contract configuration from the file specified in `contract_config_path`.
3.  Load the pre-funded accounts from `accounts.txt`.
4.  Start generating the transaction workload immediately.

This allows you to quickly restart the benchmark using the same set of contracts and funded accounts. 

## Docker Deployment

For a more isolated and reproducible environment, you can use Docker to run the benchmark.

### Prerequisites

*   [Docker](https://docs.docker.com/get-docker/)
*   [Docker Compose](https://docs.docker.com/compose/install/)

### Building the Docker Image

The included `Dockerfile` handles all the necessary setup, including installing dependencies, cloning contracts, and building the Rust application.

To build the image, run the following command from the project root:

```bash
docker compose build
```

Or, using Docker directly:

```bash
docker build -t gravity_bench .
```

### Running with Docker Compose

`docker-compose` is the recommended way to run the application, as it simplifies volume mounting and command execution.

**1. Configure `bench_config.toml`:**

Make sure your `bench_config.toml` is configured correctly. If you are connecting to a local Ethereum node on your host machine, you can use `http://localhost:8545` as the `rpc_url` because the Docker Compose file is configured to use the host network.

**2. Run the benchmark:**

The `docker-compose.yml` is configured to run the application with the default `bench_config.toml`.

```bash
docker compose run --rm gravity_bench
```

This will start the benchmark, and you will see the output in your terminal. The `--rm` flag automatically removes the container when it exits.

**Important Note about File Storage:**

The current Docker configuration uses `/tmp` for temporary file storage. This means:
- ✅ **Simplified permissions** - No volume mounting issues
- ✅ **Clean runs** - Each container run starts fresh
- ❌ **No recovery mode** - Generated files (`deploy.json`, `accounts.txt`) are not preserved between runs
- ❌ **Data not accessible** - Generated files cannot be inspected on the host machine

This configuration is ideal for:
- One-time benchmarking runs
- Performance testing without state persistence
- Simplified Docker deployment

If you need to preserve generated files or use recovery mode, you can modify the Docker configuration to mount persistent volumes.

**3. Running in Recovery Mode:**

⚠️ **Recovery mode is not available** with the current `/tmp` configuration, as the required files (`deploy.json` and `accounts.txt`) are not persisted between container runs.

If you need recovery mode, you must configure persistent volume mounts in `docker-compose.yml`.


### File Access and Persistence

**Current Configuration (`/tmp`):**
- Generated files (`deploy.json`, `accounts.txt`) are stored temporarily in the container
- Files are automatically cleaned up when the container exits
- No files are accessible on the host machine after the run
