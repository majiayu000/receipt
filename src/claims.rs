//! Find completion claims ("tests pass", "已提交") in an agent's final message.
//!
//! Bias toward silence: a negated, hedged, conditional or questioning clause is not a claim.

use crate::rules::Kind;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Claim {
    pub kind: &'static str,
    /// The clause the claim was found in, trimmed.
    pub text: String,
}

struct Compiled {
    kind: &'static Kind,
    claim: Regex,
    unless: Option<Regex>,
    unless_before: Option<Regex>,
    veto: Option<Regex>,
    zh: Option<(Regex, Regex)>,
}

pub struct Detector {
    kinds: Vec<Compiled>,
    /// Indices into `kinds` in the order an enumerated Chinese item is classified:
    /// 测试编译 is a build, so build comes before test.
    zh_order: Vec<usize>,
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?(```|$)").unwrap());
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""[^"\n]{0,200}"|“[^”\n]{0,200}”|「[^」\n]{0,200}」"#).unwrap());
static CLAUSE_SPLIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[,，]|\s(but|however|although|though|while|whereas)\s|但是|但|不过|可是|只是|而且|而",
    )
    .unwrap()
});
static UNCHECKED_TODO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*([-*]\s*)?\[ \]").unwrap());
// Words in the few words right before the claim that negate it.
static NEG_EN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(not|no|never|nothing|none|without|cannot|yet\s+to|failed\s+to|neither|nor)\b|n't\b").unwrap()
});
static NEG_ZH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"未|没|尚未|不|无|并未|别").unwrap());
// Positive phrases that contain a negation character (没有报错 = "no errors").
static NEG_ZH_OK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"没有报错|无报错|无警告|没有警告|零警告|无错误|没有错误").unwrap()
});
static POST_NEG_ZH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(不了|不过|不到)").unwrap());
// Hedges, conditions and instructions anywhere before the claim in its clause.
static HEDGE_EN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)\b(if|once|should|would|will|may|might|could|expect(ed|s)?|likely|probably|hopefully|presumably|",
        r"to\s+(verify|confirm|check|ensure|make\s+sure)|make\s+sure|ensure|needs?\s+to|please|you\s+can|try|let\s+me|",
        r"going\s+to|unless|whether|assuming|except|until|wait\s+for)\b|\b(I'll|we'll|it'll|they'll)"
    ))
    .unwrap()
});
static HEDGE_ZH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"如果|若|假如|一旦|只要|应该|应当|可能|估计|预计|预期|将会|会|待|需要|请|你可以|是否|(以|来|去|再)(确认|验证)|除了|之后再|等.{0,12}后|比如|例如|譬如|还要|必须|须|要求|只等|等待|等着").unwrap()
});
// Any Chinese verification predicate, with an optional quantifier (均通过, 全部通过).
static ZH_PRED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(均|全部|都|也|已经|已|全)?\s*(通过|全过|全绿|跑通|绿了|都过了|成功|完成|干净|退出\s*0|无警告|零警告|没有警告|无报错|没有报错|无错误|没有错误)").unwrap()
});
static ZH_ITEM_SPLIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"、|和|及|与|以及|或|／|/|\+").unwrap());
// Enumerated items that name a check but not a claim kind's result (测试名核验, 测试注册检查).
static ZH_ITEM_SKIP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"核验|注册|方法|计划|重点|名单|解析|准备|脚本|说明").unwrap());
// What follows the predicate makes it a rate or a general statement (通过率, 编译通过不代表…).
static ZH_NOT_RESULT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(率|只算|只是|不代表|不等于|并不|不能|后|之后|以后)").unwrap());
const ZH_PRIORITY: &[&str] = &["build", "typecheck", "lint", "format", "test"];

// Conditional openers anywhere earlier in the sentence ("If you rerun it, tests pass").
static COND_SENTENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*([-*]\s*)?(\*\*)?(if|once|after\s+you|unless|when\s+you|next|todo|plan)\b|^\s*([-*]\s*)?(\*\*)?(如果|若|假如|一旦|只要|等|下一步|接下来|计划|路线|待办)").unwrap()
});
static LIST_CONTINUES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(or|nor|and)\b").unwrap());
static HASH_AFTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[（(:：为]?\s*`?[0-9a-f]{7,40}\b").unwrap());
// Results of a remote CI run: this session's transcript cannot back them.
static CI_REPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bCI\b|GitHub Actions|workflow run|流水线|构建日志").unwrap()
});

