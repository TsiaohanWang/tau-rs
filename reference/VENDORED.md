# Vendored `tau` reference (read-only)

> **DO NOT EDIT anything under `reference/tau/`.** It is a byte-for-byte export
> of an upstream commit, not part of the Rust workspace. Any local edit is lost
> on the next `tools/vendor-tau.sh` run. If the port needs different behavior,
> adapt the Rust side; if the Python source itself looks wrong, open an ADR
> instead of patching the snapshot.

- upstream: <https://github.com/huggingface/tau> (canonical)
- exported from: `../tau`
- commit: `f09b15807788d24b8c42af09ff73ee0d8acbd597` (see `reference/REVISION`)
- version: `0.4.4`
- vendored at: 2026-09-21
- export command: `git -C ../tau archive <sha> | tar -x -C reference/tau`
- python env: `uv sync --frozen` inside `reference/tau` (deps pinned by
  `reference/tau/uv.lock`; pydantic 2.13.4, the version used to verify the
  differential corpus)

## Day-to-day use

- Read the Python source here; never open the external clone.
- `tools/setup-reference.sh` creates `reference/tau/.venv` (gitignored).
- `tools/fixtures.sh` uses that interpreter by default; `TAU_PYTHON` overrides.

## Updating the snapshot

```bash
tools/vendor-tau.sh <sha-or-tag>   # re-export, refresh REVISION/VENDORED.md, stage
./tools/fixtures.sh                # regenerate the Python↔Rust corpus
./tools/fixtures.sh --check        # expected drift is visible here
cargo test                         # differential tests surface behavior changes
```

The script preserves `reference/tau/.venv`, force-stages the snapshot (upstream
`.gitignore` files would otherwise hide a few upstream-tracked paths), and never
stages the venv or bytecode caches. Commit the bump separately from Rust code
changes so the behavior diff stays reviewable.
