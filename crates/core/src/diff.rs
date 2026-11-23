//! Reading a unified diff: which files changed, how many lines moved, and
//! whether any of them are contract files.

use crate::contract::{classify_path, is_excluded, PathVerdict};
use crate::{RunFlag, DIFF_LINE_LIMIT};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffSummary {
    pub files: Vec<String>,
    pub added: i64,
    pub removed: i64,
}

impl DiffSummary {
    pub fn changed_lines(&self) -> i64 {
        self.added + self.removed
    }

    pub fn is_large(&self) -> bool {
        self.changed_lines() > DIFF_LINE_LIMIT
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffVerdict {
    pub summary: DiffSummary,
    pub flags: Vec<RunFlag>,
}

/// Parse `+++ b/<path>` headers and count content lines, skipping the hunk
/// headers. `.hulahoop/` and a generated `.envrc` never count.
pub fn summarize(patch: &str) -> DiffSummary {
    let mut summary = DiffSummary::default();
    // An excluded file contributes no paths and no line counts, so a generated
    // `.envrc` or a `.hulahoop/` artifact cannot inflate the diff.
    let mut counted = false;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = rest
                .trim_start_matches("b/")
                .trim_start_matches("a/")
                .trim_start_matches("./");
            counted = path != "/dev/null" && !is_excluded(path);
            if counted {
                summary.files.push(path.to_string());
            }
            continue;
        }
        if line.starts_with("--- ") || line.starts_with("diff ") || line.starts_with("index ") {
            continue;
        }
        if line.starts_with("@@") {
            continue;
        }
        if !counted {
            continue;
        }
        if line.starts_with('+') && !line.starts_with("+++") {
            summary.added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            summary.removed += 1;
        }
    }
    summary.files.sort();
    summary.files.dedup();
    summary
}

/// Classify a diff against the contract, and derive the flags the approve
/// dialog has to show.
pub fn classify(patch: &str) -> DiffVerdict {
    let summary = summarize(patch);
    let mut flags = Vec::new();

    for path in &summary.files {
        match classify_path(path) {
            PathVerdict::Gated(flag) => flags.push(flag),
            PathVerdict::Allowed => {}
        }
    }
    if summary.is_large() {
        flags.push(RunFlag::LargeDiff);
    }
    flags.sort_by_key(|flag| flag.as_str());
    flags.dedup();

    DiffVerdict { summary, flags }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = r#"diff --git a/calc.py b/calc.py
index 111..222 100644
--- a/calc.py
+++ b/calc.py
@@ -1,2 +1,6 @@
 def add(a, b):
     return a + b
+
+
+def mul(a, b):
+    return a * b
diff --git a/justfile b/justfile
--- a/justfile
+++ b/justfile
@@ -1 +1 @@
-test:
+test:  # weakened
"#;

    #[test]
    fn counts_lines_and_files() {
        let summary = summarize(PATCH);
        assert_eq!(summary.files, vec!["calc.py", "justfile"]);
        assert_eq!(summary.added, 5);
        assert_eq!(summary.removed, 1);
    }

    #[test]
    fn contract_files_are_flagged_not_rejected() {
        let verdict = classify(PATCH);
        assert!(verdict.flags.contains(&RunFlag::TouchedContract));
    }

    #[test]
    fn generated_files_are_invisible() {
        let patch = "+++ b/.envrc\n@@ -1 +1 @@\n+use flake\n";
        let summary = summarize(patch);
        assert!(summary.files.is_empty());
        assert_eq!(summary.added, 0);
    }
}
