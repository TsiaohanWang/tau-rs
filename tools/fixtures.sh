#!/usr/bin/env bash
# Regenerate the differential fixtures with the vendored tau environment.
# Extra arguments are forwarded to gen_fixtures.py (`--check`, `--stdout`).
#
# Resolution order for the interpreter:
#   1. $TAU_PYTHON (explicit override)
#   2. reference/tau/.venv/bin/python (created by tools/setup-reference.sh)
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ -n "${TAU_PYTHON:-}" ]]; then
  PYTHON="$TAU_PYTHON"
elif [[ -x reference/tau/.venv/bin/python ]]; then
  PYTHON="reference/tau/.venv/bin/python"
else
  echo "error: no Python env found." >&2
  echo "run: ./tools/setup-reference.sh    (or set TAU_PYTHON)" >&2
  exit 1
fi

exec "$PYTHON" tools/gen_fixtures.py "$@"
