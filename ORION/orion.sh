#!/usr/bin/env bash
# Orion launcher: CLI chat, REST API, or one-shot question.
set -euo pipefail
cd "$(dirname "$0")"

case "${1:-}" in
  serve)
    shift; exec python3 -m orion.api "${1:-8765}" ;;
  ask)
    shift; exec python3 -m orion.cli "$@" ;;
  learn)
    shift; exec python3 -m orion.cli --learn "$@" ;;
  test)
    exec python3 -m unittest discover -s tests -v ;;
  eval-humaneval)
    shift; exec python3 -m orion.eval humaneval "$@" ;;
  eval-rubric)
    shift; exec python3 -m orion.eval rubric "$@" ;;
  *)
    exec python3 -m orion.cli "$@" ;;
esac
