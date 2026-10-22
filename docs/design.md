# receipt design

## One line

When a coding agent's turn ends, receipt reads the final message. If the message says "tests pass", "committed", "构建成功" or similar, receipt looks in the session transcript for a matching command that really succeeded. If there is none, it stops the agent once and tells it to either run the check or reword the claim.

## Scope

In scope:

- Hosts: Claude Code `Stop` / `SubagentStop`, and Codex `Stop`
- A CLI that audits saved transcripts
- Bilingual claim detection (English and Chinese)

Out of scope: test-tampering detection, diff-vs-claim file checks, LLM judging, signed receipts, CI integration. See `research.md` for why.

Non-goal: resisting an adversarial agent. `echo "cargo test"` matches the test rule. receipt catches the common failure, which is an agent that sincerely says something it never checked. It does not guard against an agent trying to game the gate.

## Pipeline

```
hook stdin JSON ─▶ final text (last_assistant_message, or last assistant text in transcript)
                   │
transcript JSONL ─▶ steps: Run{cmd, outcome, output_flags, background} | Edit{doc_only}
                   │
claims(text) ──────┴─▶ for each claim: evidence rule over steps ─▶ verdict
                                                                   │
                                     block (exit 2 + stderr) / warn (systemMessage) / allow
```

## Claim kinds and evidence rules

The kinds below are a closed set. Matching a command means a regex search over the command string. For MCP and other non-Bash tools, the tool name is matched.

**Verification kinds** go stale. A matching run only counts if no code edit happened after it.

| Kind | Example claims (EN / ZH) | Evidence command pattern | Failure-in-output markers |
|---|---|---|---|
| `test` | all tests pass, 42 passed, `cargo test` passes / 测试全部通过, 单测通过, 用例都过了 | `cargo test`, `cargo nextest`, `pytest`, `go test`, `bun test`, `npm/pnpm/yarn test`, `vitest`, `jest`, `mocha`, `make test`, `just test`, `mvn test`, `gradle test`, `swift test`, `xcodebuild test`, `rspec`, `phpunit`, `deno test`, `tox`, `nox`, `ctest`, `unittest`, `rustc --test`. Also a project's own runner: an interpreter or shell executing a `test`/`verify` script (`python3 tools/verify.py`, `godot --script res://tests/x.gd`, `./scripts/test.sh`) | `test result: FAILED`, `^FAILED`, `[1-9]\d* (failed\|failing)`, `--- FAIL`, `^FAIL\b`, `could not compile`, `error[E…]`, `collected 0 items`, `no tests ran`, `No tests found` |
| `build` | build succeeds, compiles cleanly / 编译通过, 构建成功, release 测试编译通过 | `cargo build\|check\|test\|clippy\|run\|bench`, `go build\|test\|vet\|run`, `tsc`, `bun/npm/pnpm run build`, `make`, `cmake --build`, `xcodebuild`, `gradle build`, `mvn package\|compile`, `swift build`, `vite build`, `next build`, `uv build`. Test, lint and run commands compile the code too | `error[E…]`, `^error(:\|\[)`, `could not compile`, `BUILD FAILED`, `Build failed`, `npm ERR!` |
| `lint` | clippy is clean, lint passes, no warnings / lint 通过, clippy 无警告, 零警告 | `clippy`, `eslint`, `ruff check`, `golangci-lint`, `biome`, `go vet`, `shellcheck`, `*run lint`, `swiftlint`, `rubocop`, `flake8`, `pylint` | `generated [1-9]\d* warnings?`, `^error`, `[1-9]\d* problems?`, `Found [1-9]\d* errors?` |
| `typecheck` | type check passes, no type errors, tsc is clean / 类型检查通过 | `tsc`, `mypy`, `pyright`, `cargo check`, `vue-tsc`, `*run typecheck` | `Found [1-9]\d* errors?`, `error TS\d+`, `^error` |
| `format` | formatted with rustfmt, fmt check passes / 已格式化, 格式检查通过 | `cargo fmt`, `rustfmt`, `prettier`, `black`, `ruff format`, `gofmt`, `biome format`, `*run format\|fmt` | `Diff in`, `would reformat`, `[1-9]\d* files? would be` |

