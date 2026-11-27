//! Contract for `hulahoop_core::memory` (plan tracks B2/C). Pure functions only.
use hulahoop_core::memory::*;

fn line(s: &str) -> MemoryLine {
    let mut v = parse_lines(s);
    assert_eq!(v.len(), 1, "{s}");
    v.remove(0).expect("line should parse")
}

#[test]
fn constants_match_the_plan() {
    assert_eq!(MEMORY_WRITES_PER_RUN, 3);
    assert_eq!(MEMORY_INDEX_BUDGET_TOKENS, 1500);
    assert_eq!(MEMORY_TITLE_MAX, 120);
    assert_eq!(MEMORY_BODY_MAX, 600);
    assert_eq!(MEMORY_STALE_RUNS, 40);
    assert_eq!(MEMORY_ARCHIVE_RUNS, 200);
}

#[test]
fn parses_a_memory_line_without_any_person() {
    let m = line("done.\nHULAHOOP_MEMORY: project gotcha envrc is blocked — run commands via direnv exec\nHULAHOOP_VERDICT: done");
    assert_eq!(m.scope, "project");
    assert_eq!(m.kind, "gotcha");
    assert_eq!(m.title, "envrc is blocked");
    assert_eq!(m.body, "run commands via direnv exec");
}

#[test]
fn parses_agent_scopes_and_ignores_unrelated_lines() {
    assert_eq!(
        line("HULAHOOP_MEMORY: agent:Reviewer fact t — b").scope,
        "agent:Reviewer"
    );
    assert!(parse_lines("no memory here\nHULAHOOP_COMMIT: x").is_empty());
}

#[test]
fn drops_bad_scope_kind_and_oversize_lines() {
    assert!(parse_lines("HULAHOOP_MEMORY: galaxy fact t — b")[0].is_err());
    assert!(parse_lines("HULAHOOP_MEMORY: project rumour t — b")[0].is_err());
    assert!(parse_lines("HULAHOOP_MEMORY: project fact t")[0].is_err());
    let long_title = "x".repeat(MEMORY_TITLE_MAX + 1);
    assert!(parse_lines(&format!("HULAHOOP_MEMORY: project fact {long_title} — b"))[0].is_err());
    let long_body = "x".repeat(MEMORY_BODY_MAX + 1);
    assert!(parse_lines(&format!("HULAHOOP_MEMORY: project fact t — {long_body}"))[0].is_err());
}

#[test]
fn similarity_detects_near_duplicates() {
    let a = "Envrc is blocked in review";
    let b = "review: the envrc is blocked";
    assert!(similarity(a, b) >= 0.6);
    assert!(similarity(a, "prototype lives in the milestone zip") < 0.3);
    assert_eq!(similarity(a, a), 1.0);
}

#[test]
fn normalize_tokens_is_case_and_punctuation_insensitive() {
    assert_eq!(
        normalize_tokens("Envrc, IS blocked!"),
        normalize_tokens("envrc is blocked")
    );
}

#[test]
fn fts_query_is_safe_and_none_for_empty() {
    assert_eq!(fts_query("  ?! "), None);
    let q = fts_query("blocking \"envrc\" OR (x*)").unwrap();
    assert!(!q.contains('(') && !q.contains('*'));
}

#[test]
fn estimate_tokens_is_chars_over_four() {
    assert_eq!(estimate_tokens(&"a".repeat(400)), 100);
}

fn cand(seq: i64, title: &str) -> IndexCandidate {
    IndexCandidate {
        seq,
        scope: "project".into(),
        kind: "fact".into(),
        title: title.into(),
        stale: false,
        match_rank: None,
        use_count: 0,
        updated_at: seq,
    }
}

#[test]
fn index_never_exceeds_budget_header_included() {
    let cands: Vec<_> = (1..200)
        .map(|i| cand(i, &format!("note number {i} with a longish title")))
        .collect();
    for budget in [0usize, 20, 100, 1500] {
        let chosen = select_index(cands.clone(), budget);
        assert!(
            estimate_tokens(&render_index(&chosen)) <= budget || chosen.is_empty(),
            "budget {budget}"
        );
    }
    let chosen = select_index(cands, 100);
    assert!(!chosen.is_empty());
}

#[test]
fn index_order_is_fresh_then_match_then_used_then_newest() {
    let mut stale = cand(1, "stale");
    stale.stale = true;
    stale.match_rank = Some(0);
    let mut matched = cand(2, "matched");
    matched.match_rank = Some(0);
    let mut used = cand(3, "used");
    used.use_count = 9;
    let newest = cand(4, "newest");
    let order: Vec<i64> = select_index(vec![stale, newest, used, matched], 1500)
        .iter()
        .map(|c| c.seq)
        .collect();
    assert_eq!(order, vec![2, 3, 4, 1]);
}

#[test]
fn note_file_names_are_safe() {
    let n = note_file_name(7, "../../Etc/passwd: ÜBER !!");
    assert!(regex_like(&n), "{n}");
    assert!(n.starts_with("7-") || n.contains('7'));
}

fn regex_like(n: &str) -> bool {
    n.ends_with(".md")
        && n[..n.len() - 3]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && n.len() > 3
}
