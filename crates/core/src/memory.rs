//! Long-term memory: the pure rules shared by the server and the runner.
//!
//! Agents save a memory with `HULAHOOP_MEMORY: <scope> <kind> <title> — <body>`
//! in their turn output. Everything here is free of I/O so it can be tested alone.

use std::collections::HashSet;

/// Memories one run may save.
pub const MEMORY_WRITES_PER_RUN: usize = 3;
/// The prompt index never costs more than this many estimated tokens.
pub const MEMORY_INDEX_BUDGET_TOKENS: usize = 1500;
pub const MEMORY_TITLE_MAX: usize = 120;
pub const MEMORY_BODY_MAX: usize = 600;
/// A memory unused for this many runs falls to the bottom of the index.
pub const MEMORY_STALE_RUNS: i64 = 40;
/// A memory unused for this many runs is archived.
pub const MEMORY_ARCHIVE_RUNS: i64 = 200;

const MARKER: &str = "HULAHOOP_MEMORY:";
const KINDS: [&str; 5] = ["decision", "convention", "preference", "fact", "gotcha"];
const SCOPES: [&str; 4] = ["workspace", "project", "person", "agent"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLine {
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub body: String,
}

/// Every `HULAHOOP_MEMORY:` line in `text`, in order. A line that is malformed
/// is an `Err` with the reason, so the caller can say why it was dropped.
pub fn parse_lines(text: &str) -> Vec<Result<MemoryLine, String>> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix(MARKER))
        .map(|rest| parse_line(rest.trim()))
        .collect()
}

fn parse_line(rest: &str) -> Result<MemoryLine, String> {
    let mut words = rest.splitn(3, char::is_whitespace);
    let scope = words.next().unwrap_or_default();
    let kind = words.next().unwrap_or_default();
    let tail = words.next().unwrap_or_default();
    let base = scope.split(':').next().unwrap_or_default();
    let named = scope.split_once(':').map(|(_, name)| name);
    if !SCOPES.contains(&base) || (named.is_some() && base != "agent" && base != "person") {
        return Err(format!("unknown scope `{scope}`"));
    }
    if named == Some("") {
        return Err(format!("scope `{scope}` names nobody"));
    }
    if !KINDS.contains(&kind) {
        return Err(format!("unknown kind `{kind}`"));
    }
    let (title, body) = tail
        .split_once('—')
        .ok_or("a memory needs `title — body`")?;
    let (title, body) = (title.trim(), body.trim());
    if title.is_empty() || body.is_empty() {
        return Err("a memory needs a title and a body".into());
    }
    if title.chars().count() > MEMORY_TITLE_MAX {
        return Err(format!("title is over {MEMORY_TITLE_MAX} characters"));
    }
    if body.chars().count() > MEMORY_BODY_MAX {
        return Err(format!("body is over {MEMORY_BODY_MAX} characters"));
    }
    Ok(MemoryLine {
        scope: scope.into(),
        kind: kind.into(),
        title: title.into(),
        body: body.into(),
    })
}

/// Lowercase alphanumeric words, in order.
pub fn normalize_tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Word-set overlap (Jaccard) from 0 to 1, for spotting near-duplicates.
pub fn similarity(a: &str, b: &str) -> f64 {
    let a: HashSet<String> = normalize_tokens(a).into_iter().collect();
    let b: HashSet<String> = normalize_tokens(b).into_iter().collect();
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// An FTS5 `OR` query of quoted words, or `None` when there is nothing to
/// search for. Quoting keeps user text from being read as FTS syntax.
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = normalize_tokens(text)
        .into_iter()
        .filter(|word| !matches!(word.as_str(), "or" | "and" | "not" | "near"))
        .map(|word| format!("\"{word}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// Rough token count: four characters to a token.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count() / 4
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexCandidate {
    pub seq: i64,
    pub scope: String,
    pub kind: String,
    pub title: String,
    pub stale: bool,
    /// Position among the full-text matches for the task, best first.
    pub match_rank: Option<i64>,
    pub use_count: i64,
    pub updated_at: i64,
}

const INDEX_HEADER: &str = "Memory index (open .hulahoop/memory/<file> for the full note):\n";

fn index_line(candidate: &IndexCandidate) -> String {
    format!(
        "- [{} {}] {} ({})\n",
        candidate.scope,
        candidate.kind,
        candidate.title,
        note_file_name(candidate.seq, &candidate.title)
    )
}

/// The candidates that fit `budget` tokens, most relevant first: fresh before
/// stale, then matches, then most used, then newest.
pub fn select_index(mut candidates: Vec<IndexCandidate>, budget: usize) -> Vec<IndexCandidate> {
    candidates.sort_by_key(|c| {
        (
            c.stale,
            c.match_rank.is_none(),
            c.match_rank,
            std::cmp::Reverse(c.use_count),
            std::cmp::Reverse(c.updated_at),
        )
    });
    let mut used = estimate_tokens(INDEX_HEADER);
    let mut chosen = Vec::new();
    for candidate in candidates {
        let cost = estimate_tokens(&index_line(&candidate)).max(1);
        if used + cost > budget {
            break;
        }
        used += cost;
        chosen.push(candidate);
    }
    chosen
}

/// The index text for the prompt; empty when nothing was chosen.
pub fn render_index(chosen: &[IndexCandidate]) -> String {
    if chosen.is_empty() {
        return String::new();
    }
    let lines: String = chosen.iter().map(index_line).collect();
    format!("{INDEX_HEADER}{lines}")
}

/// A file name for a note: `<seq>-<slug>.md`, only `[a-z0-9-]`.
pub fn note_file_name(seq: i64, title: &str) -> String {
    let slug: String = crate::slugify(title).chars().take(40).collect();
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        format!("{seq}.md")
    } else {
        format!("{seq}-{slug}.md")
    }
}
