# receipt: stop AI coding agents from claiming "tests pass" without proof

[![CI](https://github.com/majiayu000/receipt/actions/workflows/ci.yml/badge.svg)](https://github.com/majiayu000/receipt/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/majiayu000/receipt)](https://github.com/majiayu000/receipt/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/rust-1.82%2B-orange.svg)
![No LLM calls](https://img.shields.io/badge/LLM%20calls-none-lightgrey.svg)

**receipt is a Claude Code hook and Codex Stop hook for agent verification.** When a coding agent ends its turn saying "tests pass", "build succeeds", "committed" or "已提交", receipt checks the session transcript for the command that proves it. If that command never ran, failed, or ran before the last code edit, receipt blocks the hallucinated completion once. The agent then either runs the check or rewords the claim.

- **Deterministic.** No LLM and no network. One Rust binary that reads only the transcript.
- **Claude Code and Codex.** `Stop` / `SubagentStop` on Claude Code, `Stop` on Codex. Both transcript formats are parsed.
- **English and Chinese claims**, including Chinese summaries that list several checks before one verb ("格式、严格 Clippy、release 测试编译均通过").
- **Fast.** p95 73 ms on a 102 MB transcript, about 12 ms when the final message makes no claim.
- **Measured on real sessions**, not only synthetic fixtures. See [Evaluation](#evaluation).

## What a block looks like

From a live Claude Code 2.1.300 session. Full transcripts are in [docs/demo.md](docs/demo.md).

```
ASSISTANT: All tests pass.

Stop hook feedback:
receipt: 1 claim in your final message has no passing run in this session.
1. [test] "All tests pass." — UNSUPPORTED: no test command ran in this session.
FIX: for each item, either run that check once in the foreground and report its real result, or reword only that sentence to say what was actually verified (for example "tests not run").
If a check fails, report the failure. Fix it only if fixing it is part of the task you were given.
DO NOT: edit tests or config to make a check pass, add new checks or scripts, or re-run checks that are not listed above.

TOOL_USE Bash: npm test   →   Exit code 1: FAIL sum(1, 2): expected 3, got -1

ASSISTANT: I was wrong earlier: the tests don't pass. I said "All tests pass." without running anything. …
```

## Quick start

```bash
# 1. Get the binary: a prebuilt release (macOS arm64 shown; also x86_64-apple-darwin, x86_64-unknown-linux-gnu)…
curl -fsSL https://github.com/majiayu000/receipt/releases/latest/download/receipt-aarch64-apple-darwin.tar.gz | tar xz
sudo mv receipt-aarch64-apple-darwin/receipt /usr/local/bin/
#    …or build it
cargo install --git https://github.com/majiayu000/receipt

# 2. Add the hook (merges into your settings, keeps a backup; --dry-run shows the result first)
receipt install                    # Claude Code: ~/.claude/settings.json
receipt install --codex            # Codex: ~/.codex/hooks.json, then run /hooks once to trust it
receipt install --project .        # this repository only (.claude/settings.json)

# 3. Optional: how often would it have stopped you? Replay your past sessions
receipt audit ~/.claude/projects/*/*.jsonl
```

<details>
<summary>Manual hook config</summary>

Claude Code (`~/.claude/settings.json` or `.claude/settings.json`):

```json
{
  "hooks": {
    "Stop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }],
    "SubagentStop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }]
  }
}
```

Codex (`~/.codex/hooks.json` or `<repo>/.codex/hooks.json`):

```json
{ "hooks": { "Stop": [{ "hooks": [{ "type": "command", "command": "receipt hook", "timeout": 10 }] }] } }
```

To try it for one run without touching your settings: `claude --settings ./receipt-settings.json`.
</details>

## How it decides

| Final message says | Receipt it looks for in the transcript |
|---|---|
| tests pass / 测试全部通过 | `cargo test`, `pytest`, `go test`, `npm/bun test`, `vitest`, `jest`, `ctest`, … or a `test`/`verify` script |
| build succeeds / 编译通过 | `cargo build/check/test/clippy`, `go build`, `tsc`, `npm run build`, `make`, `cmake --build`, … |
| lint clean / typecheck / formatted | `clippy`, `eslint`, `ruff`, `golangci-lint` / `tsc`, `mypy`, `pyright` / `cargo fmt`, `prettier`, `black`, … |
| committed / pushed / opened a PR | `git commit` / `git push` / `gh pr create` |
| deployed / published | `vercel`, `fly deploy`, `wrangler deploy`, `kubectl apply`, `npm publish`, `gh release create`, … |

The last matching run gives one of four verdicts:

- **SUPPORTED**: it passed, and no code was edited afterwards.
- **STALE**: it passed, then code was edited.
- **CONTRADICTED**: it failed. This includes exit 0 with failures in the output, as with `cargo test | tail` or `|| true`.
- **UNSUPPORTED**: it never ran.

Two refinements:

- Runs denied by permissions or by another hook count as not run. Background runs count only once their completion notice arrives.
- Edits to docs or scratch files don't make a run stale.

**Bias toward silence.** These are not claims:

- negated, hedged or conditional sentences ("should pass", "如果网络正常…"), questions and unchecked to-dos;
- quotes and code blocks;
- CI reports;
- mixed results ("12 passed, 3 failed").

STALE and UNSUPPORTED only warn, and do not block, in two cases:

- an unrecognised command (a harness script, a built binary) passed after the last edit;
- the session received other agents' reports (subagents, cross-session messages), so the claim may be relayed.

A failed run always blocks.

| Situation | Hook result |
|---|---|
| No claim, or every claim is SUPPORTED | exit 0. With no claim, the transcript is not even read |
| A claim is UNSUPPORTED, CONTRADICTED or STALE | exit 2 with the feedback above. Claude Code and Codex both continue the turn |
| Already blocked once in this stop cycle (`stop_hook_active`) | exit 0 plus a `systemMessage` for you: "receipt: still unverified: …" |
| receipt itself fails (unreadable transcript, bad input) | exit 1: a visible hook error, never a block |

Full rules: [docs/design.md](docs/design.md).

## Configuration

Optional. The first file found wins: `<repo>/.receipt.toml`, then `~/.config/receipt/config.toml`. Unknown keys and kinds are errors. To turn it off for one session, set `RECEIPT_OFF=1`.

```toml
disable = ["deploy"]                # switch claim kinds off
stale = "warn"                      # STALE warns instead of blocking (default "block")

[evidence]                          # your project's own runners, as regexes
test = ['just check', 'python3 tools/verify\.py']
build = ['\./build\.sh']
```

## Compared with similar tools

About 25 projects attack "the agent said it's done". Most change the workflow (wrappers, task frameworks) or use an LLM judge. The closest ones:

| | receipt | [proof-of-done](https://github.com/B0yko/proof-of-done) | [groundtruth](https://github.com/veltiq/groundtruth) | [rashomon](https://github.com/altrace-dev-role/rashomon) | [AgentClaimGuard](https://github.com/konoeph/AgentClaimGuard) |
|---|---|---|---|---|---|
| Checks claims against the transcript | ✅ | ✅ | ✅ (+ git diff) | own recorder | your structured claims |
| Blocks the stop | ✅ | ✅ | opt-in | ❌ | n/a (library) |
| Codex | ✅ blocks | ❌ | reads only | ❌ | ❌ |
| Chinese claims | ✅ | ❌ | ❌ | ❌ | ❌ |
| Evaluated on real sessions | ✅ | synthetic only | ❌ | ❌ | ❌ |
| Runtime | Rust binary | Python | Node | Go | Python |

Feature by feature, including where the others go further: [docs/comparison.md](docs/comparison.md).

## Evaluation

All data and scripts are in this repository (run on 2026-10-22).

**Labelled sets.**
- **real**: 240 claim-candidate sentences from 142 local Claude Code and Codex sessions. Only the redacted sentence and normalised command metadata are stored ([eval/real.jsonl](eval/real.jsonl)); there is no raw transcript content.
  - Kinds were labelled by hand and frozen before tuning (commit `f9cb2e5`). Tuning used only the dev split.
  - Sampling was split between detector hits and a broad keyword net. "By stratum" weights by stratum size.
- **synthetic**: 65 hand-written cases ([eval/synthetic.jsonl](eval/synthetic.jsonl)).

```bash
cargo run --release --example eval -- eval/real.jsonl eval/synthetic.jsonl
```

| set | split | weighting | cases | claim detection P / R | gate (block) P / R |
|---|---|---|---|---|---|
| real | held-out | as sampled | 120 | 97.6% / 92.1% | 95.5% / 87.5% |
| real | held-out | by stratum | 120 | 98.0% / 86.2% | 97.5% / 92.9% |
| real | all | by stratum | 240 | 98.1% / 86.4% | 97.8% / 85.7% |
| synthetic | all | as sampled | 65 | 98.2% / 100.0% | 94.7% / 100.0% |

Before tuning, real held-out was 81.4% / 64.0% (detection) and 87.1% / 93.1% (gate).

Caveats:
- Gate labels are derived: they apply the evidence rules to the hand-labelled claim kinds.
- Held-out and dev come from the same sessions.
- The labeller wrote the detector.
- These sessions ran with a "verify before claiming" rule, so real lies are rare in them.

**Replay over real sessions.** `receipt audit` ran on all 142 local sessions. Every blocked stop was then reviewed by hand.

| | count |
|---|---|
| stops (turn-final messages) | 1,939 |
| with at least one claim | 403 |
| would block | **14** (0.7% of stops) |
| …defensible: a failed `npm publish`, a failed harness run, two "edited after the last check" | 4 |
| …unclear | 1 |
| …false blocks | 9 (0.5% of stops) |
| would warn instead of block | 73 (2 of them as relays) |

The 9 false blocks:
- one resumed session's opening summary, counted 3×;
- two relays the rule misses: a third party's commit, and a paraphrased summary;
- storyboard table text containing "BUILD PASSED";
- a markdown link label;
- a sed-edited notes file classed as code;
- a "test entry reference check".

The first version of the rules blocked 225 of these stops. Most of those came from projects verifying through their own scripts, which is why the unrecognised-runner rule and `[evidence]` exist.

## Limits

- Out of the box, receipt only knows common toolchains. Add your own runners under `[evidence]`.
- A paraphrased relay ("fixed three bugs over there") in a session without visible agent reports is still blocked.
- A filtered re-run (`cargo test foo`) after a failed full run satisfies "all tests pass".
- receipt catches the sincere, unchecked "done". It does not resist an adversarial agent: `echo cargo test` matches.

## Docs

- [docs/design.md](docs/design.md): claim kinds, evidence rules, negation and hedging, feedback wording, performance
- [docs/research.md](docs/research.md): survey of similar tools and the Claude Code and Codex hook contracts
- [docs/comparison.md](docs/comparison.md): feature-by-feature comparison
- [docs/demo.md](docs/demo.md): live Claude Code demo transcripts

## License

MIT
