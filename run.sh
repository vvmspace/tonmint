#!/bin/bash

echo "🚀 Starting TON Sniper & Seller..."

# 1. Check if Node.js dependencies are installed
if [ ! -d "worker/node_modules" ]; then
    echo "📦 Installing Node.js dependencies..."
    cd worker && npm install && cd ..
fi

# 2. Check if .env exists
if [ ! -f ".env" ]; then
    echo "⚠️ .env file not found! Please create it using .env.example"
    exit 1
fi

# 3. Kill existing instances
echo "🧹 Cleaning up old processes..."
pkill -f tonmint 2>/dev/null
pkill -f ts-node 2>/dev/null

# 4. Run the Rust bot
echo "🔫 Launching Sniper..."
cargo run --release
