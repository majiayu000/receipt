use receipt::{Checker, Config};

fn kinds(text: &str) -> Vec<&'static str> {
    let c = Checker::new(&Config::default()).unwrap();
    let mut k: Vec<&str> = c.claims(text).into_iter().map(|c| c.kind).collect();
    k.sort();
    k
}

#[test]
fn english_claims() {
    assert_eq!(kinds("Fixed the parser. All tests pass and the build succeeds."), ["build", "test"]);
    assert_eq!(kinds("Done: 42 tests passed, clippy is clean, committed as 1a2b3c4d."), ["commit", "lint", "test"]);
    assert_eq!(kinds("`cargo test` passes and I pushed to origin."), ["push", "test"]);
    assert_eq!(kinds("Opened a PR and deployed to staging."), ["deploy", "pr"]);
}

#[test]
fn chinese_claims() {
    assert_eq!(kinds("修好了，测试全部通过，已提交。"), ["commit", "test"]);
    assert_eq!(kinds("编译通过，clippy 无警告，已推送到远程。"), ["build", "lint", "push"]);
    assert_eq!(kinds("cargo test 全部通过，已创建 PR。"), ["pr", "test"]);
}

#[test]
fn negation_is_local_to_the_claim() {
    // The prototype's known miss: "nothing" in the second clause hid the test claim.
    assert_eq!(kinds("All 166 collector tests pass, but nothing is deployed."), ["test"]);
    assert!(kinds("Nothing is committed.").is_empty());
    assert!(kinds("I haven't pushed yet.").is_empty());
    assert!(kinds("测试未通过，还没有提交。").is_empty());
    assert!(kinds("Tests are not passing yet.").is_empty());
}

#[test]
fn hedges_conditions_and_questions_are_not_claims() {
    assert!(kinds("Tests should pass now.").is_empty());
    assert!(kinds("Run `cargo test` to confirm the tests pass.").is_empty());
    assert!(kinds("If you rerun it, the tests pass.").is_empty());
    assert!(kinds("Do the tests pass?").is_empty());
    assert!(kinds("测试应该能通过。").is_empty());
    assert!(kinds("如果网络正常，构建会成功。").is_empty());
    assert!(kinds("测试通过了吗？").is_empty());
    assert!(kinds("- [ ] confirm all tests pass").is_empty());
}

#[test]
fn mixed_reports_and_lookalikes_are_not_claims() {
    assert!(kinds("12 passed, 2 failed.").is_empty());
    assert!(kinds("测试通过率 80%").is_empty());
    assert!(!kinds("已提交 PR").contains(&"commit"));
    assert!(kinds("There are uncommitted changes.").is_empty());
    assert!(kinds("They pushed back on the design.").is_empty());
}

#[test]
fn code_and_quotes_are_ignored() {
    assert!(kinds("The log prints \"All tests pass\" at the end.").is_empty());
    assert!(kinds("Example output:\n```\ntest result: ok. All tests pass\n```").is_empty());
}

#[test]
fn disabled_kinds_are_skipped() {
    let cfg: Config = toml::from_str("disable = [\"commit\"]").unwrap();
    let c = Checker::new(&cfg).unwrap();
    let k: Vec<&str> = c.claims("Tests pass, committed.").into_iter().map(|c| c.kind).collect();
    assert_eq!(k, ["test"]);
}

#[test]
fn bad_config_is_rejected() {
    let dir = std::env::temp_dir().join(format!("receipt-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".receipt.toml"), "disable = [\"tset\"]").unwrap();
    let err = Config::load(Some(&dir)).unwrap_err();
    assert!(format!("{err:#}").contains("unknown claim kind `tset`"), "{err:#}");
    std::fs::remove_dir_all(&dir).unwrap();
}
