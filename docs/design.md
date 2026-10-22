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
| `test` | all tests pass, 42 passed, `cargo test` passes / 测试全部通过, 单测通过, 用例都过了 | `cargo test`, `cargo nextest`, `pytest`, `go test`, `bun test`, `npm/pnpm/yarn test`, `vitest`, `jest`, `mocha`, `make test`, `just test`, `mvn test`, `gradle test`, `swift test`, `xcodebuild test`, `rspec`, `phpunit`, `deno test`, `tox`, `nox` | `test result: FAILED`, `^FAILED`, `[1-9]\d* (failed\|failing)`, `--- FAIL`, `^FAIL\b`, `could not compile`, `error[E…]`, `collected 0 items`, `no tests ran`, `No tests found` |
| `build` | build succeeds, compiles cleanly / 编译通过, 构建成功 | `cargo build\|check`, `go build`, `tsc`, `bun/npm/pnpm run build`, `make`, `xcodebuild`, `gradle build`, `mvn package\|compile`, `swift build`, `vite build`, `next build`, `uv build` | `error[E…]`, `^error(:\|\[)`, `could not compile`, `BUILD FAILED`, `Build failed`, `npm ERR!` |
| `lint` | clippy is clean, lint passes, no warnings / lint 通过, clippy 无警告, 零警告 | `clippy`, `eslint`, `ruff check`, `golangci-lint`, `biome`, `go vet`, `shellcheck`, `*run lint`, `swiftlint`, `rubocop`, `flake8`, `pylint` | `generated [1-9]\d* warnings?`, `^error`, `[1-9]\d* problems?`, `Found [1-9]\d* errors?` |
| `typecheck` | type check passes, no type errors, tsc is clean / 类型检查通过 | `tsc`, `mypy`, `pyright`, `cargo check`, `vue-tsc`, `*run typecheck` | `Found [1-9]\d* errors?`, `error TS\d+`, `^error` |
| `format` | formatted with rustfmt, fmt check passes / 已格式化, 格式检查通过 | `cargo fmt`, `rustfmt`, `prettier`, `black`, `ruff format`, `gofmt`, `biome format`, `*run format\|fmt` | `Diff in`, `would reformat`, `[1-9]\d* files? would be` |

**Action kinds** never go stale. One successful run anywhere in the session is enough.

| Kind | Example claims | Evidence |
|---|---|---|
| `commit` | committed, created a commit / 已提交 (unless followed by PR/issue/评论), 已 commit | `git commit` |
| `push` | pushed to origin / 已推送, 已 push, 推上去了 | `git push` |
| `pr` | opened a PR, PR created / 已提交 PR, 已创建 PR, PR 已开 | `gh pr create`, MCP tool `*create_pull_request*` |
| `deploy` | deployed to staging, published to npm / 已部署, 已发布, 已上线 | `vercel`, `fly deploy`, `wrangler deploy\|publish`, `kubectl apply\|rollout`, `helm upgrade\|install`, `terraform apply`, `docker push`, `npm/bun/cargo publish`, `twine upload`, `uv publish`, `gh release create`, `netlify deploy`, `firebase deploy`, `gcloud … deploy`, `railway up` |

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

What counts as an edit:

- Claude `Edit`, `Write`, `MultiEdit` and `NotebookEdit`
- Codex `FileChange`
- Bash `sed -i`, `perl -i`, `apply_patch`, `git apply` and `patch`

Edits whose paths all end in `.md`, `.markdown`, `.txt`, `.rst` or `.adoc` are doc-only and do not make a run stale. The earlier exploration suggested making STALE only a warning. With doc edits excluded, a code edit after the last green run really does make "tests pass" unverified, so the default blocks. A config option turns STALE into a warning.

Subagent transcripts (`subagents/*.jsonl`) are not merged into the main session. A main agent relaying a subagent's test run would be flagged. Each subagent is checked against its own transcript on `SubagentStop`. This is a known recall/precision trade-off, listed under limits.

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

### Mixed language

English and Chinese regexes both run on every clause. The `\b` boundaries are only used on the English side. Chinese text has no word boundaries, so the Chinese side uses fixed phrases ("测试…通过") with up to 8 filler characters (`[^，。,.]{0,8}`) between the subject and the verb. That covers "测试已全部通过" and "cargo test 全部通过".

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
- The next stop in the same cycle (`stop_hook_active == true`): never block again. If claims are still unsupported, print `{"systemMessage": "receipt: still unverified: test, commit"}` on stdout and exit 0. The user sees it, and the agent stops. This is how the hook avoids nagging while staying visible.
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
- **Line prefilter.** Only lines containing `"tool_use"`, `"tool_result"`, `task-notification`, `item_completed`, `exec_command_end` or `patch_apply_end` are JSON-parsed. Assistant-text lines are read only when the hook input has no `last_assistant_message`.
- **No allocation of outputs beyond need.** For each result, only the failure-marker scan runs over the output. Output text is not stored.
- **Measurement.** Results go in the README, using `hyperfine`-style repeated runs on the largest local transcript, time measured by the binary.

## Evaluation

Two sets, never merged into one headline number:

1. **real**: candidate sentences from this machine's Claude Code and Codex sessions.
   - Candidates are sentences in final assistant messages that match a broad keyword net, which is wider than the detector.
   - Each candidate comes with redacted command metadata: normalised command (program and subcommand only, e.g. `cargo test`, `git commit`), outcome, failure-marker flag, background flag, and edits as `code`/`doc`.
   - Redaction: paths become `<path>`, URLs `<url>`, emails `<email>`, token-like strings `<secret>`, and the local user name is removed. No raw transcript lines are stored.
   - Labels are written by hand before tuning and committed. The set is split dev/held-out by id parity; tuning looks only at dev.
2. **synthetic**: hand-written cases for kinds and phrasings the local sample lacks (deploy, pr, Chinese hedges, masked pipes, background runs).

Scored separately for each set:

- **Detection** P/R: is this sentence a completion claim of kind K?
- **Gate** P/R: should this message be blocked? Positive class = block.

Bias note: the local sessions run with vibeguard's "verify before claiming" rule, so unsupported claims are rarer than in the wild. That is why the synthetic set exists.

## Limits

- Regex detection has a recall ceiling. Unusual phrasings ("everything's green on my side") are missed by design.
- Subagent evidence is not merged into the parent session.
- A filtered re-run (`cargo test foo`) after a failed full run counts as passing for "all tests pass".
- Transcript formats change. Parsing is kept tolerant (unknown records are skipped), and unit fixtures pin the formats seen on 2026-10-22.
