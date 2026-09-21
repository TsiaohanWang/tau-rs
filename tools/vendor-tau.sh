#!/usr/bin/env bash
# Re-export a pinned tau commit into reference/tau/ (the only supported way to
# change the vendored snapshot) and stage the result. See reference/VENDORED.md.
#
# Usage: tools/vendor-tau.sh [ref]        (default: upstream HEAD)
# Env:   TAU_UPSTREAM=/path/to/tau        (default: ../tau)
#
# Notes:
# - The local venv (reference/tau/.venv) is preserved across re-exports.
# - `git add -f` is required: the vendored tree carries upstream .gitignore
#   files that would hide a few upstream-tracked paths from an untracked copy.
set -euo pipefail
cd "$(dirname "$0")/.."

UPSTREAM="${TAU_UPSTREAM:-../tau}"
REF="${1:-HEAD}"

if [[ ! -d "$UPSTREAM/.git" ]]; then
  echo "error: no tau git clone at '$UPSTREAM' (set TAU_UPSTREAM)" >&2
  exit 1
fi

SHA="$(git -C "$UPSTREAM" rev-parse --verify "$REF^{commit}")"
VERSION="$(git -C "$UPSTREAM" show "$SHA:pyproject.toml" | grep -m1 '^version' | cut -d'"' -f2)"
DATE="$(date -u +%Y-%m-%d)"

# Keep the venv out of the wipe and out of the force-add below.
VENV_HOLD="reference/.venv-hold"
rm -rf "$VENV_HOLD"
if [[ -d reference/tau/.venv ]]; then
  mv reference/tau/.venv "$VENV_HOLD"
fi

rm -rf reference/tau
mkdir -p reference/tau
git -C "$UPSTREAM" archive "$SHA" | tar -x -C reference/tau

printf '%s\n' "$SHA" > reference/REVISION

python3 - "$SHA" "$VERSION" "$DATE" "$UPSTREAM" <<'PY'
import re
import sys
from pathlib import Path

sha, version, date, upstream = sys.argv[1:5]
path = Path("reference/VENDORED.md")
text = path.read_text(encoding="utf-8")
text = re.sub(r"- commit: `[0-9a-f]+`", f"- commit: `{sha}`", text)
text = re.sub(r"- version: `[^`]+`", f"- version: `{version}`", text)
text = re.sub(r"- vendored at: [0-9-]+", f"- vendored at: {date}", text)
text = re.sub(r"- exported from: .*", f"- exported from: `{upstream}`", text)
path.write_text(text, encoding="utf-8")
PY

find reference/tau -type d -name __pycache__ -prune -exec rm -rf {} +
git add -f reference/tau reference/REVISION reference/VENDORED.md

if [[ -d "$VENV_HOLD" ]]; then
  mv "$VENV_HOLD" reference/tau/.venv
fi

echo "vendored tau $VERSION @ $SHA (staged)"
echo "next: ./tools/fixtures.sh && ./tools/fixtures.sh --check && cargo test"
