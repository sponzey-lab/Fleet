#!/usr/bin/env sh
set -eu

ARTIFACT_DIR="${1:-}"
PRIVATE_KEY="${2:-}"
SIGNATURE_FILE="${3:-}"

if [ -z "$ARTIFACT_DIR" ] || [ -z "$PRIVATE_KEY" ]; then
  echo "usage: $0 <artifact-dir> <private-key-pem> [signature-file]" >&2
  exit 1
fi

SUMS_FILE="$ARTIFACT_DIR/SHA256SUMS"
if [ -z "$SIGNATURE_FILE" ]; then
  SIGNATURE_FILE="$ARTIFACT_DIR/SHA256SUMS.sig"
fi

if [ ! -f "$SUMS_FILE" ]; then
  echo "SHA256SUMS file not found: $SUMS_FILE" >&2
  exit 1
fi

if [ ! -f "$PRIVATE_KEY" ]; then
  echo "private key file not found: $PRIVATE_KEY" >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required to sign release checksums" >&2
  exit 1
fi

openssl dgst -sha256 -sign "$PRIVATE_KEY" -out "$SIGNATURE_FILE" "$SUMS_FILE"
echo "release checksum signature written: $SIGNATURE_FILE"
