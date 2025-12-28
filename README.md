# cloudflare-speed-cli

[![Rust](https://img.shields.io/badge/rust-1.81+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPLv3-blue.svg)](LICENSE)

A CLI tool that displays network speed test results from Cloudflare's speed test service in a TUI interface.

![screenshot](./images/screenshot.png)

## Features

- **Interactive TUI**: Real-time charts and statistics
- **Speed Tests**: Measures download/upload throughput, idle latency, and loaded latency
- **History**: View and manage past test results
- **Export**: Save results as JSON
- **Text/JSON Modes**: Headless operation for scripting

## Installation

### Linux (All Distributions)

Download the static binary for your system from the [latest release](https://github.com/kavehtehrani/cloudflare-speed-cli/releases).

Or for the lazy:

```bash
# For x86_64 systems
wget https://github.com/kavehtehrani/cloudflare-speed-cli/releases/latest/download/cloudflare-speed-cli_-x86_64-unknown-linux-musl.tar.xz
tar -xJf cloudflare-speed-cli_-x86_64-unknown-linux-musl.tar.xz
sudo mv cloudflare-speed-cli /usr/local/bin/

# For ARM64 systems
wget https://github.com/kavehtehrani/cloudflare-speed-cli/releases/latest/download/cloudflare-speed-cli_-aarch64-unknown-linux-musl.tar.xz
tar -xJf cloudflare-speed-cli_-aarch64-unknown-linux-musl.tar.xz
sudo mv cloudflare-speed-cli /usr/local/bin/
```

### macOS

```bash
# For Intel Macs
wget https://github.com/kavehtehrani/cloudflare-speed-cli/releases/latest/download/cloudflare-speed-cli_-x86_64-apple-darwin.tar.xz
tar -xJf cloudflare-speed-cli_-x86_64-apple-darwin.tar.xz
sudo mv cloudflare-speed-cli /usr/local/bin/

# For Apple Silicon (M1/M2/M3)
wget https://github.com/kavehtehrani/cloudflare-speed-cli/releases/latest/download/cloudflare-speed-cli_-aarch64-apple-darwin.tar.xz
tar -xJf cloudflare-speed-cli_-aarch64-apple-darwin.tar.xz
sudo mv cloudflare-speed-cli /usr/local/bin/
```

### Windows

1. Download `cloudflare-speed-cli_-x86_64-pc-windows-msvc.zip` from [GitHub Releases](https://github.com/kavehtehrani/cloudflare-speed-cli/releases/latest)
2. Extract the ZIP file
3. Move `cloudflare-speed-cli.exe` to a directory in your PATH (e.g., `C:\Windows\System32` or add a custom directory to PATH)

### From Source (Cargo)

```bash
cargo install --git https://github.com/kavehtehrani/cloudflare-speed-cli --features tui
```

## Usage

Run with the TUI (default):

```bash
cloudflare-speed-cli
```

Text output mode:

```bash
cloudflare-speed-cli --text
```

JSON output mode:

```bash
cloudflare-speed-cli --json
```

## Source

Uses endpoints from https://speed.cloudflare.com/

## Contributing

Contributions and comments are very welcome! Please feel free to open issues or pull requests.
