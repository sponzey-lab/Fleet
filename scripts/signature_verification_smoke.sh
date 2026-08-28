#!/usr/bin/env sh
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

ARTIFACT_DIR="$TMP_DIR/release"
PRIVATE_KEY="$TMP_DIR/release-private.pem"
PUBLIC_KEY="$TMP_DIR/release-public.pem"
mkdir -p "$ARTIFACT_DIR"

if ! command -v openssl >/dev/null 2>&1; then
  echo "openssl is required for release signature smoke" >&2
  exit 1
fi

printf '%s\n' "d41d8cd98f00b204e9800998ecf8427e  fleet-linux-x64.tar.gz" > "$ARTIFACT_DIR/SHA256SUMS"
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$PRIVATE_KEY" >/dev/null 2>&1
openssl pkey -in "$PRIVATE_KEY" -pubout -out "$PUBLIC_KEY" >/dev/null 2>&1

"$SCRIPT_DIR/sign_release_sums.sh" "$ARTIFACT_DIR" "$PRIVATE_KEY" >/dev/null
"$SCRIPT_DIR/verify_release_signature.sh" "$ARTIFACT_DIR" "$PUBLIC_KEY" >/dev/null

printf '%s\n' "tampered" >> "$ARTIFACT_DIR/SHA256SUMS"
if "$SCRIPT_DIR/verify_release_signature.sh" "$ARTIFACT_DIR" "$PUBLIC_KEY" >/dev/null 2>&1; then
  echo "signature verification unexpectedly passed for a tampered SHA256SUMS" >&2
  exit 1
fi

"$SCRIPT_DIR/sign_release_sums.sh" "$ARTIFACT_DIR" "$PRIVATE_KEY" >/dev/null
rm "$ARTIFACT_DIR/SHA256SUMS.sig"
if "$SCRIPT_DIR/verify_release_signature.sh" "$ARTIFACT_DIR" "$PUBLIC_KEY" >/dev/null 2>&1; then
  echo "signature verification unexpectedly passed with no signature file" >&2
  exit 1
fi

echo "release signature smoke ok"
