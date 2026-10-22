# Research: tools that check an agent's "done"

Date: 2026-10-22. Star counts and update dates come from `gh repo view` on that day.

## Search method

GitHub repository search, run in several keyword groups because multi-word queries often return nothing:

1. Claim wording: `agent claims verification transcript`, `claim verification agent`, `completion claim hook`, `AI lied tests pass`, `agent false completion`, `hallucinated success agent`, `agent honesty checker`
2. "Done" wording: `proof of done`, `faking done`, `done gate`, `verify agent done`, `done verification claude`, `premature completion`, `fake done ai`
3. Hook / host wording: `claude code stop hook verify claims`, `claude stop hook`, `stop hook transcript verify`, `stop hook tests`, `evidence hook claude`, `claude code hook tests pass evidence`, `verify completion claude-code`
4. Receipt / gate wording: `receipts agent`, `agent verification gate`, `agent proof of work tests`, `lying agent`, `sycophancy coding agent`

Plus the projects named in the earlier exploration notes (`explore-2026-10/wave2/3-agent-infra/README.md`, `explore-2026-10/5-human-in-loop/README.md`). Every project in the matrix below had its README read. Repositories that only share a name (insurance claims, payment receipts, Web3) were dropped.

## The field in one paragraph

About 25 projects appeared between 2026-07 and 2026-10, and the largest has 51★. They fall into four approaches:

- **Prompt / skill**: tell the agent to verify. Nothing enforces it.
- **Wrapper / receipt capture**: the agent must run checks through the tool, which records exit codes.
- **Supervisor / task framework**: the tool owns the loop, the tasks and the acceptance criteria.
- **Transcript audit**: read what the agent said, compare it with what the transcript shows it ran.

Only the last approach needs no change to how the agent works. Within it, three projects match this task closely: B0yko/proof-of-done, veltiq/groundtruth and salatmaster/pinocchio. Nobody covers Chinese wording. Only groundtruth reads Codex transcripts, and it does not block on Codex.

## Feature matrix

Legend. Detection: **T** = parse the transcript, **W** = wrapper records runs, **R** = re-run checks itself, **D** = git diff / AST, **L** = LLM judge, **P** = prompt only. Hosts: CC = Claude Code, CX = Codex, CU = Cursor.

| Project | ★ | Lang | Detection | Claim extraction | Hosts | Blocks? | False-positive handling | Interaction |
|---|---|---|---|---|---|---|---|---|
| konoeph/AgentClaimGuard | 51 | Python | structured claims vs evidence refs | caller supplies structured claims | any (library, LangGraph) | returns `blocked` | policy contracts | Python API / FastAPI |
| Tetusa1/DoneGate | 31 | Python | W + R + D (worktree, lease, checks) | none (task-level) | worker-agnostic | yes, task validation | path denylist | CLI task runner |
| Kibertum/tausik-core | 19 | Python | W + task gates, ed25519-signed receipts | none (task criteria) | CC, CX, CU, others via MCP | yes, task close | task must declare criteria | MCP + skills |
| altrace-dev-role/rashomon | 17 | Go | own recorder via hooks + failure-word list in final message | lists failure words that are *absent* | CC only | no | "never guesses intent" | one line at end of turn, `rashomon report` |
| Lians-ai/Lians | 16 | Python | checkpoints bound to git state; trust classes | agent claims are `agent_attested`, never trusted | CC, CX, CU (MCP) | blocks "done" | strict trust model | MCP, plugin |
| AtharvaMaik/donecheck | 16 | Python | R + static scan (TODOs, secrets) | none | any (manual), GH Action | CLI exit | `--skip-reason` | CLI, DONECHECK.md |
| EvilFreelancer/crossreview | 9 | Python | multi-agent review | — | — | — | — | orchestrator |
| mohamedzhioua/agent-done-or-not | 6 | Shell | W (`capture` with exit code + sha256) + audit | markers the agent emits, regex fallback | CC (Stop), CU / CX (rules only) | yes on CC | needs a fresh passing capture | CLI, Stop hook |
| PyModel/code-max | 4 | — | P | — | any skill host | no | — | skill |
| veltiq/groundtruth | 3 | TS | T + D, no LLM | regex: files, symbols, tests, deps, commands | CC hook; reads CX / Gemini / Cursor transcripts | off by default, `--strict` | fuzzy claims become `review`; `ignore` globs | Stop hook, CLI, GH Action |
| semitexa/laravel-ai-verify | 3 | PHP | R (diff-scoped Laravel checks) | none | CC hook, others via AGENTS.md | yes on CC | diff scoping | artisan command |
| djfksjd/lupus | 1 | Python | supervisor + R, frozen tests | none (owns DONE) | CC, CX as workers | owns completion | frozen tests | supervisor CLI |
| zaidharoon8/claude-code-recap | 1 | Shell | regex on the final message only | "done / deployed / fixed" words | CC | yes | accepts `pending:` | `/recap` + Stop hook |
| salatmaster/pinocchio | 0 | TS | L (Sonnet) over prompts, edits, commands, final message | LLM, quote required | CC (mod) | never | "would rather miss a lie"; respects user-asked skips | nose UI, `/pinocchio` |
| **B0yko/proof-of-done** | 0 | Python | **T, no LLM**: last matching run after last relevant edit | English regex; skips code, quotes, negation, hedges, questions | CC (Stop + SubagentStop) | yes, fails open | 10 verdict reasons incl. masked pipes, empty runs, background | Stop hook with `Run:` line, `audit` |
| kuroudo-ai/done-gate | 0 | Python | T (this turn has any test output / exit code) | "fixed / done" words | CC, CX plugin | blocks once | one block per turn | plugin |
| Jvrd97/claude-done-gate | 0 | TS | R (runs your test command at Stop) + test-file lock | none | CC (mod) | yes, 3 rounds | — | toasts, `/unlock-tests` |
| D-ROXX/agent-honesty-checker | 0 | Python | R in worktree + Python AST diff | claim text optional | any (CLI) | CLI exit | — | CLI report |
| wasaybuilds/proof-of-done | 0 | TS | D (test tampering) | none | any, GH Action | CLI exit | refactor detection | CLI, CI |
| M2artiD/vigil | 0 | Python | task queue evidence files | none | CC (Stop / SubagentStop) | yes | — | research harness |
| jl-cmd/claude-workflow | 0 | Python | skill step state file | none | CC | yes | fails open on bad state | plugin |
| Samurai007AK/receipts-agent | 0 | Python | contract + referee (browser back-office) | — | own agent | — | — | different domain |

