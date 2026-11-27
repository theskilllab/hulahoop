//! Long-term memory: the pure rules shared by the server and the runner.
//!
//! Agents save a memory with `HULAHOOP_MEMORY: <scope> <kind> <title> — <body>`
//! in their turn output. Everything here is free of I/O so it can be tested alone.

use std::collections::HashSet;

/// Memories one run may save.
pub const MEMORY_WRITES_PER_RUN: usize = 3;
/// Word-set overlap (see [`similarity`]) at or above which a new memory in the
/// same scope updates the existing one instead of adding another. The server
/// uses this for dedupe; `fts_query` only ranks the prompt index.
pub const MEMORY_DEDUPE_MIN: f64 = 0.6;
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
/// Valid lines past `MEMORY_WRITES_PER_RUN` become an `Err` too, so the cap
/// cannot be bypassed by the caller.
pub fn parse_lines(text: &str) -> Vec<Result<MemoryLine, String>> {
    let mut saved = 0;
    text.lines()
        .filter_map(|line| line.trim().strip_prefix(MARKER))
        .map(|rest| {
            let line = parse_line(rest.trim())?;
            saved += 1;
            if saved > MEMORY_WRITES_PER_RUN {
                return Err(format!("over {MEMORY_WRITES_PER_RUN} memories in one run"));
            }
            Ok(line)
        })
        .collect()
}

/// `text` on one line: whitespace runs (newlines included) become one space and
/// other control characters are dropped.
pub fn one_line(text: &str) -> String {
    let kept: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Splits `title — body`; agents often write ` -- ` or ` - ` instead.
fn split_title_body(tail: &str) -> Option<(&str, &str)> {
    tail.split_once('—')
        .or_else(|| tail.split_once(" -- "))
        .or_else(|| tail.split_once(" - "))
}

fn parse_line(rest: &str) -> Result<MemoryLine, String> {
    let rest: String = rest.chars().filter(|c| !c.is_control()).collect();
    let rest = rest.as_str();
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
    let (title, body) = split_title_body(tail).ok_or("a memory needs `title — body`")?;
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

/// Connective words that say nothing about a subject, so two notes sharing only
/// these are not duplicates.
const STOP_WORDS: [&str; 24] = [
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "in", "is", "it", "of", "on", "or",
    "that", "the", "this", "to", "was", "when", "with", "not", "do",
];

fn content_words(text: &str) -> HashSet<String> {
    normalize_tokens(text)
        .into_iter()
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect()
}

/// Content-word overlap (Jaccard) from 0 to 1, for spotting near-duplicates.
pub fn similarity(a: &str, b: &str) -> f64 {
    let (a, b) = (content_words(a), content_words(b));
    let union = a.union(&b).count();
    if union == 0 {
        return 0.0;
    }
    a.intersection(&b).count() as f64 / union as f64
}

/// Most words one search query carries; a long task body must not build a query
/// SQLite refuses.
pub const FTS_QUERY_MAX_WORDS: usize = 24;

/// An FTS5 `OR` query of the first distinct words, quoted, or `None` when there
/// is nothing to search for. Quoting keeps user text from being read as FTS
/// syntax, so no word needs to be dropped as a keyword.
pub fn fts_query(text: &str) -> Option<String> {
    let mut seen = HashSet::new();
    let words: Vec<String> = normalize_tokens(text)
        .into_iter()
        .filter(|word| seen.insert(word.clone()))
        .take(FTS_QUERY_MAX_WORDS)
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
