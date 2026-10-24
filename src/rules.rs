//! The closed set of claim kinds: how each is worded, and what counts as its receipt.

/// One claim kind. All patterns are regex source; `docs/design.md` explains each table row.
pub struct Kind {
    pub name: &'static str,
    /// Verification kinds go stale when code is edited after the passing run.
    pub stale: bool,
    pub claim: &'static str,
    /// Text right after the claim match that cancels it (e.g. 已提交 PR is not a commit).
    pub unless_after: Option<&'static str>,
    /// Text earlier in the clause that cancels it (方案已提交 is a plan handed in, not a commit).
    pub unless_before: Option<&'static str>,
    /// A sentence matching this is a mixed result report, not a claim of this kind.
    pub veto: Option<&'static str>,
    /// Chinese verification claims: an enumerated subject item of this kind ("格式、Clippy、测试编译")
    /// and the predicates that complete it ("均通过"). See `claims::zh_items`.
    pub zh: Option<(&'static str, &'static str)>,
    pub evidence: &'static str,
    /// A run whose output matches this failed, whatever its exit status says.
    pub fail_output: Option<&'static str>,
    /// UNSUPPORTED only warns: this kind's real evidence is usually outside the transcript (deploys
    /// run from CI, ssh or a dashboard). CONTRADICTED still blocks.
    pub unsupported_warns: bool,
}

