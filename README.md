# receipt

**No receipt, no "done".** receipt is a Stop hook for Claude Code and Codex. When the agent's final message says "tests pass", "已提交" or "构建成功", receipt looks in the session transcript for the command that proves it. If that command never ran, failed, or ran before the last code edit, receipt stops the agent once and asks it to run the check or reword the claim.

- Deterministic: no LLM, no network. One Rust binary that reads only the transcript.
- Bilingual: English and Chinese claims, including Chinese enumerations such as "格式、严格 Clippy、release 测试编译均通过".
- Hosts: Claude Code `Stop` / `SubagentStop` and Codex `Stop`, with both transcript formats parsed.
- Fast: p95 73 ms on a 102 MB transcript, about 12 ms when the message contains no claim.

```
receipt: 1 claim in your final message has no passing run in this session.
1. [test] "All tests pass." — UNSUPPORTED: no test command ran in this session.
FIX: for each item, either run that check once in the foreground and report its real result, or reword only that sentence to say what was actually verified (for example "tests not run").
If a check fails, report the failure. Fix it only if fixing it is part of the task you were given.
DO NOT: edit tests or config to make a check pass, add new checks or scripts, or re-run checks that are not listed above.
```

That block, and what the agent did next, comes from a real Claude Code session: [docs/demo.md](docs/demo.md).

## Install

```bash
cargo install --git https://github.com/majiayu000/receipt
```

(Until the repository is published: `cargo install --path .` from a checkout.)

## Hook setup

### Claude Code

In `~/.claude/settings.json` (all projects) or `.claude/settings.json` (one project):

```json
{
  "hooks": {
    "Stop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }],
    "SubagentStop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }]
  }
}
```

To try it without touching your settings: `claude --settings ./receipt-settings.json` with the JSON above.

### Codex

In `~/.codex/hooks.json` or `<repo>/.codex/hooks.json`, then trust the hook once with `/hooks`:

```json
{ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }] } }
```

### What the hook does

| Situation | Hook result |
|---|---|
| No claim in the final message | exit 0, the transcript is not even read |
| Every claim has a passing run after the last code edit | exit 0 |
| A claim is UNSUPPORTED, CONTRADICTED or STALE | exit 2 with the feedback above. Both hosts continue the turn |
| Same stop cycle, already blocked once (`stop_hook_active`) | exit 0 plus `systemMessage` "receipt: still unverified: …" for you |
| Unreadable transcript or bad input | exit 1 with `receipt: …`. Shown as a hook error, never a block |

To turn it off for one session, set `RECEIPT_OFF=1`.

## What counts as a receipt

The full tables are in [docs/design.md](docs/design.md). In short:

| Claim | Receipt |
|---|---|
| test | `cargo test`, `pytest`, `go test`, `bun/npm test`, `vitest`, `jest`, `ctest`, … or a `test`/`verify` script run by an interpreter |
| build | `cargo build/check/test/clippy`, `go build`, `tsc`, `npm run build`, `make`, `cmake --build`, … |
| lint / typecheck / format | `clippy`, `eslint`, `ruff`, `golangci-lint` / `tsc`, `mypy`, `pyright` / `cargo fmt`, `prettier`, `black`, … |
| commit / push / pr | `git commit` / `git push` / `gh pr create` |
| deploy | `vercel`, `fly deploy`, `wrangler deploy`, `kubectl apply`, `npm publish`, `gh release create`, … (an unsupported deploy only warns) |

How the verdicts are decided:

- The last matching run decides.
- A run that exited 0 but whose output reports failures (`cargo test | tail`, `|| true`) counts as failed.
- A run denied by permissions or a hook never ran.
- A background run counts only once its completion notification arrives.
- Edits to docs or scratch files do not make a run stale.
- If an unrecognised command (a harness script, a built binary) succeeded after the last edit, a STALE or UNSUPPORTED verification claim becomes a warning instead of a block. CONTRADICTED always blocks.

These are not claims: negated, hedged or conditional sentences ("should pass", "如果网络正常…"), questions, unchecked to-do items, quoted text and code blocks, CI reports, and mixed results ("12 passed, 3 failed").

## Configuration

Optional. The first file found wins: `<cwd>/.receipt.toml`, then `~/.config/receipt/config.toml`. Unknown keys and kinds are rejected.

```toml
disable = ["deploy"]                 # switch claim kinds off
stale = "warn"                       # STALE warns instead of blocking (default "block")

[evidence]                           # your project's own runners, as regexes
test = ['just check', 'python3 tools/verify\.py']
build = ['\./build\.sh']
```

## Evaluation

All numbers below come from this repository's committed data and code, run on 2026-10-22. They are not tuned to look good.

### Labelled sets