impl Detector {
    pub fn new(kinds: &[&'static Kind]) -> Self {
        let kinds: Vec<Compiled> = kinds
            .iter()
            .map(|k| Compiled {
                kind: k,
                claim: Regex::new(k.claim)
                    .unwrap_or_else(|e| panic!("claim regex for {}: {e}", k.name)),
                unless: k.unless_after.map(|r| Regex::new(r).unwrap()),
                unless_before: k.unless_before.map(|r| Regex::new(r).unwrap()),
                veto: k.veto.map(|r| Regex::new(r).unwrap()),
                zh: k.zh.map(|(item, pred)| {
                    (
                        Regex::new(item).unwrap(),
                        Regex::new(&format!("^({pred})$")).unwrap(),
                    )
                }),
            })
            .collect();
        let zh_order = ZH_PRIORITY
            .iter()
            .filter_map(|n| kinds.iter().position(|c| c.kind.name == *n))
            .collect();
        Self { kinds, zh_order }
    }

    /// One claim per kind at most: the first clause that makes it.
    pub fn detect(&self, text: &str) -> Vec<Claim> {
        let mut out: Vec<Claim> = vec![];
        let mut push = |kind: &'static str, clause: &str| {
            if !out.iter().any(|c| c.kind == kind) {
                out.push(Claim {
                    kind,
                    text: clause.trim().chars().take(160).collect(),
                });
            }
        };
        for s in sentences(text) {
            if is_question(&s) || UNCHECKED_TODO.is_match(&s) || COND_SENTENCE.is_match(&s) {
                continue;
            }
            let mut negated_list = false;
            for clause in clauses(&s) {
                // "I haven't run the tests, or made a commit": the negation covers the later list items.
                if negated_list && LIST_CONTINUES.is_match(clause) {
                    continue;
                }
                negated_list |= NEG_EN.is_match(clause);
                let ci = CI_REPORT.is_match(clause);
                for c in &self.kinds {
                    if ci && c.kind.stale {
                        continue;
                    }
                    if c.veto.as_ref().is_some_and(|v| v.is_match(&s)) {
                        continue;
                    }
                    if self.regex_claim(c, clause) {
                        push(c.kind.name, clause);
                    }
                }
                for kind in if ci { vec![] } else { self.zh_items(clause) } {
                    if !self.kinds.iter().any(|c| {
                        c.kind.name == kind && c.veto.as_ref().is_some_and(|v| v.is_match(&s))
                    }) {
                        push(kind, clause);
                    }
                }
            }
        }
        out
    }

    fn regex_claim(&self, c: &Compiled, clause: &str) -> bool {
        c.claim.find_iter(clause).any(|m| {
            let (before, after) = (&clause[..m.start()], &clause[m.end()..]);
            !(c.unless.as_ref().is_some_and(|u| u.is_match(after))
                // A commit hash right after the claim settles it, whatever came before.
                || (c.unless_before.as_ref().is_some_and(|u| u.is_match(before)) && !HASH_AFTER.is_match(after))
                || negated(before, m.as_str(), after)
                || HEDGE_EN.is_match(before)
                // Chinese hedges can sit inside the match's filler gap.
                || HEDGE_ZH.is_match(&clause[..m.end()]))
        })
    }

