#!/bin/bash

# Function to print colored logs
log() {
    local type="$1"
    local message="$2"
    case "$type" in
        "info") echo -e "\033[0;34m[INFO]\033[0m $message" ;;
        "success") echo -e "\033[0;32m[SUCCESS]\033[0m $message" ;;
        "warn") echo -e "\033[0;33m[WARN]\033[0m $message" ;;
        "error") echo -e "\033[0;31m[ERROR]\033[0m $message" >&2 ;;
        *) echo "$message" ;;
    esac
}

# Function to check if a command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# --- 1. Environment Sanity Checks ---
log "info" "Starting environment sanity checks..."
FAILED_CHECKS=0

# Check for Node.js and npm
if ! command_exists node || ! command_exists npm; then
    log "error" "Node.js or npm is not installed. Please install them to continue."
    FAILED_CHECKS=$((FAILED_CHECKS + 1))
else
    log "success" "Node.js and npm are installed."
    log "info" "Node version: $(node -v)"
    log "info" "npm version: $(npm -v)"
fi

# Check for Python 3 and pip3
if ! command_exists python3 || ! command_exists pip3; then
    log "error" "Python 3 or pip3 is not installed. Please install them to continue."
    FAILED_CHECKS=$((FAILED_CHECKS + 1))
else
    log "success" "Python 3 and pip3 are installed."
    log "info" "Python version: $(python3 --version)"
fi

# Exit if any check failed
if [ "$FAILED_CHECKS" -ne 0 ]; then
    log "error" "Environment checks failed. Please install the missing dependencies and run the script again."
    exit 1
fi

log "success" "Environment checks passed!"
echo "-----------------------------------------------------"

# --- 2. Clone Git Repositories ---
log "info" "Cloning required Uniswap repositories..."
[ ! -d "contracts/uniswap-lib" ] && git clone https://github.com/Uniswap/solidity-lib.git contracts/uniswap-lib
[ ! -d "contracts/v2-periphery" ] && git clone https://github.com/Uniswap/v2-periphery.git contracts/v2-periphery
[ ! -d "contracts/v2-core" ] && git clone https://github.com/Uniswap/v2-core.git contracts/v2-core
log "success" "All repositories cloned or already exist."
echo "-----------------------------------------------------"


# --- 3. Install Dependencies ---
log "info" "Installing Node.js and Python dependencies..."

# Install Node.js dependencies
log "info" "Running 'npm install' for OpenZeppelin contracts..."
npm install @openzeppelin/contracts
if [ $? -eq 0 ]; then
    log "success" "npm dependencies installed successfully."
else
    log "error" "npm install failed. Please check the errors above."
    exit 1
fi

# Create Python virtual environment
log "info" "Creating Python virtual environment..."
if [ ! -d "venv" ]; then
    python3 -m venv venv
    if [ $? -eq 0 ]; then
        log "success" "Python virtual environment created successfully."
    else
        log "error" "Failed to create Python virtual environment."
        exit 1
    fi
else
    log "info" "Python virtual environment already exists."
fi

# Activate virtual environment and install Python dependencies
log "info" "Installing Python dependencies in virtual environment..."
source venv/bin/activate
pip install -r requirements.txt
if [ $? -eq 0 ]; then
    log "success" "Python dependencies installed successfully in virtual environment."
else
    log "error" "pip install failed. Please check the errors above."
    deactivate
    exit 1
fi
echo "-----------------------------------------------------"

log "success" "🎉 Setup completed successfully! Your environment is ready."
echo ""
log "info" "To activate the Python virtual environment in the future, run:"
log "info" "  source venv/bin/activate"
log "info" "To deactivate the virtual environment, run:"
log "info" "  deactivate"