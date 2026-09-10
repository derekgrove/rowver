# rowver

## Agent skills

### Issue tracker

Issues live on GitLab (gitlab.cern.ch/dgrove/rowver) via the `glab` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: needs-triage, needs-info, ready-for-agent, ready-for-human, wontfix. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `CONTEXT.md` at the root, ADRs in `docs/adr/`. See `docs/agents/domain.md`.

### Independent verification

For cartridge contracts, numerical semantics, extraction, remote-data behavior, or
cross-repository checks, see `docs/agents/verification.md`.

### Commit gate: cross-model adversarial review

After `/code-review` passes and before committing, the working agent runs an adversarial review by the *other* vendor's model — never itself:

- Claude is the worker → reviewer is Codex: `codex exec "<review prompt>"`
- Codex is the worker → reviewer is Claude fable 5.1: `claude -p --model claude-fable-5.1 "<review prompt>"`

Review prompt (same for both): `Adversarially review the staged diff (git diff --cached) for correctness bugs. Context: <one-line summary of the change and its intent>. End your reply with exactly AGREED or REJECTED (with findings).` Procedure per round:

1. Run the reviewer with the prompt above.
2. REJECTED → fix the real findings (push back on wrong ones in the next round's prompt), re-stage, go to 1 with the round number incremented.
3. AGREED → commit, recording convergence in the message as `<reviewer> AGREED r<N>` (e.g. `codex AGREED r2`; r1, r2, … = rounds run).

The reviewer runs read-only against the working tree; the working agent makes all edits. The review rounds happen inside the implement phase — don't clear or compact context until the commit lands.