    /// Chinese verification claims, where one predicate completes an enumerated subject:
    /// "格式、严格 Clippy、release 测试编译均通过" claims format, lint and build.
    fn zh_items(&self, clause: &str) -> Vec<&'static str> {
        let mut out = vec![];
        let mut start = 0;
        for m in ZH_PRED.find_iter(clause) {
            let pred = ZH_PRED
                .captures(m.as_str())
                .and_then(|c| c.get(2))
                .map_or("", |g| g.as_str());
            let mut subject = &clause[start..m.start()];
            start = m.end();
            // "私有副本构建完成：格式检查、Clippy 通过": the enumeration starts after the colon.
            if let Some(i) = subject.rfind(['：', ':']) {
                subject = &subject[i + subject[i..].chars().next().map_or(1, char::len_utf8)..];
            }
            let after = clause[m.end()..].trim_start();
            // A negation anywhere in the subject covers the whole enumeration (不等于编译、测试通过).
            let subject_negated = NEG_ZH.is_match(&NEG_ZH_OK.replace_all(subject, ""));
            if subject_negated
                || HEDGE_ZH.is_match(subject)
                || POST_NEG_ZH.is_match(after)
                || ZH_NOT_RESULT.is_match(after)
            {
                continue;
            }
            for item in ZH_ITEM_SPLIT.split(subject) {
                if item.trim().is_empty() || ZH_ITEM_SKIP.is_match(item) {
                    continue;
                }
                let kind = self.zh_order.iter().map(|&i| &self.kinds[i]).find_map(|c| {
                    let (item_re, pred_re) = c.zh.as_ref()?;
                    (item_re.is_match(item.trim()) && pred_re.is_match(pred)).then_some(c.kind.name)
                });
                out.extend(kind);
            }
        }
        out
    }
}

fn is_question(s: &str) -> bool {
    let t = s.trim_end();
    t.ends_with('?') || t.ends_with('？') || t.contains('吗')
}

fn negated(before: &str, matched: &str, after: &str) -> bool {
    let words: Vec<&str> = before.split_whitespace().collect();
    let tail_words = words[words.len().saturating_sub(4)..].join(" ");
    if NEG_EN.is_match(&tail_words) {
        return true;
    }
    let chars: Vec<char> = before.trim_end().chars().collect();
    let tail_chars: String = chars[chars.len().saturating_sub(6)..].iter().collect();
    let tail_chars = NEG_ZH_OK.replace_all(&tail_chars, "");
    if NEG_ZH.is_match(&tail_chars) {
        return true;
    }
    let matched = NEG_ZH_OK.replace_all(matched, "");
    // Only Chinese negation inside the match: "no warnings" is part of a positive English claim.
    NEG_ZH.is_match(&matched) || POST_NEG_ZH.is_match(after.trim_start())
}

/// Strip code blocks and quotes, then split into sentences.
pub fn sentences(text: &str) -> Vec<String> {
    let text = FENCE.replace_all(text, "\n");
    let text = QUOTED.replace_all(&text, " ");
    let text = text.replace('`', "");
    let mut out = vec![];
    for line in text.lines() {
        let mut cur = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            cur.push(c);
            let end = match c {
                '。' | '！' | '？' | '；' | ';' | '!' | '?' => true,
                // A period ends a sentence only before whitespace (keeps "v1.2" and "foo.rs" whole).
                '.' => chars.peek().is_none_or(|n| n.is_whitespace()),
                _ => false,
            };
            if end {
                push_sentence(&mut out, &mut cur);
            }
        }
        push_sentence(&mut out, &mut cur);
    }
    out
}

fn push_sentence(out: &mut Vec<String>, cur: &mut String) {
    let t = cur.trim();
    if !t.is_empty() {
        out.push(t.to_string());
    }
    cur.clear();
}

fn clauses(sentence: &str) -> Vec<&str> {
    CLAUSE_SPLIT
        .split(sentence)
        .filter(|c| !c.trim().is_empty())
        .collect()
}