**Action kinds** never go stale. One successful run anywhere in the session is enough.

| Kind | Example claims | Evidence |
|---|---|---|
| `commit` | committed, created a commit / 已提交, 已 commit. Not when the clause hands in something else (方案/计划/报告/排期已提交, 已提交 PR/评论), unless a commit hash follows | `git commit` |
| `push` | pushed to origin / 已推送, 已 push, 推上去了 | `git push` |
| `pr` | opened a PR, PR created / 已提交 PR, 已创建 PR, PR 已开 | `gh pr create`, MCP tool `*create_pull_request*` |
| `deploy` | deployed to staging, published to npm / 已部署, 已发布, 已上线. UNSUPPORTED only warns: real deploys mostly run from CI, ssh or a dashboard | `vercel`, `fly deploy`, `wrangler deploy\|publish`, `kubectl apply\|rollout`, `helm upgrade\|install`, `terraform apply`, `docker push`, `npm/bun/cargo publish`, `twine upload`, `uv publish`, `gh release create`, `netlify deploy`, `firebase deploy`, `gcloud … deploy`, `railway up` |

Projects can add evidence patterns for their own commands, such as `./ci.sh` or `just check`. See the config section.

## Outcome of one run

| Observed | Outcome |
|---|---|
| Claude `tool_result` without `is_error` / Codex `exit_code == 0` | `ok` |
| `is_error` + text starting `Exit code N` / Codex `exit_code != 0` | `failed` |
| `is_error` + `Permission…`, `…hook error`, `<tool_use_error>`, user rejection | `not_run`: ignored, as if it never happened |
| Result says `running in background with ID` | `pending` until a `<task-notification>` with the same tool-use id reports `(exit code N)`; no notification ⇒ stays `pending` |
| No result at all (session cut off) | `no_result` |
| Outcome `ok` but the output shows a failure marker for the kind | `failed` (this catches `cargo test \| tail`, `\|\| true`) |

Only the failure markers of the claim's kind apply, so a `cargo build` warning cannot fail a commit claim.

## Verdict per claim

Let the candidates be the matching runs with an outcome other than `not_run`. The last candidate decides:

| Condition | Verdict | Default action |
|---|---|---|
| No candidate | `UNSUPPORTED` (`no_command`) | block |
| Last candidate `failed` | `CONTRADICTED` | block |
| Last candidate `pending` or `no_result` | `UNSUPPORTED` (`no_result`) | block |
| Last candidate `ok`, verification kind, a code edit after it | `STALE` | block |
| Otherwise | `SUPPORTED` | allow |

Before matching, the command text is normalised:

- Segments that only read or print are dropped (`rg 'a|cargo test'`, `cat`, `grep`, `ps`, `git log/diff/status`). The split on `&&`, `||`, `;`, `|` and newline respects quotes.
- A heredoc written to a file (`cat > run.sh <<EOF`) is dropped.
- Quotes, commas and brackets become spaces, so a harness script's `subprocess.run(["cargo", "test"])` reads as `cargo test`.

What counts as an edit:

- Claude `Edit`, `Write`, `MultiEdit` and `NotebookEdit`
- Codex `FileChange`
- Bash `sed -i`, `perl -i`, `apply_patch`, `git apply` and `patch`

An edit is non-code, and does not make a run stale, when every path it names ends in `.md`, `.markdown`, `.txt`, `.rst` or `.adoc`, or lies under a scratch directory (`/tmp/`, `/private/tmp/`, `/var/folders/`). For Bash edits, the paths are the file-like tokens of the command. The earlier exploration suggested making STALE only a warning. With doc edits excluded, a code edit after the last green run really does make "tests pass" unverified, so the default blocks. A config option turns STALE into a warning.

**Unrecognised runners.** Replaying 1,920 real stops showed that most STALE and UNSUPPORTED verdicts on verification claims came from projects that verify through their own runners (Python harness scripts, built binaries, game engines). receipt cannot see those. So a STALE or UNSUPPORTED verdict on a verification kind is downgraded from block to warn when a successful command ran after the last code edit and that command:

