#!/usr/bin/env sh
set -eu

ARTIFACT_DIR="${1:-dist/release}"
PUBLIC_KEY="${2:-}"
SIGNATURE_FILE="${3:-}"

if [ -z "$PUBLIC_KEY" ]; then
  echo "usage: $0 <artifact-dir> <public-key-pem> [signature-file]" >&2
  exit 1
fi

SUMS_FILE="$ARTIFACT_DIR/SHA256SUMS"
if [ -z "$SIGNATURE_FILE" ]; then
  SIGNATURE_FILE="$ARTIFACT_DIR/SHA256SUMS.sig"
fi

if [ ! -d "$ARTIFACT_DIR" ]; then
  echo "artifact directory not found: $ARTIFACT_DIR" >&2
  exit 1
fi

if [ ! -f "$SUMS_FILE" ]; then
  echo "SHA256SUMS file not found: $SUMS_FILE" >&2
  exit 1
fi

if [ ! -f "$PUBLIC_KEY" ]; then
  echo "public key file not found: $PUBLIC_KEY" >&2
  exit 1
fi

if [ ! -f "$SIGNATURE_FILE" ]; then
  echo "signature file not found: $SIGNATURE_FILE" >&2
  exit 1
fi

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required to verify release signatures" >&2
  exit 1
fi

openssl dgst -sha256 -verify "$PUBLIC_KEY" -signature "$SIGNATURE_FILE" "$SUMS_FILE" >/dev/null
echo "release signature verification ok: $SIGNATURE_FILE"
