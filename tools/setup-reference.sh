#!/usr/bin/env bash
# Create the Python environment for the vendored tau snapshot.
#
#   uv sync --frozen   -> reference/tau/.venv, deps pinned by reference/tau/uv.lock
#
# Safe to re-run. Requires `uv` and network access on the first run.
set -euo pipefail
cd "$(dirname "$0")/.."

REFERENCE="reference/tau"
if [[ ! -d "$REFERENCE" ]]; then
  echo "error: $REFERENCE missing; run tools/vendor-tau.sh first" >&2
  exit 1
fi

if [[ ! -x "$REFERENCE/.venv/bin/python" ]]; then
  (cd "$REFERENCE" && uv sync --frozen)
fi

"$REFERENCE/.venv/bin/python" - <<'PY'
import pydantic

print(f"reference interpreter ready (pydantic {pydantic.VERSION})")
PY

PYTHONPATH="$REFERENCE/src" "$REFERENCE/.venv/bin/python" -c \
  "import tau_agent.messages; print('tau_agent import ok')"
