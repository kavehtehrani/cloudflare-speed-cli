# cloudflare-speed-cli

[![Rust](https://img.shields.io/badge/rust-1.81+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPLv3-blue.svg)](LICENSE)

A CLI tool that displays network speed test results from Cloudflare's speed test service in a TUI interface.

![screenshot](./images/screenshot.png)

## Features

- **Speed Tests**: Measures download/upload throughput, idle latency, and loaded latency
- **Interactive TUI**: Real-time charts and statistics
- **History**: View and manage past test results
- **Export**: Save results as JSON
- **Text/JSON Modes**: Headless operation for scripting

## Installation

### From Source

My preferred way if you have cargo installed

```bash
cargo install --git https://github.com/kavehtehrani/cloudflare-speed-cli --features tui
```

### Installation Script

For the lazy:

```bash
curl -fsSL https://raw.githubusercontent.com/kavehtehrani/cloudflare-speed-cli/main/install.sh | sh
```

### Binaries

Download the static binary for your system from the
[latest release](https://github.com/kavehtehrani/cloudflare-speed-cli/releases).

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