// Chinese patterns allow up to 8 filler characters between subject and verb (测试已全部通过).
pub const KINDS: &[Kind] = &[
    Kind {
        name: "test",
        stale: true,
        claim: concat!(
            r"(?i)\b(all\s+)?(the\s+)?(\d+\s+)?(new\s+|existing\s+|unit\s+|integration\s+|e2e\s+|regression\s+)?",
            r"(tests?|specs?|test\s+suites?)\s+(now\s+|still\s+|all\s+|both\s+)*",
            r"(pass(es|ed)?|are\s+(all\s+)?(passing|green)|is\s+(passing|green)|succeed(s|ed)?|went\s+green|ran\s+green)\b",
            r"|\b\d+\s+(tests?\s+)?passed\b",
            r"|\b(cargo\s+(test|nextest)|pytest|go\s+test|bun\s+test|npm\s+test|vitest|jest|make\s+test)\b[^.,;]{0,20}?\b(pass(es|ed)?|is\s+green|succeed(s|ed)?)\b",
        ),
        unless_after: Some(r"^\s*率"),
        unless_before: None,
        veto: Some(r"(?i)\b[1-9]\d*\s+(failed|failing|failures?)\b|[1-9]\d*\s*个?(失败|未通过)"),
        zh: Some((
            r"(?i)测试|单测|用例|回归|\btests?\b|cargo test|pytest",
            r"通过|全过|全绿|跑通|绿了|都过了",
        )),
        evidence: concat!(
            r"\b(cargo\s+(\+\S+\s+)?(test|nextest)|pytest|py\.test|go\s+test|bun\s+(run\s+)?test|deno\s+test|",
            r"(npm|pnpm|yarn)\s+(run\s+)?test|vitest|jest|mocha|make\s+(\S+\s+)?test|just\s+test|",
            r"mvn\s+(\S+\s+)?test|gradlew?\s+(\S+\s+)?test|swift\s+test|xcodebuild\s+.*\btest\b|rspec|phpunit|tox|nox|ctest|unittest|rustc\s+.*--test)\b",
            // A project's own runner: an interpreter or shell executing a test/verify script.
            r"|(^|[;&|\n]\s*)(\S*/)?(python3?|bash|sh|zsh|node|bun|deno|ruby|godot|uv\s+run)\s[^;&|\n]*?(\btests?/|\btest_\w+|\w_test\b|\bverify\w*\.|\brun_tests)",
            r"|(^|[;&|\n]\s*)\.{0,2}/\S*(test|verify)\w*\.sh\b",
        ),
        fail_output: Some(concat!(
            // "1 failed," / "1 failed;" / "1 failed in 0.2s", not "passed 34 failed 0".
            r"(?m)test result: FAILED|^FAILED\b|\b[1-9]\d* (failed|failing)([,;)|]|\s*$|\s+in\b)|^--- FAIL|^FAIL\b|could not compile|",
            r"error\[E\d{4}\]|collected 0 items|no tests ran|No tests found",
        )),
        unsupported_warns: false,
    },
    Kind {
        name: "build",
        stale: true,
        claim: concat!(
            r"(?i)\b(the\s+)?(release\s+|debug\s+)?(build|builds|compilation)\s+(now\s+|still\s+)?",
            r"(succeed(s|ed)?|pass(es|ed)?|is\s+(clean|green|successful|passing)|was\s+successful|completed?\s+successfully)\b",
            r"|\bcompiles?\s+(cleanly|successfully|fine|without\s+(errors|warnings))\b",
            r"|\bbuilds?\s+(cleanly|successfully|fine)\b",
            r"|\b(cargo\s+(build|check)|go\s+build|npm\s+run\s+build|bun\s+run\s+build)\b[^.,;]{0,20}?\b(pass(es|ed)?|succeed(s|ed)?|is\s+clean)\b",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: Some((
            r"(?i)测试(编译|构建|二进制)|libtest|编译|构建|重建|\bbuild\b|cargo check",
            r"通过|成功|完成|退出\s*0|无报错|没有报错",
        )),
        evidence: concat!(
            // Test, lint and run commands compile the code too.
            r"\b(cargo\s+(\+\S+\s+)?(build|check|b|test|nextest|clippy|run|bench)|go\s+(build|test|vet|run)|tsc|(npm|pnpm|yarn|bun)\s+(run\s+)?build|make|",
            r"xcodebuild|gradlew?\s+(\S+\s+)?(build|assemble)|mvn\s+(\S+\s+)?(package|compile|install)|swift\s+build|",
            r"vite\s+build|next\s+build|uv\s+build|zig\s+build|cmake\s+--build|just\s+build)\b",
        ),
        fail_output: Some(
            r"(?m)error\[E\d{4}\]|^error(:|\[)|could not compile|BUILD FAILED|Build failed|npm ERR!|error TS\d+",
        ),
        unsupported_warns: false,
    },
    Kind {
        name: "lint",
        stale: true,
        claim: concat!(
            r"(?i)\b(clippy|lint|linting|linter|eslint|ruff(\s+check)?|golangci-lint|biome|shellcheck|go\s+vet)\b[^.,;]{0,20}?",
            r"\b(is\s+|are\s+)?(clean|pass(es|ed)?|green|happy|no\s+(warnings|issues|errors)|0\s+warnings)\b",
            r"|\b(zero|no)\s+(clippy|lint)\s+(warnings|errors|issues)\b",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: Some((
            r"(?i)clippy|lint|eslint|ruff|静态检查|代码检查",
            r"通过|干净|无警告|零警告|没有警告|退出\s*0",
        )),
        evidence: concat!(
            r"\b(clippy|eslint|ruff\s+check|ruff|golangci-lint|biome\s+(check|lint|ci)|go\s+vet|shellcheck|swiftlint|",
            r"rubocop|flake8|pylint|(npm|pnpm|yarn|bun)\s+(run\s+)?lint|just\s+lint|make\s+lint|oxlint)\b",
        ),
        fail_output: Some(
            r"(?m)generated [1-9]\d* warnings?|^error|\b[1-9]\d* problems?\b|Found [1-9]\d* errors?",
        ),
        unsupported_warns: false,
    },
    Kind {
        name: "typecheck",
        stale: true,
        claim: concat!(
            r"(?i)\btype[\s-]?check(s|ing|er)?\s+(now\s+)?(pass(es|ed)?|is\s+clean|clean|succeed(s|ed)?)\b",
            r"|\bno\s+type\s+errors\b",
            r"|\b(tsc|mypy|pyright|vue-tsc)\b[^.,;]{0,20}?\b(pass(es|ed)?|is\s+clean|clean|reports?\s+no\s+errors|0\s+errors)\b",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: Some((
            r"(?i)类型检查|\btsc\b|mypy|pyright",
            r"通过|无错误|没有错误|干净",
        )),
        evidence: r"\b(tsc|mypy|pyright|cargo\s+(check|clippy)|vue-tsc|(npm|pnpm|yarn|bun)\s+(run\s+)?(typecheck|type-check|tsc)|go\s+vet)\b",
        fail_output: Some(r"(?m)Found [1-9]\d* errors?|error TS\d+|^error|\b[1-9]\d* errors?\b"),
        unsupported_warns: false,
    },
    Kind {
        name: "format",
        stale: true,
        claim: concat!(
            r"(?i)\b(cargo\s+fmt|rustfmt|prettier|black\s|gofmt|ruff\s+format|fmt\s+check|format\s+check|formatting)\b[^.,;]{0,20}?",
            r"\b(pass(es|ed)?|is\s+clean|clean)\b",
            r"|\bformatted\s+(with|using|by)\s+\w+",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: Some((
            r"(?i)格式(检查|化|复查)?$|格式检查|格式化|格式复查|\bfmt\b|rustfmt|prettier",
            r"通过|完成|干净|退出\s*0",
        )),
        evidence: r"\b(cargo\s+fmt|rustfmt|prettier|black|ruff\s+format|gofmt|goimports|biome\s+format|(npm|pnpm|yarn|bun)\s+(run\s+)?(format|fmt)|swift-format|dprint)\b",
        fail_output: Some(
            r"(?m)^Diff in|would reformat|[1-9]\d* files? would be|Code style issues found",
        ),
        unsupported_warns: false,
    },
    Kind {
        name: "commit",
        stale: false,
        claim: concat!(
            r"(?i)\bcommitted\b|\bcommit(ted)?\s+as\s+`?[0-9a-f]{7,}|\b(created|made)\s+(a\s+|the\s+|one\s+)?(new\s+)?commit\b",
            r"|已(经)?\s*(提交|commit)|提交了|(代码|改动|修改|更改|变更)已(经)?提交",
        ),
        unless_after: Some(
            r"(?i)^\s*(了\s*)?(一个\s*|下一轮\s*)?(PR|pull|issue|评论|申请|反馈|工单|报告|审核|审批|表单|作业|排期|最小方案|方案|计划|设计)",
        ),
        unless_before: Some(
            r"方案|计划|设计|报告|排期|申请|表单|作业|评论|反馈|工单|审核|请求|任务|答卷|材料",
        ),
        veto: None,
        zh: None,
        evidence: r"\bgit\s+(-C\s+\S+\s+|-c\s+\S+\s+)*commit\b",
        fail_output: None,
        unsupported_warns: false,
    },
    Kind {
        name: "push",
        stale: false,
        claim: r"(?i)\bpushed\b|已(经)?\s*(推送|push)|推送(到|至)\s*(远程|远端|origin|github|GitHub)|推上去了|已(经)?推到",
        unless_after: Some(r"(?i)^\s*back"),
        unless_before: None,
        veto: None,
        zh: None,
        evidence: r"\bgit\s+(-C\s+\S+\s+|-c\s+\S+\s+)*push\b|\bgh\s+repo\s+create\b.*\s--push\b",
        fail_output: None,
        unsupported_warns: false,
    },
    Kind {
        name: "pr",
        stale: false,
        claim: concat!(
            r"(?i)\b(opened|created|submitted|filed|raised)\s+(a\s+|the\s+|an\s+)?(draft\s+)?(PR|pull\s+request)\b",
            r"|\b(PR|pull\s+request)\s+(is\s+|has\s+been\s+|was\s+)?(opened|created|up|submitted)\b",
            r"|(已|已经)\s*(提交|创建|开|发起|提)(了)?\s*(一个\s*)?(PR|pr|pull request|合并请求)",
            r"|(提交|创建|开|发起|提)了\s*(一个\s*)?(PR|pr|pull request|合并请求)|(PR|pr)\s*已(经)?(提交|创建|开|发起)",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: None,
        evidence: r"\bgh\s+pr\s+create\b|create_pull_request|\bgh\s+api\b.*\bpulls\b",
        fail_output: None,
        unsupported_warns: false,
    },
    Kind {
        name: "deploy",
        stale: false,
        claim: concat!(
            r"(?i)\bdeployed\s+(to|on|at|it|the|successfully)\b|\b(has\s+been|was|is|been)\s+deployed\b",
            r"|\bshipped\s+to\s+(prod|production|staging)\b|\bpublished\s+(to|on)\s+(npm|crates\.io|pypi|the\s+registry)\b",
            r"|\breleased\s+v?\d+\.\d+",
            r"|(已|已经)\s*(部署|发布|上线)|部署(成功|完成)|发布(成功|完成)|成功(部署|发布|上线)",
        ),
        unless_after: None,
        unless_before: None,
        veto: None,
        zh: None,
        evidence: concat!(
            r"\b(vercel|fly\s+deploy|flyctl\s+deploy|wrangler\s+(deploy|publish|pages\s+deploy)|kubectl\s+(apply|rollout)|",
            r"helm\s+(upgrade|install)|terraform\s+apply|docker\s+push|(npm|bun|pnpm|yarn|cargo)\s+publish|twine\s+upload|",
            r"uv\s+publish|gh\s+release\s+create|netlify\s+deploy|firebase\s+deploy|gcloud\s+.*deploy|railway\s+up|serverless\s+deploy)\b",
        ),
        fail_output: None,
        unsupported_warns: true,
    },
];

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name == name)
}
