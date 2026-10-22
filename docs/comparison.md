# receipt compared with other "agent said done" checkers

Checked 2026-10-22. Stars, language, license and creation date come from the GitHub API on that day. Features come from each project's README on that day. "—" means the README does not claim it. The full survey (about 25 projects, with search terms) is in [research.md](research.md).

## At a glance

| | receipt | [proof-of-done](https://github.com/B0yko/proof-of-done) | [groundtruth](https://github.com/veltiq/groundtruth) | [rashomon](https://github.com/altrace-dev-role/rashomon) | [AgentClaimGuard](https://github.com/konoeph/AgentClaimGuard) | [agent-done-or-not](https://github.com/mohamedzhioua/agent-done-or-not) | [pinocchio](https://github.com/salatmaster/pinocchio) | [done-gate](https://github.com/kuroudo-ai/done-gate) |
|---|---|---|---|---|---|---|---|---|
| Stars | new | 0 | 3 | 17 | 51 | 6 | 0 | 0 |
| Language | Rust (one binary) | Python | TypeScript | Go | Python | Shell | TypeScript (Claude Code mod) | Python |
| License | MIT | Apache-2.0 | MIT | Apache-2.0 | Apache-2.0 | — | MIT | — |
| How it decides | transcript: claim vs. recorded runs | transcript: claim vs. recorded runs | transcript + git diff | own hook recorder; failure words in final message | caller-supplied structured claims and evidence | wrapper records runs; Stop gate | LLM judge (Sonnet) | transcript: any test output this turn |
| Needs an LLM | no | no | no | no | no | no | yes | no |
| Agent workflow unchanged | yes | yes | yes | yes | no (you build claim objects) | no (checks go through its wrapper) | yes | yes |
| Blocks the stop | yes, once per cycle | yes | opt-in `--strict` | no (one-line notice) | returns `blocked` to your code | yes | never | yes, once |
| Claude Code | Stop + SubagentStop | Stop + SubagentStop | Stop | yes | — | Stop | yes | yes |
| Codex | Stop hook (blocks) | — | reads transcripts, no block | — | — | AGENTS.md rule only | — | yes |
| Chinese claims | yes, incl. enumerations ("格式、Clippy、测试编译均通过") | — (English only) | — | — | — | — | "any language" (LLM) | — |
| Stale run (edited after the check) | yes | yes | — | — | — | needs a fresh capture | yes | — |
| Exit 0 but output says failed (`\| tail`, `\|\| true`) | yes | yes | — | — | — | capture keeps the real exit code | — | — |
| Background runs | waits for the completion notification | background runs don't count | — | — | — | — | — | — |
| Denied or hook-blocked command counts as "not run" | yes | — | — | records it | — | — | — | — |
| Relayed results (subagents, other sessions) | warns instead of blocking | SubagentStop on the subagent | — | shows subagent calls | — | SubagentStop audit | — | — |
| Project's own runners | `[evidence]` regexes; unknown runners after the edit → warn | `suggest` per rule | `ignore` globs | — | policy files | any command via `capture` | — | — |
| One-command setup | `receipt install [--codex]` | plugin install | `npx … setup` | plugin or installer | `pip install` | `npx … init` | plugin install | plugin or `install.py` |
| Audit past sessions | `receipt audit` | `proof-of-done audit` | `groundtruth stats` | `rashomon report` | — | `audit` | `/pinocchio` | — |
| Published accuracy | P/R on 240 real + 65 synthetic cases; replay of 1,939 real stops | P/R on synthetic sets (held-out 98.2% / 82.1%) | — | — | — | — | — | — |
| Hook latency | p95 73 ms on a 102 MB transcript | p95 175 ms on 10 MB (Python 3.9) | — | — | n/a | — | seconds (LLM call) | — |

## Where receipt is different

1. **Chinese and mixed-language summaries.** Real Chinese agent summaries list several checks before one shared verb. receipt splits that list and checks each item against its own evidence rule.
2. **Codex blocks too.** receipt reads both transcript formats (Claude Code JSONL and Codex rollouts). It answers Codex's Stop hook with exit 2, which Codex turns into a continuation prompt.
3. **Measured on real sessions.**
   - Replay of 1,939 real stops on the author's machine: 14 would be blocked, about 0.7% of stops. A hand review judged 4 of those defensible, 1 unclear and 9 false blocks.
   - The README lists every error class.
   - Other projects either publish no numbers or measure only synthetic fixtures (proof-of-done says so itself).
4. **Bias toward silence.** It warns instead of blocking when an unrecognised command ran after the last edit, or when the session is relaying other agents' reports. Failed runs always block.

## Where others go further

- **proof-of-done** resists more gaming: fake `echo "12 passed"`, a redefined `pytest`, `PATH` tricks. It also handles partial re-runs (`-k`, `--lf`) after a failed full run. receipt does not try to resist an adversarial agent.
- **groundtruth** checks claims about files and symbols against the git diff ("added `validateInput`"). receipt only checks claims about commands.
- **agent-done-or-not**, **tausik-core** and **Lians** give stronger guarantees: hashed or signed receipts bound to the commit. The price is that the agent must run checks through their wrapper or task system.
- **rashomon** records every tool call independently of the transcript. That includes subagents, and calls another hook rewrote.
- **pinocchio**, **wasaybuilds/proof-of-done** and **Jvrd97/claude-done-gate** catch test tampering (`it.skip`, weakened asserts). receipt does not look at diffs.
- **AgentClaimGuard** is a general claim/evidence policy library for any LLM app, not a coding-agent hook.
