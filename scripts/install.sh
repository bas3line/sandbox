#!/bin/sh
# Source-tree bootstrap for the canonical registry installer.
set -eu
umask 077

fail() {
  printf '%s\n' "sandbox installer bootstrap: $*" >&2
  exit 1
}

BASE_URL=${SANDBOX_INSTALL_BASE_URL:-https://tools.yshubham.com}
BASE_URL=${BASE_URL%/}
case "$BASE_URL" in
  https://*) ;;
  *) fail "SANDBOX_INSTALL_BASE_URL must use HTTPS" ;;
esac
command -v curl >/dev/null 2>&1 || fail "missing required command: curl"
command -v mktemp >/dev/null 2>&1 || fail "missing required command: mktemp"

temporary=$(mktemp -d "${TMPDIR:-/tmp}/sandbox-installer-bootstrap.XXXXXX")
trap 'rm -rf "$temporary"' EXIT INT TERM
curl --fail --location --proto '=https' --proto-redir '=https' --silent --show-error \
  --output "$temporary/install.sh" -- "$BASE_URL/sandbox/install.sh"
/bin/sh "$temporary/install.sh"
