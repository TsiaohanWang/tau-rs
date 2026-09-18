#!/usr/bin/env bash
# Regenerate the differential fixtures with the tau virtualenv.
# Extra arguments are forwarded to gen_fixtures.py (`--check`, `--stdout`).
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -n "${TAU_PYTHON:-}" ]]; then
  PYTHON="$TAU_PYTHON"
elif [[ -x ../tau/.venv/bin/python ]]; then
  PYTHON="../tau/.venv/bin/python"
else
  PYTHON="python3"
fi

exec "$PYTHON" tools/gen_fixtures.py "$@"
