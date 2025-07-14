#!/bin/sh

# Check if Rust is installed
if ! command -v rustc &> /dev/null
then
    echo "Rust is not installed. Installing Rust..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    # Source cargo environment
    export PATH="$HOME/.cargo/bin:$PATH"
    source "$HOME/.cargo/env"
    cargo update
else
    echo "Rust is already installed. Skipping installation."
fi

# Ensure cargo is in PATH
export PATH="$HOME/.cargo/bin:$PATH"

# Continue with build process

# if dev profile, build with dev profile
if [ "$1" = "dev" ]; then
    cargo build --profile dev && cargo install --profile dev --path . --root ~/.local
elif [ "$1" = "offline" ]; then
    cargo build --profile dev --offline && cargo install --profile dev --offline --path . --root ~/.local
else
    cargo build --release && cargo install --path . --root ~/.local
fi

if ! echo "$PATH" | grep -q "$HOME/.local/bin"; then
    if ! grep -Fxq 'export PATH="$HOME/.local/bin:$PATH"' ~/.bashrc; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
    fi
    if ! grep -Fxq 'export PATH="$HOME/.local/bin:$PATH"' ~/.zshrc; then
        echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
    fi
    export PATH="$HOME/.local/bin:$PATH"
fi

