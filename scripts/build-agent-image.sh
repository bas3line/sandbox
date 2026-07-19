#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
. "$repo_dir/images/agents/versions.env"

agent=${1:-}
case "$agent" in
  codex) package=@openai/codex; version=$CODEX_VERSION ;;
  claude) package=@anthropic-ai/claude-code; version=$CLAUDE_VERSION ;;
  opencode) package=opencode-ai; version=$OPENCODE_VERSION ;;
  pi) package=@mariozechner/pi-coding-agent; version=$PI_VERSION ;;
  *) printf '%s\n' "usage: $0 {codex|claude|opencode|pi}" >&2; exit 2 ;;
esac

tag="sandbox-agent-${agent}:local"
docker build \
  --file "$repo_dir/images/agents/Dockerfile" \
  --build-arg "AGENT_PACKAGE=$package" \
  --build-arg "AGENT_VERSION=$version" \
  --tag "$tag" \
  "$repo_dir"
printf '%s\n' "built $tag ($package@$version)"
