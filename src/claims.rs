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

pub struct Detector {
    kinds: Vec<(&'static Kind, Regex, Option<Regex>, Option<Regex>)>,
}

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)```.*?(```|$)").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"\n]{0,200}"|“[^”\n]{0,200}”|「[^」\n]{0,200}」"#).unwrap());
static CLAUSE_SPLIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[,，]|\s(but|however|although|though|while|whereas)\s|但是|但|不过|可是|只是|而且|而").unwrap()
});
static UNCHECKED_TODO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([-*]\s*)?\[ \]").unwrap());
// Words in the few words right before the claim that negate it.
static NEG_EN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(not|no|never|nothing|none|without|cannot|yet\s+to|failed\s+to|neither|nor)\b|n't\b").unwrap()
});
static NEG_ZH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"未|没|尚未|不|无|并未|别").unwrap());
// Positive phrases that contain a negation character (没有报错 = "no errors").
static NEG_ZH_OK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"没有报错|无报错|无警告|没有警告|零警告|无错误|没有错误").unwrap());
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
    Regex::new(r"如果|若|假如|一旦|只要|应该|应当|可能|估计|预计|将会|会|待|需要|请|你可以|是否|(以|来|去|再)(确认|验证)|除了|之后再|等.{0,12}后").unwrap()
});
// Conditional openers anywhere earlier in the sentence ("If you rerun it, tests pass").
static COND_SENTENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(if|once|after\s+you|unless|when\s+you)\b|^\s*(如果|若|假如|一旦|只要|等)").unwrap());

impl Detector {
    pub fn new(kinds: &[&'static Kind]) -> Self {
        let kinds = kinds
            .iter()
            .map(|k| {
                let claim = Regex::new(k.claim).unwrap_or_else(|e| panic!("claim regex for {}: {e}", k.name));
                let unless = k.unless_after.map(|r| Regex::new(r).unwrap());
                let veto = k.veto.map(|r| Regex::new(r).unwrap());
                (*k, claim, unless, veto)
            })
            .collect();
        Self { kinds }
    }

    /// One claim per kind at most: the first clause that makes it.
    pub fn detect(&self, text: &str) -> Vec<Claim> {
        let sentences = sentences(text);
        let mut out: Vec<Claim> = vec![];
        for (k, claim, unless, veto) in &self.kinds {
            'sentences: for s in &sentences {
                if is_question(s) || UNCHECKED_TODO.is_match(s) || COND_SENTENCE.is_match(s) {
                    continue;
                }
                if veto.as_ref().is_some_and(|v| v.is_match(s)) {
                    continue;
                }
                for clause in clauses(s) {
                    for m in claim.find_iter(clause) {
                        let after = &clause[m.end()..];
                        if unless.as_ref().is_some_and(|u| u.is_match(after)) {
                            continue;
                        }
                        if negated(&clause[..m.start()], m.as_str(), after) || HEDGE_EN.is_match(&clause[..m.start()]) {
                            continue;
                        }
                        // Chinese hedges can sit inside the match's filler gap (测试应该能通过).
                        if HEDGE_ZH.is_match(&clause[..m.end()]) {
                            continue;
                        }
                        out.push(Claim { kind: k.name, text: clause.trim().chars().take(160).collect() });
                        break 'sentences;
                    }
                }
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
    CLAUSE_SPLIT.split(sentence).filter(|c| !c.trim().is_empty()).collect()
}