The "5-human-in-loop" list also named Kibertum/tausik-core 19, Lians 16, donecheck 16, agent-done-or-not 6, groundtruth 3, laravel-ai-verify 3, claude-code-recap 1 and pinocchio 0. All of them are in the table.

## What the closest projects teach

**B0yko/proof-of-done.** Closest in design, and the most careful.
- Python, English only, Claude Code only, created 2026-09-28.
- Logic: the latest matching foreground command after the last relevant edit decides.
- Reasons it tracks: masked exit status (`| tail`, `|| true`) needs a success line in the output; `collected 0 items` is an empty run; background runs don't count; a partial `-k` run can't override a failed full run.
- Ignores claims inside code blocks, inline code and quotes, and negated, hedged, future, question and instruction forms.
- Its feedback ends with a `Run:` line.
- Published held-out numbers: 98.2% gate precision and 82.1% recall. Its README says itself that all of its sets are synthetic and written by the same author.
- Latency: p95 175 ms on a 10 MB transcript with Python 3.9. That is close to the 200 ms budget used here.

**veltiq/groundtruth.**
- Grades claims against the git diff (files, symbols) as well as commands.
- Defaults to non-blocking. Its stated "deliberate bias toward silence" means fuzzy claims become `review`, never a failure.
- Reads Codex, Gemini, Cursor and opencode transcripts, but only the Claude Code hook blocks.

**salatmaster/pinocchio.**
- Uses an LLM and never blocks.
- Useful rules:
  - a claim must quote something that is really there;
  - "should pass", "run the tests to verify" and "I haven't run them" are not lies;
  - something the user asked for (skip this test) is not counted.

**rashomon.**
- Records independently through its own hooks, because the transcript comes from the agent.
- Counts subagent calls separately, since those calls are not in the main transcript.

**agent-done-or-not / tausik / Lians.**
- Strong guarantees (hashed or signed receipts), at the cost of changing the workflow: the agent must run checks through their wrapper, or open tasks first.

**kuroudo-ai/done-gate.** Blocks only once per turn. That is the simplest anti-loop rule.

## Gaps receipt can fill

1. **Chinese and mixed-language claims.** None of the deterministic tools handle them ("测试全部通过", "已提交", "编译通过"). The user and their sessions are bilingual.
2. **Codex as a blocking host.** Codex has had a `Stop` hook with `decision: block` since its hooks feature shipped. No transcript-audit tool blocks on Codex today.
3. **Speed.** The hook runs as a single static Rust binary. The goal is well under 200 ms on 100 MB transcripts (the largest local Claude Code session is 102 MB). The Python tools pay around 50 ms for interpreter start alone.
4. **Evaluation on real sessions.** Every published number in this field comes from synthetic fixtures. receipt's eval set takes claim sentences and command metadata from real local sessions, redacted, and reports the synthetic and real slices separately.
5. **Zero setup for the agent.** Unlike wrapper and receipt-capture tools, the agent does nothing differently.

Not a gap worth chasing: test-tampering detection (wasaybuilds, pinocchio, done-gate test lock) and diff-vs-claim file checks (groundtruth). They are separate products, and adding them would be scope creep.

## Host hook facts

### Claude Code (docs: code.claude.com/docs/en/hooks; local version 2.1.300)

