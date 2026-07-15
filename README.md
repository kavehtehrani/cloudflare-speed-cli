<a href="https://github.com/kavehtehrani/cloudflare-speed-cli">
  <img alt="cloudflare-speed-cli" src="images/banner.svg">
</a>

# cloudflare-speed-cli

[![Crates.io](https://img.shields.io/crates/v/cloudflare-speed-cli.svg)](https://crates.io/crates/cloudflare-speed-cli)
[![Rust](https://img.shields.io/badge/rust-1.81+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPLv3-blue.svg)](LICENSE)

A CLI tool that displays network speed test results from Cloudflare's speed test service in a TUI interface.

> [!WARNING]
> If you're seeing zero download speeds, a hotfix has been implemented in v0.6.2. If you are on a version before this, upgrade by following the [installation instructions](#installation) below.

[demo.webm](https://github.com/user-attachments/assets/0a1fd635-fce9-431a-a3fc-ed4e5243775a)

## Features

- **Speed Tests**: Download/upload throughput, idle latency, and loaded latency
- **Diagnostics**: DNS resolution and TLS handshake timing run alongside every test
- **UDP Quality**: Packet loss, jitter, and MOS-style link scoring
- **IPv4 vs IPv6**: Optional side-by-side comparison (`--compare-ip-versions`), or force a single family with `-4`/`--ipv4-only` and `-6`/`--ipv6-only`
- **Traceroute**: Optional ICMP traceroute to the Cloudflare edge (`--traceroute`)
- **Proxy Support**: Route through HTTP, HTTPS, or SOCKS5 proxies (`--proxy`)
- **TLS Trust**: Loads OS truststore, or supply a private CA with `--certificate`
- **Interface Binding**: Bind to a specific network interface or source IP
- **Interactive TUI**: Real-time charts and statistics
- **History**: View and manage past test results
- **Export**: Save results as JSON or CSV. headless text/JSON modes for scripting

run with `--help` to see full list of features

![screenshot](./images/screenshot-history.png)
![screenshot](./images/screenshot-charts.png)


## Installation

### Cargo (crates.io)

My preferred way if you have cargo installed:

```bash
cargo install cloudflare-speed-cli
```

To install the latest unreleased version from git instead:

```bash
cargo install --git https://github.com/kavehtehrani/cloudflare-speed-cli --features tui
```

### Homebrew

This works for both older Intel and newer Silicon Mac computers.

```bash
brew tap kavehtehrani/tap
brew install cloudflare-speed-cli
```

### MacPorts

Supports both Intel and Apple Silicon Macs. The package is available on the [official MacPorts repository](https://ports.macports.org/port/cloudflare-speed-cli/) and can be installed with:

```bash
sudo port install cloudflare-speed-cli
```

### Installation Script

For the lazy:

```bash
curl -fsSL https://raw.githubusercontent.com/kavehtehrani/cloudflare-speed-cli/main/install.sh | sh
```

### Linux Distribution Packages

As other people have kindly contributed, I've merged their pull requests for the distro they have
contributed toward. My main OS is Ubuntu and Fedora, and the main distribution mechanism for this package is via
['cargo-dist'](https://github.com/axodotdev/cargo-dist) which I have found helpful in generating the compatible binaries
via github actions. If you would like to contribute for other distros, please open an issue or send in a pull request.

#### Arch Linux

The package can be installed from the [official repositories](https://archlinux.org/packages/extra/x86_64/cloudflare-speed-cli/):

```bash
pacman -S cloudflare-speed-cli
```

#### NixOS

```bash
 nix run github:kavehtehrani/cloudflare-speed-cli
```

### Binaries

Download the static binary for your system from the
[latest release](https://github.com/kavehtehrani/cloudflare-speed-cli/releases).

_Note: I have checked the binaries on Linux (x64 and ARM64) and on Windows 11 but I do not have an Apple device. If there is any issue please do let me know!_

### Docker

If you have docker installed you can run the speedtest by pulling in the image:

```bash
docker pull ghcr.io/kavehtehrani/cloudflare-speed-cli:latest
docker run --rm -it ghcr.io/kavehtehrani/cloudflare-speed-cli
```

If you also want the history of runs to persist, then you have to mount a volume, e.g.
`docker run --rm -it -v cloudflare-speed-data:/root/.local/share/cloudflare-speed-cli ghcr.io/kavehtehrani/cloudflare-speed-cli`

## Usage

Run with the TUI (default):

```bash
cloudflare-speed-cli
```

Text output mode:

```bash
cloudflare-speed-cli --text
```

To see all options:

```bash
cloudflare-speed-cli --help
```

## Source

Uses endpoints from [speed.cloudflare.com](https://speed.cloudflare.com/)
You can read more about how Cloudflare's speedtest works [here](https://speed.cloudflare.com/about/).

## Outstanding Issues

- Network information on Windows is incomplete. I haven't used Windows (outside gaming) in many years and unless there's demand for it I likely won't implement this part. Feel free to open a PR or an issue and we can chat. Honestly the only reason there's a Windows binary at all is because ['cargo-dist'](https://github.com/axodotdev/cargo-dist) made it so easy to do so.
- Unable to detect the SSID on macOS Sonoma 14.4 and later as this is now considered location-sensitive data.

## Contributing

Contributions and comments are very welcome! Please feel free to open issues or pull requests.