- matches no evidence rule;
- is not read-only;
- is not housekeeping (`git`, `gh`, `cd`, `cp`, `mv`, `rm`, `mkdir`, `ps`, `kill` and similar).

The detail then names that step, so the user can see what may have done the verifying. CONTRADICTED is never downgraded, and neither is "nothing ran at all after the edit".

**Relayed results.** Subagent transcripts (`subagents/*.jsonl`) are not merged into the main session; each subagent is checked against its own transcript on `SubagentStop`. Instead, the parser notes whether the session received reports from other agents: `Agent`/`Task`/`SendMessage` calls, MCP tools about sessions, threads or agents, Codex's multi-agent tools (`spawn_agent`, `send_message`, …), and `<cross-session-message>` / `<side-sessions-event>` deliveries. In such a session an UNSUPPORTED claim (nothing of its own ran) warns instead of blocking, because the agent is most likely passing on another agent's result. A failed or stale run of its own still blocks.

The first attempt matched the claim's words verbatim against earlier tool outputs. On the replay it caught none of the real relays, because agents paraphrase or translate what they relay, so it was removed.

## Recognising claims

### Text preparation

1. Remove fenced code blocks (```` ``` ````…```` ``` ````). Inline code keeps its content and loses the backticks, so `` `cargo test` passes `` is still a claim.
2. Remove double-quoted spans: `"…"`, `“…”`, `「…」`. Quoting a log line or the user is not a claim.
3. Split into sentences on newline, `. `, `!`, `?`, `。`, `！`, `？`, `；` and `;`.
4. Split each sentence into clauses on `,` `，` and the conjunctions `but`, `however`, `although`, `while`, `但`, `但是`, `不过`, `而`, `只是`, `可是`.

Step 4 fixes the prototype's known miss. The prototype judged negation over the whole sentence, so the clause "nothing is deployed" negated the earlier clause "All 166 collector tests pass". After the clause split, each claim is judged only with the words around it.

### Detection

Each kind has one claim regex for English and one for Chinese. A clause can produce more than one kind ("build and tests pass" → `build`, `test`). For each kind, the first clause that matches wins. A claim match is discarded if its clause shows any of the following.

**Negation**, meaning a negation word in the 4 words (English) or 6 characters (Chinese) right before the match, or in the match itself:

- EN: `not`, `no`, `never`, `nothing`, `none`, `n't`, `without`, `cannot`, `failed to`, `yet to`
- ZH: `未`, `没`, `没有`, `尚未`, `不`, `无`, `并未`, `别`

Chinese post-negation is also checked right after the match: `不了`, `不过` (as in 通过不了).

**Hedge, condition or instruction**, anywhere in the clause before the match:

- EN: `if`, `once`, `should`, `would`, `will`, `may`, `might`, `could`, `expect`, `likely`, `probably`, `hopefully`, `to verify`, `to confirm`, `to check`, `make sure`, `ensure`, `need(s) to`, `please`, `you can`, `try`, `let me`, `I'll`, `going to`, `when`, `unless`, `whether`, `assuming`, `except`
- ZH: `如果`, `若`, `假如`, `一旦`, `应该`, `应当`, `可能`, `估计`, `预计`, `将`, `会`, `待`, `等`, `需要`, `请`, `可以`, `是否`, `确认`, `验证`, `除了`

**Question**: the sentence ends with `?` or `？`, or contains `吗`.

The bias is toward silence. A fuzzy sentence is not a claim. "Should pass now" is a promise, not a receipt. pinocchio, groundtruth and proof-of-done all draw the same line.

**CI reports**: a clause that mentions CI (`CI`, `GitHub Actions`, `workflow run`, `流水线`, `构建日志`) makes no verification claim. A remote run is not this session's receipt.

**English lists**: a negation carries over to later list items that start with `or` / `nor` / `and`. In "I haven't run the tests, or made a commit", no commit is claimed.

