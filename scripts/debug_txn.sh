#!/bin/bash

# Script Name: eth_tx_analyzer.sh
# Description: A shell script to analyze an Ethereum transaction by fetching its receipt, details, traces, and balance changes.
# Usage: ./eth_tx_analyzer.sh <TX_HASH> <RPC_URL>
# Example: ./eth_tx_analyzer.sh 0x... https://mainnet.infura.io/v3/your-key
# Dependencies: curl, jq, bc

# --- Pre-flight Checks ---

# 1. Check for required commands
for cmd in curl jq bc; do
    if ! command -v $cmd &> /dev/null; then
        echo "Error: Required command '$cmd' is not installed."
        echo "Please install it and try again."
        exit 1
    fi
done

# 2. Check for correct number of arguments
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 <TX_HASH> <RPC_URL>"
    echo "Example: $0 0x1234... https://eth-mainnet.g.alchemy.com/v2/your-key"
    exit 1
fi


# --- Configuration ---
# Correctly assign arguments to variables
TX_HASH=$1
RPC_URL=$2


# --- Main Script ---
echo "========================================"
echo "      Ethereum Transaction Analyzer"
echo "========================================"
echo "Transaction Hash: $TX_HASH"
echo "RPC URL: $RPC_URL"
echo "========================================"


# --- Helper Functions ---

# Function: send_rpc_request
# Sends a JSON-RPC request to the specified RPC URL.
# $1: JSON-RPC method (e.g., "eth_getTransactionReceipt")
# $2: JSON-RPC parameters (e.g., "[\"$TX_HASH\"]")
# $3: Request ID (optional, defaults to 1)
send_rpc_request() {
    local method=$1
    local params=$2
    local id=${3:-1}

    curl -s -X POST \
        -H "Content-Type: application/json" \
        --data "{\"jsonrpc\":\"2.0\",\"method\":\"$method\",\"params\":$params,\"id\":$id}" \
        "$RPC_URL"
}

# Function: wei_to_eth
# Converts a Wei value (in hex format) to an ETH value (decimal).
# $1: Wei value as a hex string (e.g., "0x...")
wei_to_eth() {
    local wei_hex=$(echo "$1" | sed 's/0x//' | tr 'a-f' 'A-F')
    if [ -z "$wei_hex" ]; then
        echo "0.0"
        return
    fi
    # bc requires uppercase hex letters for ibase=16
    echo "scale=18; ibase=16; $wei_hex / 1000000000000000000" | bc -l 2>/dev/null || echo "0.0"
}


# 1. Get Transaction Receipt
echo -e "\n📄 Transaction Receipt (eth_getTransactionReceipt)"
echo "----------------------------------------"
RECEIPT=$(send_rpc_request "eth_getTransactionReceipt" "[\"$TX_HASH\"]")
echo "$RECEIPT" | jq '.'

# Extract key information from the receipt
FROM_ADDRESS=$(echo "$RECEIPT" | jq -r '.result.from // empty')
TO_ADDRESS=$(echo "$RECEIPT" | jq -r '.result.to // empty')
BLOCK_NUMBER=$(echo "$RECEIPT" | jq -r '.result.blockNumber // empty')
STATUS=$(echo "$RECEIPT" | jq -r '.result.status // empty')

echo -e "\nExtracted Information:"
echo "From: $FROM_ADDRESS"
echo "To: $TO_ADDRESS"
echo "Block: $BLOCK_NUMBER"
if [ "$STATUS" == "0x1" ]; then
    echo "Status: Success"
elif [ "$STATUS" == "0x0" ]; then
    echo "Status: Failed"
else
    echo "Status: $STATUS (Not found or pending)"
fi


# 2. Get Transaction Details
echo -e "\n📋 Transaction Details (eth_getTransactionByHash)"
echo "----------------------------------------"
TX_DETAILS=$(send_rpc_request "eth_getTransactionByHash" "[\"$TX_HASH\"]")
echo "$TX_DETAILS" | jq '.'


# 3. Get Call Trace
echo -e "\n🔍 Call Trace (debug_traceTransaction with callTracer)"
echo "----------------------------------------"
# Note: Not all nodes support debug_traceTransaction.
TRACE=$(send_rpc_request "debug_traceTransaction" "[\"$TX_HASH\", {\"tracer\": \"callTracer\"}]")

if echo "$TRACE" | jq -e '.error' > /dev/null; then
    echo "⚠️  Node does not support 'debug_traceTransaction' with 'callTracer' or an error occurred:"
    echo "$TRACE" | jq '.error'

    # Fallback attempt for Parity/OpenEthereum clients
    echo -e "\nTrying 'trace_transaction' as a fallback..."
    TRACE=$(send_rpc_request "trace_transaction" "[\"$TX_HASH\"]")
    if echo "$TRACE" | jq -e '.error' > /dev/null; then
        echo "⚠️  'trace_transaction' is also not supported or failed:"
        echo "$TRACE" | jq '.error'
    else
        echo "$TRACE" | jq '.'
    fi
else
    echo "$TRACE" | jq '.'
fi


# 4. Get Debug Trace
echo -e "\n🐛 Debug Trace (debug_traceTransaction with prestateTracer)"
echo "----------------------------------------"
DEBUG_TRACE=$(send_rpc_request "debug_traceTransaction" "[\"$TX_HASH\", {\"tracer\": \"prestateTracer\"}]")