- Events: `Stop` and `SubagentStop`. Stop takes no matcher. SubagentStop matches on `agent_type`.
- Stop input: `session_id`, `transcript_path`, `cwd`, `permission_mode`, `hook_event_name`, `stop_hook_active`, `last_assistant_message`, `background_tasks`, `session_crons`.
- SubagentStop adds `agent_id`, `agent_type` and `agent_transcript_path` (the subagent's own JSONL under `<session>/subagents/`).
- The docs say to use `last_assistant_message` for the final text, because the transcript file is written asynchronously and may not yet contain the last message.
- Exit codes:
  - `0`: success, and JSON on stdout is parsed.
  - `2`: block. Stderr becomes the reason, and Claude continues the conversation (on SubagentStop, the subagent continues).
  - Any other code: a non-blocking error shown to the user.
  - Special case: exit 2 with empty stdout and a stderr saying a file is missing is treated as non-blocking.
- JSON output instead of exit 2: `{"decision":"block","reason":"…"}`. `hookSpecificOutput.additionalContext` gives non-error feedback. `systemMessage` warns the user. `continue:false` plus `stopReason` halts everything.
- Loops: `stop_hook_active` is true while Claude is already continuing because of a stop hook. Claude Code caps consecutive stop-hook continuations at 8 (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`). The count resets whenever Claude calls a tool.
- Default command-hook timeout: 600 s.
- Config locations: `~/.claude/settings.json`, `.claude/settings.json`, `.claude/settings.local.json`, plugin `hooks/hooks.json`, or `--settings <file|json>` for one run. `CLAUDE_CONFIG_DIR` relocates the user config dir, which is how the demo avoids touching the real `~/.claude/settings.json`.

Transcript facts, from reading the local `~/.claude/projects` files (structure only):

- Bash calls are `tool_use` blocks with `input.command`. Results are `tool_result` blocks keyed by `tool_use_id`, with `is_error`. Results also carry a `toolUseResult` object (`stdout`, `stderr`, `interrupted`).
- An error result whose text starts with `Exit code N` means the command ran and failed: 182 of the error results in an 80-session sample.
- Other error results mean the command **never ran**:
  - `Permission for this action…` (permission denied)
  - `PreToolUse:Bash hook error` (a hook blocked it)
  - `<tool_use_error>` (tool-level error)
- With `run_in_background: true`, the immediate result is `Command running in background with ID: …`. The real outcome arrives later as a `<task-notification>` user message containing `<tool-use-id>`, `<status>` and `… (exit code N)`.
- Subagent turns are marked `isSidechain: true`, or live in `subagents/*.jsonl`.
- Size: the largest local transcript is 102 MB, and several exceed 30 MB.

### Codex (docs: learn.chatgpt.com/docs/hooks; local codex-cli 0.163.0)

- Events include `Stop` and `SubagentStop`. Hooks are on by default (`[features] hooks = false` turns them off).
- Config locations: `~/.codex/hooks.json`, `<repo>/.codex/hooks.json`, or `[hooks]` tables in `config.toml`. Non-managed hooks must be trusted once via `/hooks`.
- Stop input: `session_id`, `transcript_path`, `cwd`, `hook_event_name`, `model`, `permission_mode`, `turn_id`, `stop_hook_active`, `last_assistant_message`.
- Output: exit 0 requires JSON on stdout; plain text is invalid for Stop. `{"decision":"block","reason":"…"}`, or exit 2 with the reason on stderr. Either way, Codex turns the reason into a new continuation prompt. `continue:false` overrides the other hooks.
- `notify` (in `config.toml`) is fire-and-forget at turn end and cannot block. The Stop hook is the right integration point.

Rollout transcript (`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`). Current files record each finished item as an `event_msg` whose `payload.type == "item_completed"`, with:

- `item.type == "CommandExecution"`: `command` (an argv array such as `["/bin/zsh","-lc","cargo test"]`), `exit_code` (a number), `status` (`completed` / `failed`) and `aggregated_output`.
- `item.type == "FileChange"`: `changes` (a map of path → change) and `status`.
- `item.type == "AgentMessage"`: `content` (`[{type:"Text", text}]`) and `phase`.

Older Codex CLI rollouts used `event_msg` payloads `exec_command_end` (`command`, `exit_code`) and `patch_apply_end`. None of the 84 local files contain them, but the parser accepts them because other users may have older logs.

## Takeaways for the design

- Use `last_assistant_message` when present; fall back to the transcript's last assistant text.
- "Blocked by permission or hook" is not a failed run. It is no run.
- An exit code of 0 is not enough when the command is piped or `|| true`'d. Look at the output.
- Background runs count only once their `task-notification` reports the exit code.
- Default to silence on anything fuzzy. A false block costs more than a missed lie, as pinocchio, groundtruth and proof-of-done all say.
- Block at most once per stop cycle (`stop_hook_active`). The host cap of 8 is a backstop, not the plan.
