#!/usr/bin/env python3
import os
import time
import json
import argparse
from web3 import Web3
from solcx import compile_files, install_solc, set_solc_version

# --- 0. Configuration & Helper Functions ---
# ==============================================================================
SOLC_VERSIONS = ["0.8.20", "0.6.6", "0.5.16"]
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

def filter_abi(full_abi, function_names):
    """Filters an ABI, keeping only specified functions and essential parts like the constructor."""
    return [
        item for item in full_abi
        if item.get('name') in function_names or item['type'] in ('constructor', 'fallback', 'receive')
    ]

# --- 1. Contract Compilation ---
# ==============================================================================
def install_solc_versions():
    """Installs all required solc versions."""
    print(f"Installing Solc versions: {', '.join(SOLC_VERSIONS)}")
    for v in SOLC_VERSIONS:
        try:
            install_solc(v)
            print(f"Solc v{v} installed successfully.")
        except Exception as e:
            print(f"Could not install/check solc v{v}. It might already be installed. Error: {e}")

def compile_all_contracts():
    """Compiles all necessary smart contracts at once."""
    install_solc_versions()
    print("-" * 60)
    
    project_root = os.path.abspath('.')
    
    # --- Compile Uniswap V2 Core ---
    set_solc_version("0.5.16")
    print("Compiling Uniswap V2 Factory...")
    factory_path = os.path.join(project_root, 'contracts/v2-core/contracts/UniswapV2Factory.sol')
    factory_compiled = compile_files(
        [factory_path], output_values=['abi', 'bin'], solc_version="0.5.16",
        optimize=True, optimize_runs=999999
    )
    
    # --- Compile Uniswap V2 Periphery ---
    set_solc_version("0.6.6")
    print("Compiling Uniswap V2 Router...")
    router_path = os.path.join(project_root, 'contracts/v2-periphery/contracts/UniswapV2Router02.sol')
    core_path_remap = os.path.join(project_root, 'contracts/v2-core')
    lib_path_remap = os.path.join(project_root, 'contracts/uniswap-lib')
    router_compiled = compile_files(
        [router_path], output_values=['abi', 'bin'], solc_version="0.6.6",
        optimize=True, optimize_runs=999999,
        import_remappings={
            "@uniswap/v2-core/": f"{core_path_remap}/",
            "@uniswap/lib/": f"{lib_path_remap}/"
        },
        allow_paths=[core_path_remap, lib_path_remap, os.path.dirname(router_path)]
    )

    # --- Compile Custom Tokens & WETH ---
    set_solc_version("0.8.20")
    print("Compiling MyToken and WETH9...")
    openzeppelin_path = os.path.join(project_root, "node_modules/@openzeppelin")
    custom_files = [
        os.path.join(project_root, 'contracts/MyToken.sol'),
        os.path.join(project_root, 'contracts/WETH9.sol')
    ]
    custom_compiled = compile_files(
        custom_files, output_values=['abi', 'bin'], solc_version="0.8.20",
        optimize=True, optimize_runs=999999,
        import_remappings={"@openzeppelin/": f"{openzeppelin_path}/"}
    )
    print("✅ All contracts compiled successfully!")
    print("-" * 60)

    return {
        'factory': factory_compiled[f'{factory_path}:UniswapV2Factory'],
        'router': router_compiled[f'{router_path}:UniswapV2Router02'],
        'token': custom_compiled['contracts/MyToken.sol:MyToken'],
        'weth': custom_compiled['contracts/WETH9.sol:WETH9']
    }

# --- 2. Deployment & Interaction ---
# ==============================================================================
def send_transaction(w3, tx, private_key, description):
    """Signs and sends a transaction, then waits for the receipt."""
    try:
        signed_tx = w3.eth.account.sign_transaction(tx, private_key=private_key)
        tx_hash = w3.eth.send_raw_transaction(signed_tx.raw_transaction)
        print(f"  > {description} transaction sent, TX Hash: {tx_hash.hex()}")
        tx_receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=300)
        if tx_receipt['status'] == 0:
            print(f"  ❌ {description} transaction FAILED.")
            return None
        print(f"  ✅ {description} transaction successful.")
        return tx_receipt
    except Exception as e:
        print(f"  ❌ Exception during '{description}' transaction: {e}")
        return None

def deploy_contract(w3, account, private_key, contract_interface, contract_name, *args):
    """A generic contract deployment function."""
    print(f"Deploying {contract_name}...")
    Contract = w3.eth.contract(abi=contract_interface['abi'], bytecode=contract_interface['bin'])
    
    nonce = w3.eth.get_transaction_count(account.address)
    gas_price = w3.eth.gas_price
    
    try:
        gas_limit = int(Contract.constructor(*args).estimate_gas({'from': account.address}) * 1.2)
    except Exception:
        gas_limit = 5_000_000  # Fallback Gas
        
    tx = Contract.constructor(*args).build_transaction({
        'from': account.address, 'nonce': nonce,
        'gas': gas_limit, 'gasPrice': gas_price
    })
    
    receipt = send_transaction(w3, tx, private_key, f"Deploy {contract_name}")
    if not receipt: return None, None
    
    address = receipt['contractAddress']
    print(f"✅ {contract_name} deployed to: {address}")
    return address, w3.eth.contract(address=address, abi=contract_interface['abi'])