### Mixed language

The English claim regexes run on every clause; `\b` boundaries only appear on the English side.

Chinese verification claims (test, build, lint, typecheck, format) are matched differently, because real Chinese summaries enumerate several checks before one shared predicate: "格式、严格 Clippy、release 测试编译均通过". The matcher works like this:

1. Find each predicate (`通过`, `成功`, `完成`, `无警告`, …, optionally after `均`/`全部`/`都`).
2. Take the subject before it (after the last `：`) and split it into items on `、和及与或/`.
3. Classify each item by keyword, in the order build → typecheck → lint → format → test. 测试编译 / 测试二进制构建 / libtest 构建 are builds, not test results.
4. Check that the item's kind accepts that predicate. 测试完成 is not 测试通过.

The whole enumeration is skipped in any of these cases:

- the subject contains a negation (不等于编译、测试通过);
- the subject contains a hedge;
- the predicate is followed by a rate or a general statement (通过率, 通过不代表…, 通过后).

Items naming a check but not a result (测试名核验, 测试注册检查, 格式解析, 构建准备) are skipped.

Chinese action claims (commit, push, PR, deploy) stay as regexes.

## Feedback to the agent

The feedback follows the house guard-message format (observation, a bounded FIX with an escape, and DO NOT). It names each claim, the command and step that decide it, and the exact command to run:

```
receipt: 2 claims in your final message have no passing run in this session.
1. [test] "All tests pass" — CONTRADICTED: the last `cargo test --all` (step 14) failed (exit 101).
2. [commit] "已提交" — UNSUPPORTED: no `git commit` ran in this session.
FIX: for each item, either run that check once in the foreground and report its real result, or reword only that sentence to say what was actually verified (for example "tests not run").
If a check fails, report the failure. Fix it only if fixing it is part of the task you were given.
DO NOT: edit tests or config to make a check pass, add new checks or scripts, or re-run checks that are not listed above.
```

Design choices:

- **One list, numbered, at most 5 items.** Keep it short; a sixth claim is summarised as "+N more".
- **Both exits are offered (run it, or reword).** Rewording is a legitimate outcome. The tool exists to make the message honest, not to force work the user may not want.
- **"Only that sentence" and "not listed above"** bound the scope. Without them, agents re-run the whole CI or rewrite the summary.
- **Run in the foreground.** A background run stays `pending`, which would block again.

### Loop control

- First stop with missing receipts: block with exit 2 and the feedback on stderr. Claude Code and Codex both accept this.
- The next stop in the same cycle (`stop_hook_active == true`): never block again. If claims are still unsupported, print `{"systemMessage": "receipt: still unverified: test (STALE), commit (UNSUPPORTED)"}` on stdout and exit 0. The user sees it, and the agent stops. This is how the hook avoids nagging while staying visible.
- Findings that only warn (a STALE downgraded by an unrecognised runner, an unsupported deploy, `stale = "warn"`) print `receipt: unverified: …` the same way, without blocking.
- Internal error (unreadable transcript, bad JSON): print a one-line `receipt: …` on stderr and exit 1. Both hosts show this as a non-blocking hook error. receipt never blocks on its own failure, and never passes silently either.

## Config and allowlist

Optional TOML, first found wins: `<cwd>/.receipt.toml`, then `$XDG_CONFIG_HOME/receipt/config.toml` (default `~/.config/receipt/config.toml`).

```toml
# Turn kinds off entirely (allowlist by kind).
disable = ["deploy"]

# "block" (default) or "warn": STALE becomes a systemMessage instead of a block.
stale = "block"

# Extra evidence regexes per kind, for project-specific commands.
[evidence]
test = ['just check', '\./scripts/ci\.sh']
deploy = ['make release']
```

Session-level escape: setting the environment variable `RECEIPT_OFF=1` makes the hook exit 0 immediately.

Rejected (overdesign for v0.1): per-path ignore globs, per-claim ignore phrases, custom claim regexes. Add them only if eval data shows a need.

## Codex specifics

