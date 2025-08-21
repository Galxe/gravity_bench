# Use an official Rust image as a parent image
FROM rust:1.86-slim-bookworm AS builder

# Install necessary dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    curl \
    python3 \
    python3-pip \
    python3-venv \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Create a virtual environment
RUN python3 -m venv /opt/venv
# Add venv to the PATH
ENV PATH="/opt/venv/bin:$PATH"

# Copy the entire project to the container
COPY . .

# Install Python dependencies from requirements.txt into the venv
# No --break-system-packages needed now
RUN pip install --no-cache-dir -r requirements.txt

# Run the setup script to download contracts and install npm packages
# This will use the python/pip from the virtual environment
RUN bash ./setup.sh

# This script patches one of the contract files.
# This will use the python from the virtual environment
RUN python scripts/refresh_init_code.py

# Build the Rust application
RUN cargo build --release

# --- Final Stage ---
FROM debian:bookworm-slim

# Install runtime dependencies for OpenSSL, Python, Node.js and other libraries
RUN apt-get update && apt-get install -y --no-install-recommends \
    libssl3 \
    ca-certificates \
    python3 \
    python3-pip \
    python3-venv \
    curl \
    wget \
    nodejs \
    npm \
    git \
    && rm -rf /var/lib/apt/lists/*

# Set working directory to /tmp for temporary files
WORKDIR /tmp

# Copy the built binary and necessary files from the builder stage
COPY --from=builder /app/target/release/gravity_bench .
COPY --from=builder /app/bench_config.template ./bench_config.toml

# Copy Python virtual environment and scripts
COPY --from=builder /opt/venv /opt/venv
COPY --from=builder /app/scripts ./scripts

# Copy Node.js dependencies (OpenZeppelin contracts)
COPY --from=builder /app/node_modules ./node_modules

# Copy contracts if they are needed at runtime (they might be)
COPY --from=builder /app/contracts ./contracts

# Set up Python virtual environment PATH and solcx directory
ENV PATH="/opt/venv/bin:$PATH"
ENV SOLCX_BINARY_PATH="/tmp/.solcx"

# Ensure /tmp permissions and create solcx directory
RUN chmod 1777 /tmp && \
    mkdir -p /tmp/.solcx && \
    chmod 777 /tmp/.solcx

# The entrypoint to run the application
# The user will provide arguments via docker run or docker-compose
ENTRYPOINT ["./gravity_bench"]

# Default command can be empty or a default action like --help
CMD ["--help"]