def approve_token(w3, account, private_key, token_contract, spender_address, token_symbol):
    """Approves a token for a spender (e.g., the Router)."""
    print(f"Approving {token_symbol} for the Router...")
    amount = 2**256 - 1  # Max approval
    allowance = token_contract.functions.allowance(account.address, spender_address).call()
    if allowance >= amount:
        print(f"  ✅ {token_symbol} is already sufficiently approved.")
        return True
        
    tx = token_contract.functions.approve(spender_address, amount).build_transaction({
        'from': account.address,
        'nonce': w3.eth.get_transaction_count(account.address),
        'gas': 100000,
        'gasPrice': w3.eth.gas_price
    })
    
    return send_transaction(w3, tx, private_key, f"Approve {token_symbol}") is not None

def add_liquidity(w3, account, private_key, router_contract, token_a_info, token_b_info):
    """Adds liquidity for a pair of ERC20 tokens."""
    token_a_addr, token_a_sym = token_a_info['address'], token_a_info['symbol']
    token_b_addr, token_b_sym = token_b_info['address'], token_b_info['symbol']
    pair_name = f"{token_a_sym}/{token_b_sym}"

    print(f"\n--- Adding liquidity for {pair_name} ---")
    amount_desired = w3.to_wei(1_000_000, 'ether')
    deadline = int(time.time()) + 1200
    
    tx = router_contract.functions.addLiquidity(
        token_a_addr, token_b_addr, amount_desired, amount_desired,
        0, 0, account.address, deadline
    ).build_transaction({
        'from': account.address, 'nonce': w3.eth.get_transaction_count(account.address),
        'gas': 3_000_000, 'gasPrice': w3.eth.gas_price
    })
    
    receipt = send_transaction(w3, tx, private_key, f"Add Liquidity for {pair_name}")
    return receipt is not None

def add_liquidity_eth(w3, account, private_key, router_contract, token_info):
    """Adds liquidity for ETH and an ERC20 token."""
    token_addr, token_sym = token_info['address'], token_info['symbol']
    pair_name = f"{token_sym}/ETH"
    print(f"\n--- Adding liquidity for {pair_name} ---")
    
    token_amount = w3.to_wei(1_000_000, 'ether')
    eth_amount = w3.to_wei(1, 'ether') # e.g., 1 ETH
    deadline = int(time.time()) + 1200
    
    tx = router_contract.functions.addLiquidityETH(
        token_addr, token_amount, 0, 0, account.address, deadline
    ).build_transaction({
        'from': account.address, 'value': eth_amount,
        'nonce': w3.eth.get_transaction_count(account.address),
        'gas': 3_000_000, 'gasPrice': w3.eth.gas_price
    })
    
    receipt = send_transaction(w3, tx, private_key, f"Add Liquidity for {pair_name}")
    return receipt is not None

