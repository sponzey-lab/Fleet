#!/usr/bin/env sh
set -eu

require_pattern() {
  file="$1"
  pattern="$2"
  message="$3"
  if ! grep -F "$pattern" "$file" >/dev/null 2>&1; then
    echo "$message" >&2
    exit 1
  fi
}

require_pattern docs/storage.md "## S3-Compatible Artifact Store Decision" \
  "docs/storage.md must include the S3-compatible artifact store decision section"
require_pattern docs/storage.md "Decision: defer S3-compatible adapter implementation" \
  "S3 decision must explicitly state whether implementation is deferred"
require_pattern docs/storage.md "No runtime configuration mutation is allowed" \
  "S3 decision must prohibit runtime configuration mutation"
require_pattern docs/storage.md "Credentials must be startup secrets or secret references" \
  "S3 decision must define credential source boundaries"
require_pattern docs/storage.md "Application and Domain crates must not depend on an S3 SDK" \
  "S3 decision must preserve application/domain dependency direction"
require_pattern docs/storage.md 'typed immutable `ArtifactStoreSettings`' \
  "storage docs must describe bootstrap-only typed artifact store settings"
require_pattern docs/storage.md "Runtime API, Web Admin, request payload, process env" \
  "storage docs must prohibit runtime artifact store backend/root mutation"
require_pattern docs/storage.md "Implementation trigger" \
  "S3 decision must define the next implementation trigger"
require_pattern docs/security.md "S3-compatible artifact storage credentials" \
  "docs/security.md must define S3-compatible credential handling"
require_pattern docs/security.md "Artifact store backend selection is represented by typed immutable" \
  "docs/security.md must define artifact store settings boundary"
require_pattern docs/feature-matrix.md "Decision recorded; adapter implementation deferred" \
  "feature matrix must reflect the S3 adapter decision"
require_pattern docs/release-notes-mvp.md "S3-compatible adapter decision is recorded" \
  "release notes must mention the S3-compatible adapter decision"

echo "storage decision gate ok"
