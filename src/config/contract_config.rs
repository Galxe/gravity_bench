use alloy::primitives::Address;
use alloy::sol;
use serde::Deserialize;
use std::str::FromStr;

#[derive(Debug, Deserialize, Clone)]
pub struct ContractConfig {
    pub addresses: Addresses,
    #[serde(rename = "minimal_abis")]
    #[allow(unused)]
    pub minimal_abis: MinimalAbis,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Addresses {
    #[allow(unused)]
    pub deployer: String,
    #[serde(rename = "uniswap_v2_factory")]
    #[allow(unused)]
    pub uniswap_v2_factory: Option<String>,
    #[serde(rename = "uniswap_v2_router")]
    pub uniswap_v2_router: Option<String>,
    #[allow(unused)]
    pub weth9: Option<String>,
    pub tokens: Vec<Token>,
    #[serde(rename = "liquidity_eth_pairs")]
    #[allow(unused)]
    pub liquidity_eth_pairs: Vec<LiquidityPair>,
    #[serde(rename = "liquidity_pairs")]
    #[allow(unused)]
    pub liquidity_pairs: Vec<LiquidityPair>, // Using Value as it's an empty array in the example
}

#[derive(Debug, Deserialize, Clone)]
pub struct Token {
    #[allow(unused)]
    pub symbol: String,
    pub address: String,
    #[serde(rename = "faucet_address")]
    pub faucet_address: String,
    #[serde(rename = "faucet_balance")]
    pub faucet_balance: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LiquidityPair {
    #[serde(rename = "token_a_symbol")]
    #[allow(unused)]
    pub token_a_symbol: String,
    #[serde(rename = "token_a_address")]
    #[allow(unused)]
    pub token_a_address: String,
    #[serde(rename = "token_b_symbol")]
    #[allow(unused)]
    pub token_b_symbol: String,
    #[serde(rename = "token_b_address")]
    #[allow(unused)]
    pub token_b_address: String,
    #[serde(rename = "lp_pair_address")]
    #[allow(unused)]
    pub lp_pair_address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MinimalAbis {
    #[allow(unused)]
    router: Vec<serde_json::Value>,
    #[allow(unused)]
    erc20: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct AbiInput {
    #[serde(rename = "internalType")]
    #[allow(unused)]
    internal_type: String,
    #[allow(unused)]
    name: String,
    #[serde(rename = "type")]
    #[allow(unused)]
    param_type: String,
}

impl ContractConfig {
    /// Load configuration from file
    pub fn load_from_file(path: &str) -> anyhow::Result<Self> {
        let file = std::fs::read_to_string(path)?;
        let config: Self = serde_json::from_str(&file)?;
        Ok(config)
    }

    /// Get router address as Address type
    pub fn get_router_address(&self) -> anyhow::Result<Address> {
        Address::from_str(
            &self
                .addresses
                .uniswap_v2_router
                .as_ref()
                .unwrap_or_else(|| panic!("Uniswap V2 router address not found")),
        )
        .map_err(|e| anyhow::anyhow!("Invalid router address: {}", e))
    }

    /// Get WETH address as Address type
    #[allow(unused)]
    pub fn get_weth_address(&self) -> anyhow::Result<Address> {
        Address::from_str(
            &self
                .addresses
                .weth9
                .as_ref()
                .unwrap_or_else(|| panic!("WETH address not found")),
        )
        .map_err(|e| anyhow::anyhow!("Invalid WETH address: {}", e))
    }

    /// Get all token addresses
    pub fn get_all_token(&self) -> Vec<Token> {
        self.addresses
            .tokens
            .iter()
            .map(|token| token.clone())
            .collect()
    }

    pub fn get_all_token_addresses(&self) -> Vec<Address> {
        self.addresses
            .tokens
            .iter()
            .filter_map(|token| Address::from_str(&token.address).ok())
            .collect()
    }

    pub fn get_liquidity_pairs(&self) -> &Vec<LiquidityPair> {
        &self.addresses.liquidity_pairs
    }
}

sol! {
    // contract IERC20 { ... }
    #[sol(rpc)]
    interface IERC20 {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
        function transfer(address to, uint256 amount) external returns (bool);
        function transferFrom(address from, address to, uint256 amount) external returns (bool);
    }

    interface IUniswapV2Router {
        function swapExactETHForTokens(
            uint amountOutMin,
            address[] calldata path,
            address to,
            uint deadline
        ) external payable returns (uint[] memory amounts);

        function swapExactTokensForTokens(
            uint amountIn,
            uint amountOutMin,
            address[] calldata path,
            address to,
            uint deadline
        ) external returns (uint[] memory amounts);

        function swapExactTokensForETH(
            uint amountIn,
            uint amountOutMin,
            address[] calldata path,
            address to,
            uint deadline
        ) external returns (uint[] memory amounts);

        function addLiquidityETH(
            address token,
            uint amountTokenDesired,
            uint amountTokenMin,
            uint amountETHMin,
            address to,
            uint deadline
        ) external payable returns (uint amountToken, uint amountETH, uint liquidity);
    }
}