# --- 3. Main Execution Logic ---
# ==============================================================================
def main(args):
    """Main execution function."""
    # --- Initialization ---
    w3 = Web3(Web3.HTTPProvider(args.rpc_url))
    account = w3.eth.account.from_key(args.private_key)
    print(f"Connected to: {args.rpc_url}")
    print(f"Deployer address: {account.address}")
    print(f"ETH Balance: {w3.from_wei(w3.eth.get_balance(account.address), 'ether'):.4f} ETH")
    print("-" * 60)
    
    # --- Compile ---
    contracts = compile_all_contracts()

    # --- Deploy Core Contracts (Factory, WETH, Router) ---
    print("\n--- Deploying Uniswap V2 & WETH Core Contracts ---")
    factory_address, factory_contract = deploy_contract(w3, account, args.private_key, contracts['factory'], "UniswapV2Factory", account.address)
    weth_address, _ = deploy_contract(w3, account, args.private_key, contracts['weth'], "WETH9")
    router_address, router_contract = deploy_contract(w3, account, args.private_key, contracts['router'], "UniswapV2Router02", factory_address, weth_address)
    
    if not all([factory_address, weth_address, router_address]):
        raise Exception("Core contract deployment failed. Exiting.")

    # --- Deploy Custom Tokens ---
    print("\n--- Deploying Custom ERC20 Tokens ---")
    initial_supply = w3.to_wei(10**20, 'ether')
    deployed_tokens = []
    for i in range(args.num_tokens):
        token_name = f"MyToken{i+1}"
        token_symbol = f"TKN{i+1}"
        addr, contract = deploy_contract(w3, account, args.private_key, contracts['token'], token_name, token_name, token_symbol, initial_supply)
        if addr:
            deployed_tokens.append({'symbol': token_symbol, 'address': addr, 'contract': contract})
            
    if len(deployed_tokens) != args.num_tokens:
        raise Exception("Token deployment was not fully successful. Exiting.")

    # --- Add Liquidity ---
    print("\n--- Starting to Add Liquidity ---")
    liquidity_eth_pairs = []
    liquidity_token_pairs = []

    # 1. Create Token/ETH pools for all tokens
    for token_info in deployed_tokens:
        if approve_token(w3, account, args.private_key, token_info['contract'], router_address, token_info['symbol']):
            if add_liquidity_eth(w3, account, args.private_key, router_contract, token_info):
                 pair_addr = factory_contract.functions.getPair(token_info['address'], weth_address).call()
                 liquidity_eth_pairs.append({
                     "token_a_symbol": token_info['symbol'],
                     "token_a_address": token_info['address'],
                     "token_b_symbol": "ETH",
                     "token_b_address": weth_address,
                     "lp_pair_address": pair_addr
                })
    
    # 2. If enabled, create Token/Token pools for token pairs
    if args.enable_swap_token:
        print("\n--- 'enable_swap_token' is ON. Creating Token-Token liquidity pools. ---")
        if args.num_tokens % 2 != 0:
            print("⚠️ WARNING: Odd number of tokens. The last token will not be paired.")
        
        # Iterate through tokens in pairs
        for i in range(0, args.num_tokens // 2 * 2, 2):
            token_a = deployed_tokens[i]
            token_b = deployed_tokens[i+1]
            
            # Approve both tokens for the router
            approve_a = approve_token(w3, account, args.private_key, token_a['contract'], router_address, token_a['symbol'])
            approve_b = approve_token(w3, account, args.private_key, token_b['contract'], router_address, token_b['symbol'])
            
            if approve_a and approve_b:
                if add_liquidity(w3, account, args.private_key, router_contract, token_a, token_b):
                    pair_addr = factory_contract.functions.getPair(token_a['address'], token_b['address']).call()
                    liquidity_token_pairs.append({
                        "token_a_symbol": token_a['symbol'],
                        "token_a_address": token_a['address'],
                        "token_b_symbol": token_b['symbol'],
                        "token_b_address": token_b['address'],
                        "lp_pair_address": pair_addr
                    })
    else:
        print("\n--- 'enable_swap_token' is OFF. Skipping Token-Token liquidity pool creation. ---")


    # --- Generate Output File ---
    print("\n--- Generating Output File ---")
    final_token_list = [{k: v for k, v in token.items() if k != 'contract'} for token in deployed_tokens]
    final_results = {
        "addresses": {
            "deployer": account.address,
            "uniswap_v2_factory": factory_address,
            "uniswap_v2_router": router_address,
            "weth9": weth_address,
            "tokens": final_token_list,
            "liquidity_eth_pairs": liquidity_eth_pairs,
            "liquidity_pairs": liquidity_token_pairs
        },
        "minimal_abis": {
            "router": filter_abi(contracts['router']['abi'], ['addLiquidity', 'addLiquidityETH', 'swapExactTokensForTokens', 'swapExactETHForTokens', 'swapExactTokensForETH']),
            "erc20": filter_abi(contracts['token']['abi'], ['approve', 'balanceOf', 'allowance', 'transfer']),
        }
    }

    with open(args.output_file, 'w') as f:
        json.dump(final_results, f, indent=2)

    print("\n" + "=" * 60)
    print("🎉 Deployment completed! 🎉")
    print(f"📁 Results saved to: {args.output_file}")
    total_pairs = len(liquidity_eth_pairs) + len(liquidity_token_pairs)
    print(f"✅ Successfully created {total_pairs} liquidity pools ({len(liquidity_eth_pairs)} with ETH, {len(liquidity_token_pairs)} token-to-token).")
    print("=" * 60)


# --- 4. Command-Line Argument Parsing ---
# ==============================================================================
if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Deploys ERC20 tokens and creates Uniswap V2 liquidity pools.')
    parser.add_argument('--private-key', required=True, help='Private key of the deployer account.')
    parser.add_argument('--num-tokens', type=int, required=True, help='The total number of ERC20 tokens to deploy.')
    parser.add_argument('--rpc-url', default='http://localhost:8545', help='RPC URL of the Ethereum node.')
    parser.add_argument('--output-file', default='deploy.json', help='File path to save the JSON output.')
    parser.add_argument('--enable-swap-token', action='store_true', help='If set, token-to-token liquidity pools will be created.')

    args = parser.parse_args()
    main(args)