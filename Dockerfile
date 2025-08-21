# Use an official Rust image as a parent image
FROM rust:1.77-slim-bookworm AS builder

# Install necessary dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    curl \
    python3 \
    python3-pip \
    nodejs \
    npm \
    && rm -rf /var/lib/apt/lists/*

# Set the working directory
WORKDIR /app

# Copy the entire project to the container
COPY . .

# Install Python dependencies from requirements.txt if it exists
# setup.sh runs this, but we can do it earlier to cache the layer
RUN if [ -f requirements.txt ]; then pip3 install --no-cache-dir -r requirements.txt; fi

# Run the setup script to download contracts and install npm packages
# This will clone uniswap repos into contracts/ and run npm install
RUN bash ./setup.sh

# This script patches one of the contract files.
RUN python3 scripts/refresh_init_code.py

# Build the Rust application
RUN cargo build --release

# --- Final Stage ---
FROM debian:bookworm-slim

# Create a non-root user
RUN groupadd -r appgroup && useradd -r -g appgroup -s /bin/bash appuser

# Set working directory
WORKDIR /home/appuser/

# Copy the built binary and necessary files from the builder stage
COPY --from=builder /app/target/release/txgen .
COPY --from=builder /app/bench_config.toml .
COPY --from=builder /app/deploy.json .

# Copy contracts if they are needed at runtime (they might be)
COPY --from=builder /app/contracts ./contracts

# Change ownership to the new user
RUN chown -R appuser:appgroup /home/appuser

# Switch to the non-root user
USER appuser

# The entrypoint to run the application
# The user will provide arguments via docker run or docker-compose
ENTRYPOINT ["./txgen"]

# Default command can be empty or a default action like --help
CMD ["--help"]
