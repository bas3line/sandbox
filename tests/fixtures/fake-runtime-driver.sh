#!/bin/sh
# Protocol fixture only. It provides no isolation and must never be used in production.
set -eu

action=${1:-}
request=$(/bin/dd bs=64k 2>/dev/null)
[ -n "$request" ] || { printf '%s\n' '{"success":false,"result":null,"error":"empty request"}'; exit 0; }

case "$action" in
  probe)
    printf '%s\n' '{"success":true,"result":{"name":"fake-test-runtime","version":"1","tiers":["container","microvm"]},"error":null}'
    ;;
  create|delete)
    printf '%s\n' '{"success":true,"result":{},"error":null}'
    ;;
  exec)
    printf '%s\n' '{"success":true,"result":{"exit_code":0,"stdout":"fake runtime ok\n","stderr":"","truncated":false},"error":null}'
    ;;
  *)
    printf '%s\n' '{"success":false,"result":null,"error":"unknown action"}'
    ;;
esac