- **real**: 240 claim-candidate sentences from this machine's Claude Code and Codex sessions (142 session files). Only the redacted sentence and normalised command metadata are stored ([eval/real.jsonl](eval/real.jsonl)). No raw transcript content is in the repo.
  - Sampled in two strata: 120 the detector hit and 120 that only hit a broad keyword net. "By stratum" rows weight each case by stratum size to estimate population numbers.
  - Kinds were labelled by hand and committed before tuning (commit `f9cb2e5`). Tuning only looked at the dev split.
- **synthetic**: 65 hand-written cases covering kinds and phrasings the local sample lacks ([eval/synthetic.jsonl](eval/synthetic.jsonl)).

```bash
cargo run --release --example eval -- eval/real.jsonl eval/synthetic.jsonl
```

| set | split | weighting | cases | claim detection P / R | gate (block) P / R |
|---|---|---|---|---|---|
| real | heldout | as sampled | 120 | 97.6% / 92.1% | 95.5% / 87.5% |
| real | heldout | by stratum | 120 | 98.0% / 86.2% | 97.5% / 92.9% |
| real | dev | as sampled | 120 | 97.1% / 94.3% | 96.0% / 88.9% |
| real | all | by stratum | 240 | 98.1% / 86.4% | 97.8% / 85.7% |
| synthetic | heldout | as sampled | 32 | 96.6% / 100.0% | 90.9% / 100.0% |
| synthetic | all | as sampled | 65 | 98.2% / 100.0% | 94.7% / 100.0% |

Before tuning (frozen labels, first detector), the real held-out split scored 81.4% / 64.0% on detection and 87.1% / 93.1% on the gate.

Read these numbers with the following caveats:

- **Gate labels are derived.** They apply the evidence rules to the hand-labelled kinds. So gate P/R measures how detection errors propagate, not whether a block was fair. Fairness is measured by the replay below.
- **Held-out is not independent of dev.** Both splits come from the same sessions; one long Codex research project dominates them. The held-out jump after tuning shows that shared phrasing as much as generalisation.
- **Same author.** The labeller wrote the detector, as with every other tool in this space.
- **Labels were not changed after the fact.** For example, "[报告]已提交" and "方案已提交" were labelled as commits. The replay review later showed no `git commit` behind them, so the detector now rejects them. The labels stay as written, and those cases count as misses.
- **Biased sample.** These sessions run with vibeguard's "verify before claiming" rule, so real lies are rare in them.

### Replay over real sessions

```bash
cargo run --release --example replay -- ~/.claude/projects/*/*.jsonl ~/.codex/sessions/*/*/*/*.jsonl
```

Every turn-final message of the 142 local sessions went through the hook logic. The blocked stops were then reviewed by hand.

| | count |
|---|---|
| stops (turn-final messages) | 1,920 |
| with at least one claim | 396 |
| would be blocked | **15** (0.8% of stops) |
| of those, defensible by hand review: a failed `npm publish`, a failed harness run, two "edited after the last check" | 4 |
| unclear | 1 |
| false blocks | 10 (0.5% of stops, 2.5% of claiming stops) |

The false blocks were:

- summaries relayed from other sessions (4);
- one resumed session's opening summary, counted 3 times;
- storyboard table text containing "BUILD PASSED";
- a markdown link label;
- a third party's commit;
- a sed-edited notes file classed as code;
- a "test entry reference check".

The first version of the rules blocked 225 of the same 1,920 stops. Most of those were projects verifying through their own scripts. The unrecognised-runner rule and the `[evidence]` config exist because of that.

### Live demo

Two real Claude Code 2.1.300 runs, with the hook added via `--settings`. In one the agent reworded its claim; in the other it ran `npm test` and reported the failure. The demo also caught a negation false positive, which is now fixed: [docs/demo.md](docs/demo.md).

## Compared with others

About 25 projects attack "the agent said done". Most change the workflow (wrappers, task frameworks, supervisors) or use an LLM judge. The closest deterministic transcript checker is [B0yko/proof-of-done](https://github.com/B0yko/proof-of-done): Python, English, Claude Code only, synthetic evaluation. receipt adds Chinese, Codex, a single binary, and evaluation on real sessions. The full survey is in [docs/research.md](docs/research.md).

## Limits

- Out of the box, receipt only knows common toolchains. Add your own runners under `[evidence]`.
- Subagent runs are not merged into the parent session. A parent relaying "tests pass" from a subagent is blocked; the subagent itself is checked on `SubagentStop`.
- A filtered re-run (`cargo test foo`) after a failed full run satisfies "all tests pass".
- receipt does not resist an adversarial agent: `echo cargo test` matches. It is built to catch the sincere "done" that nobody checked.

## License

MIT
