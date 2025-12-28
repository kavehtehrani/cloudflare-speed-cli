#!/usr/bin/env sh
set -eu

REPO="kavehtehrani/cloudflare-speed-cli"
BIN="cloudflare-speed-cli"
PKG_NAME="cloudflare-speed-cli_"

echo "Fetching latest version..." >&2
VERSION="${VERSION:-$(curl -fsSL \
  https://api.github.com/repos/${REPO}/releases/latest \
  | sed -n 's/.*"tag_name": "\(.*\)".*/\1/p')}"

[ -n "$VERSION" ] || { echo "Error: Could not resolve latest version" >&2; exit 1; }
echo "Version: ${VERSION}" >&2

case "$(uname -s)" in
  Linux) OS="unknown-linux-musl" ;;
  Darwin) OS="apple-darwin" ;;
  *) echo "Error: Unsupported OS: $(uname -s)" >&2; exit 1 ;;
esac
echo "OS: ${OS}" >&2

case "$(uname -m)" in
  x86_64) ARCH="x86_64" ;;
  arm64|aarch64) ARCH="aarch64" ;;
  *) echo "Error: Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac
echo "Architecture: ${ARCH}" >&2

FILE="${PKG_NAME}${ARCH}-${OS}.tar.xz"
BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"
FILE_URL="${BASE_URL}/${FILE}"
SHA256_URL="${BASE_URL}/${FILE}.sha256"

echo "Download URL: ${FILE_URL}" >&2
echo "SHA256 URL: ${SHA256_URL}" >&2

TMP="$(mktemp -d)"
trap "rm -rf ${TMP}" EXIT
cd "$TMP"

echo "Downloading ${FILE}..." >&2
if ! curl -fsSLO "${FILE_URL}"; then
  echo "Error: Failed to download ${FILE_URL}" >&2
  echo "HTTP status: $(curl -s -o /dev/null -w '%{http_code}' "${FILE_URL}")" >&2
  exit 1
fi

echo "Downloading ${FILE}.sha256..." >&2
if ! curl -fsSLO "${SHA256_URL}"; then
  echo "Error: Failed to download ${SHA256_URL}" >&2
  echo "HTTP status: $(curl -s -o /dev/null -w '%{http_code}' "${SHA256_URL}")" >&2
  exit 1
fi

echo "Verifying checksum..." >&2
sha256sum -c "${FILE}.sha256" || { echo "Error: Checksum verification failed" >&2; exit 1; }

echo "Extracting archive..." >&2
tar -xJf "$FILE"

# The archive extracts to a directory, find the binary inside
EXTRACTED_DIR="${PKG_NAME}${ARCH}-${OS}"
if [ -d "$EXTRACTED_DIR" ]; then
  BINARY_PATH="${EXTRACTED_DIR}/${BIN}"
else
  BINARY_PATH="${BIN}"
fi

if [ ! -f "$BINARY_PATH" ]; then
  echo "Error: Binary not found at ${BINARY_PATH}" >&2
  echo "Contents of ${TMP}:" >&2
  ls -la "$TMP" >&2
  exit 1
fi

INSTALL_DIR="${HOME}/.local/bin"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$BINARY_PATH" "$INSTALL_DIR/$BIN"

echo "Installed ${BIN} ${VERSION} to ${INSTALL_DIR}"
echo "Make sure ${INSTALL_DIR} is in your PATH"