- Transcript format is detected per line. A line with `payload` is a Codex rollout record; one with `message` is Claude Code.
- Runs come from `item_completed` `CommandExecution` items: `command` is an argv array, and the last element is the shell script when argv[0] is a shell. `exit_code` is a number. `aggregated_output` feeds the failure markers. Edits come from `FileChange` items, whose `changes` keys are the paths. The legacy events `exec_command_end` and `patch_apply_end` are read the same way.
- Final text comes from `last_assistant_message`, else the last `AgentMessage`.
- Codex reads Stop hook output the same way (exit 2 + stderr continues the turn). `systemMessage` is a common output field there too.

## Performance budget

The hook must finish within 200 ms (p95) on the largest real transcript (102 MB).

- **Fast path.** If the final text contains no claim keyword (a single prefilter regex), exit before opening the transcript. This is most stops.
- **Line prefilter.** Background-run notifications arrive in `queue-operation` and `attachment` records; their tags are read straight from the raw line. Only lines containing `"assistant"`, `"tool_result"`, `task-notification`, `item_completed`, `exec_command_end` or `patch_apply_end` are JSON-parsed. Assistant-text lines are read only when the hook input has no `last_assistant_message`.
- **No allocation of outputs beyond need.** For each result, only the failure-marker scan runs over the output. Output text is not stored.
- **Measured** (20 runs each, M-series Mac, release build):

| Transcript | With claim, p95 | No claim, p95 |
|---|---|---|
| 102.7 MB (largest) | 73.5 ms | 12.6 ms |
| 14.4 MB (p90) | 22.6 ms | 12.9 ms |
| 2.9 MB (median) | 19.1 ms | 12.5 ms |

The roughly 12 ms floor is process start plus regex compilation.

## Evaluation

Two sets, never merged into one headline number:

1. **real**: candidate sentences from this machine's Claude Code and Codex sessions. Each run in the metadata is stored as the evidence-rule match text only (e.g. `cargo test`), plus whether it was a multi-line or looped script.
   - Candidates are sentences in final assistant messages that match a broad keyword net, which is wider than the detector.
   - Each candidate comes with redacted command metadata: normalised command (program and subcommand only, e.g. `cargo test`, `git commit`), outcome, failure-marker flag, background flag, and edits as `code`/`doc`.
   - Redaction: paths become `<path>`, URLs `<url>`, emails `<email>`, token-like strings `<secret>`, and the local user name is removed. No raw transcript lines are stored.
   - Labels are written by hand before tuning and committed. The set is split dev/held-out by id parity; tuning looks only at dev.
2. **synthetic**: hand-written cases for kinds and phrasings the local sample lacks (deploy, pr, Chinese hedges, masked pipes, background runs).

Scored separately for each set:

- **Detection** P/R: is this sentence a completion claim of kind K?
- **Gate** P/R: should this message be blocked? Positive class = block.

Bias note: the local sessions run with vibeguard's "verify before claiming" rule, so unsupported claims are rarer than in the wild. That is why the synthetic set exists.

The gate label is derived, not judged. It applies the evidence rules to the hand-labelled kinds. Gate P/R therefore measures how detection errors propagate, not whether the agent was honest. Whether a block is fair is measured separately by **replay**:

- `receipt audit` runs every turn-final message of the local sessions through the hook logic;
- the blocked stops are then reviewed by hand. Results are in the README.

## Limits

- Out of the box, receipt only knows common toolchains. Projects with their own runners should add them under `[evidence]`. Otherwise their claims get warnings, or blocks when nothing at all ran after the last edit.
- A relayed summary in a session with no visible agent reports (for example a paraphrase of a file the agent read) has no receipt in this transcript and is blocked.

- Regex detection has a recall ceiling. Unusual phrasings ("everything's green on my side") are missed by design.
- Subagent evidence is not merged into the parent session.
- A filtered re-run (`cargo test foo`) after a failed full run counts as passing for "all tests pass".
- Transcript formats change. Parsing is kept tolerant (unknown records are skipped), and unit fixtures pin the formats seen on 2026-10-22.
