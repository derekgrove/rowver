#!/bin/sh
# Step-0 gate: the documented workflow must run from an installed wheel in a
# clean venv. Usage: sh wheel_gate.sh [python]
set -eu

PY="${1:-python3}"
ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

"$PY" -m pip wheel --no-deps -w "$WORK/dist" "$ROOT" >/dev/null
"$PY" -m venv "$WORK/venv"
VPY="$WORK/venv/bin/python"
"$VPY" -m pip install --quiet "$WORK/dist"/rowver-*.whl

cd "$WORK"
"$VPY" -c "from rowver import Cart, CartError, FormatError, study_names"
"$VPY" -m rowver.cli --help >/dev/null

# rowver new: the template ships as package data and imports the public package
"$VPY" -m rowver.cli new demo
grep -q "from rowver import Cart" demo/cart.py
"$VPY" -m rowver.cli run --list >/dev/null

# rowver run on a study directory path fails CLEANLY without ROOT files
if "$VPY" -m rowver.cli run demo >run.out 2>&1; then
    echo "FAIL: run succeeded without input files"; exit 1
fi
grep -q "error:" run.out

# check / show on a real cartridge written via the public API
"$VPY" - <<'EOF'
import numpy as np
from rowver import rvr
made = {"study_name": "wheel-gate", "by": f"RowVer {rvr.VERSION}", "built": "wheel_gate",
        "samples": {"mc": {"files": {}, "weights": {"kind": "branch", "value": "w"}, "metadata": {}},
                    "data": {"files": {}, "weights": None, "metadata": {}}},
        "keep": {"kind": "branch", "value": "pass"}, "prepare": None, "cart": None}
meta, buf = rvr.pack(
    {"mc": {"pt": np.array([10.5, 20.0])}, "data": {"pt": np.array([30.0, 40.0])}},
    weighted={"mc": True, "data": False}, weights={"mc": np.array([0.5, 2.0])}, made=made)
rvr.write("gate.rvr", meta, buf)
EOF
"$VPY" -m rowver.cli check gate.rvr
"$VPY" -m rowver.cli show gate.rvr >/dev/null

# the console entry point itself
"$WORK/venv/bin/rowver" check gate.rvr >/dev/null

echo "wheel gate: green"