if echo "$DEBUG_TRACE" | jq -e '.error' > /dev/null; then
    echo "⚠️  'prestateTracer' is not supported or an error occurred:"
    echo "$DEBUG_TRACE" | jq '.error'

    # Fallback to a basic struct logger trace
    echo -e "\nTrying basic debug trace (structLogs)..."
    DEBUG_TRACE=$(send_rpc_request "debug_traceTransaction" "[\"$TX_HASH\"]")
    if echo "$DEBUG_TRACE" | jq -e '.error' > /dev/null; then
        echo "⚠️  Basic debug trace is also not supported or failed:"
        echo "$DEBUG_TRACE" | jq '.error'
    else
        # Display only the first 100 lines to avoid excessively long output
        LOG_COUNT=$(echo "$DEBUG_TRACE" | jq '.result.structLogs | length')
        echo "$DEBUG_TRACE" | jq '.result.structLogs[:100]'
        if [ "$LOG_COUNT" -gt 100 ]; then
          echo "..."
          echo "(Showing first 100 of $LOG_COUNT total logs. The full log may be very long)"
        fi
    fi
else
    echo "$DEBUG_TRACE" | jq '.'
fi


# 5. Get Address Balances (Before and After)
echo -e "\n💰 Address Balances (eth_getBalance)"
echo "----------------------------------------"

# Ensure we have the necessary info to proceed
if [ -z "$BLOCK_NUMBER" ] || [ "$BLOCK_NUMBER" == "null" ]; then
    echo "⚠️  Cannot fetch balances: Block number not found in receipt (transaction may be pending)."
else
    # Calculate the previous block number. Bash's ((...)) handles hex (0x...) automatically.
    PREV_BLOCK_DEC=$(($BLOCK_NUMBER - 1))
    PREV_BLOCK_HEX=$(printf "0x%x" $PREV_BLOCK_DEC)

    # --- From Address Balance ---
    if [ -n "$FROM_ADDRESS" ] && [ "$FROM_ADDRESS" != "null" ]; then
        echo "From Address Balance ($FROM_ADDRESS):"

        # Balance Before (at the previous block)
        FROM_BALANCE_BEFORE_RESP=$(send_rpc_request "eth_getBalance" "[\"$FROM_ADDRESS\", \"$PREV_BLOCK_HEX\"]")
        BALANCE_BEFORE_WEI=$(echo "$FROM_BALANCE_BEFORE_RESP" | jq -r '.result // "0x0"')
        BALANCE_BEFORE_ETH=$(wei_to_eth "$BALANCE_BEFORE_WEI")
        echo "  - Before Tx (Block $PREV_BLOCK_DEC): $BALANCE_BEFORE_ETH ETH ($BALANCE_BEFORE_WEI)"

        # Balance After (at the transaction's block)
        FROM_BALANCE_AFTER_RESP=$(send_rpc_request "eth_getBalance" "[\"$FROM_ADDRESS\", \"$BLOCK_NUMBER\"]")
        BALANCE_AFTER_WEI=$(echo "$FROM_BALANCE_AFTER_RESP" | jq -r '.result // "0x0"')
        BALANCE_AFTER_ETH=$(wei_to_eth "$BALANCE_AFTER_WEI")
        echo "  - After Tx  (Block $(($BLOCK_NUMBER))): $BALANCE_AFTER_ETH ETH ($BALANCE_AFTER_WEI)"
    fi

    # --- To Address Balance ---
    # Check if To Address exists and is different from From Address
    if [ -n "$TO_ADDRESS" ] && [ "$TO_ADDRESS" != "null" ] && [ "$TO_ADDRESS" != "$FROM_ADDRESS" ]; then
        echo -e "\nTo Address Balance ($TO_ADDRESS):"

        # Balance Before
        TO_BALANCE_BEFORE_RESP=$(send_rpc_request "eth_getBalance" "[\"$TO_ADDRESS\", \"$PREV_BLOCK_HEX\"]")
        BALANCE_BEFORE_WEI=$(echo "$TO_BALANCE_BEFORE_RESP" | jq -r '.result // "0x0"')
        BALANCE_BEFORE_ETH=$(wei_to_eth "$BALANCE_BEFORE_WEI")
        echo "  - Before Tx (Block $PREV_BLOCK_DEC): $BALANCE_BEFORE_ETH ETH ($BALANCE_BEFORE_WEI)"

        # Balance After
        TO_BALANCE_AFTER_RESP=$(send_rpc_request "eth_getBalance" "[\"$TO_ADDRESS\", \"$BLOCK_NUMBER\"]")
        BALANCE_AFTER_WEI=$(echo "$TO_BALANCE_AFTER_RESP" | jq -r '.result // "0x0"')
        BALANCE_AFTER_ETH=$(wei_to_eth "$BALANCE_AFTER_WEI")
        echo "  - After Tx  (Block $(($BLOCK_NUMBER))): $BALANCE_AFTER_ETH ETH ($BALANCE_AFTER_WEI)"
    fi
fi


echo -e "\n========================================"
echo "          Analysis Complete!"
echo "========================================"