#!/usr/bin/env sh
set -eu

ARTIFACT_DIR="${1:-dist/release}"
SUMS_FILE="$ARTIFACT_DIR/SHA256SUMS"

if [ ! -d "$ARTIFACT_DIR" ]; then
  echo "artifact directory not found: $ARTIFACT_DIR" >&2
  exit 1
fi

if [ ! -f "$SUMS_FILE" ]; then
  echo "SHA256SUMS file not found: $SUMS_FILE" >&2
  exit 1
fi

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
    return
  fi
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
    return
  fi
  echo "sha256sum or shasum is required" >&2
  exit 1
}

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT INT TERM

verified_count=0

while read -r expected file_name; do
  case "$expected" in
    ""|\#*)
      continue
      ;;
  esac
  case "$file_name" in
    ""|*/*|*..*)
      echo "invalid artifact name in SHA256SUMS: $file_name" >&2
      exit 1
      ;;
  esac

  archive="$ARTIFACT_DIR/$file_name"
  if [ ! -f "$archive" ]; then
    echo "artifact listed in SHA256SUMS was not found: $archive" >&2
    exit 1
  fi

  actual="$(sha256_file "$archive")"
  if [ "$actual" != "$expected" ]; then
    echo "checksum mismatch for $file_name" >&2
    echo "expected: $expected" >&2
    echo "actual:   $actual" >&2
    exit 1
  fi

  extract_dir="$TMP_DIR/$verified_count"
  mkdir -p "$extract_dir"
  tar -xzf "$archive" -C "$extract_dir"
  binary="$(find "$extract_dir" -type f -name sponzey | head -n 1)"
  if [ -z "$binary" ]; then
    echo "artifact does not contain a sponzey binary: $file_name" >&2
    exit 1
  fi
  if [ ! -x "$binary" ]; then
    echo "artifact sponzey binary is not executable: $file_name" >&2
    exit 1
  fi

  verified_count=$((verified_count + 1))
done < "$SUMS_FILE"

if [ "$verified_count" -eq 0 ]; then
  echo "no artifacts were listed in $SUMS_FILE" >&2
  exit 1
fi

echo "standalone artifact verification ok: $verified_count artifact(s)"
