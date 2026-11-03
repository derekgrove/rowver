#!/usr/bin/env bash
# Whether an rvr names the commit checked out here, as every file it presses records it:
# check-commit.sh RVR, in a job of the release workflow, from the checkout.
set -euo pipefail
rvr=${1:?usage: check-commit.sh RVR, the rvr to check, run from the checkout it was built from}
want=$(git rev-parse HEAD)
v=$("$rvr" --version)
echo "$v"
case $v in
  *"commit $want)") ;;
  *"commit unknown)")
    echo "This rvr names no commit: its build.rs could not ask git about the checkout, or the source" \
      "package it was built from carries none. In a container, trust the checkout before building: git" \
      "config --global --add safe.directory \"\$GITHUB_WORKSPACE\". A source package gets its commit" \
      "only from the release's sdist job; one built without it says unknown." >&2
    exit 1 ;;
  *-dirty*)
    echo "This rvr was built from changed code: the job that built it changed files under crates/, or" \
      "Cargo.toml or Cargo.lock, before building. Add git status to that job to see which, and build" \
      "the checkout as it is." >&2
    exit 1 ;;
  *)
    echo "This rvr names another commit than $want, the one checked out here: the job that built it" \
      "checked out another. Every job checks out the run's own commit (actions/checkout with no ref)." >&2
    exit 1 ;;
esac
