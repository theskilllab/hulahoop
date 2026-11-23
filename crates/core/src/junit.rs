//! JUnit XML parsing: the machine-readable verdict the gate reads.
//!
//! The parser is deliberately small and tolerant. It handles the two shapes
//! `just test` produces in practice — a `<testsuites>` root with `<testsuite>`
//! children and a bare `<testsuite>` root — and ignores everything else.

use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCase {
    pub name: String,
    pub classname: Option<String>,
    pub failed: bool,
    pub skipped: bool,
}

impl TestCase {
    /// The name with its class prefix, which is how a test is referred to in a
    /// review body (`tests.test_calc.test_ac1_addition`).
    pub fn qualified_name(&self) -> String {
        match (&self.classname, self.name.is_empty()) {
            (Some(class), false) => format!("{class}.{}", self.name),
            (_, _) => self.name.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JUnitReport {
    pub cases: Vec<TestCase>,
}

impl JUnitReport {
    pub fn total(&self) -> usize {
        self.cases.len()
    }

    pub fn failed_count(&self) -> usize {
        self.cases.iter().filter(|c| c.failed).count()
    }

    pub fn skipped_count(&self) -> usize {
        self.cases.iter().filter(|c| c.skipped).count()
    }

    /// A suite is green when it ran at least one test and none failed.
    /// A report with zero tests is not green: an empty suite proves nothing.
    pub fn is_green(&self) -> bool {
        !self.cases.is_empty() && self.failed_count() == 0
    }

    pub fn is_red(&self) -> bool {
        self.failed_count() > 0
    }

    pub fn failed_names(&self) -> Vec<String> {
        self.cases
            .iter()
            .filter(|c| c.failed)
            .map(|c| c.qualified_name())
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.cases.iter().map(|c| c.qualified_name()).collect()
    }

    pub fn case_named(&self, name: &str) -> Option<&TestCase> {
        self.cases.iter().find(|c| c.qualified_name() == name)
    }
}

/// Token match with a boundary on both sides, so `ac1` never matches `ac10`
/// and `test_ac1` still matches.
pub fn contains_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(found) = haystack[start..].find(token) {
        let at = start + found;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let after = at + token.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = at + token.len();
    }
    false
}

fn tag_attribute(block: &str, name: &str) -> Option<String> {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Regex>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let pattern = format!(r#"\b{name}\s*=\s*"([^"]*)""#);
    let regex = {
        let mut guard = cache.lock().unwrap();
        guard
            .entry(name.to_string())
            .or_insert_with(|| Regex::new(&pattern).expect("valid attribute regex"))
            .clone()
    };
    regex
        .captures(block)
        .and_then(|captures| captures.get(1))
        .map(|m| unescape(m.as_str()))
}

fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Split the document into `<testcase ...>...</testcase>` blocks, including the
/// self-closing form. Anything outside a testcase is ignored.
fn testcase_blocks(xml: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while let Some(open) = xml[cursor..].find("<testcase") {
        let start = cursor + open;
        let after_open = match xml[start..].find('>') {
            Some(offset) => start + offset,
            None => break,
        };
        let open_tag = &xml[start..after_open + 1];
        if open_tag.ends_with("/>") {
            blocks.push(&xml[start..after_open + 1]);
            cursor = after_open + 1;
            continue;
        }
        match xml[after_open..].find("</testcase>") {
            Some(offset) => {
                let end = after_open + offset + "</testcase>".len();
                blocks.push(&xml[start..end]);
                cursor = end;
            }
            None => break,
        }
    }
    blocks
}

pub fn parse(xml: &str) -> JUnitReport {
    let mut cases = Vec::new();
    for block in testcase_blocks(xml) {
        let open_end = block.find('>').map(|i| i + 1).unwrap_or(block.len());
        let open_tag = &block[..open_end];
        let body = &block[open_end..];
        let name = tag_attribute(open_tag, "name").unwrap_or_default();
        let classname = tag_attribute(open_tag, "classname");
        // `<failure`, `<error` and `<skipped` only count inside the testcase body,
        // and `<error` counts as a failure: a test that could not run did not pass.
        let failed = body.contains("<failure") || body.contains("<error");
        let skipped = !failed && body.contains("<skipped");
        cases.push(TestCase {
            name,
            classname,
            failed,
            skipped,
        });
    }

    // A report that declares counts but lists no testcases (some runners emit
    // only aggregates) is still red when it says so.
    if cases.is_empty() {
        if let Some(root) = xml.find("<testsuite") {
            let open_end = xml[root..]
                .find('>')
                .map(|i| root + i + 1)
                .unwrap_or(xml.len());
            let open_tag = &xml[root..open_end];
            let failures = tag_attribute(open_tag, "failures")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let errors = tag_attribute(open_tag, "errors")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let total = tag_attribute(open_tag, "tests")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            for index in 0..total {
                let failed = index < failures + errors;
                cases.push(TestCase {
                    name: format!("<unnamed-{index}>"),
                    classname: None,
                    failed,
                    skipped: false,
                });
            }
        }
    }

    JUnitReport { cases }
}

pub fn parse_file(path: &std::path::Path) -> std::io::Result<JUnitReport> {
    let xml = std::fs::read_to_string(path)?;
    Ok(parse(&xml))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUITE: &str = r#"<?xml version="1.0"?>
<testsuites>
  <testsuite name="pytest" tests="3" failures="1">
    <testcase classname="tests.test_calc" name="test_ac1_addition"/>
    <testcase classname="tests.test_calc" name="test_ac10_factorial">
      <failure message="expected 3628800">assert 6 == 3628800</failure>
    </testcase>
    <testcase classname="tests.test_calc" name="test_ac2_division">
      <skipped message="not implemented"/>
    </testcase>
  </testsuite>
</testsuites>"#;

    #[test]
    fn parses_cases_and_failures() {
        let report = parse(SUITE);
        assert_eq!(report.total(), 3);
        assert_eq!(report.failed_count(), 1);
        assert_eq!(report.skipped_count(), 1);
        assert!(report.is_red());
        assert!(!report.is_green());
        assert_eq!(
            report.failed_names(),
            vec!["tests.test_calc.test_ac10_factorial"]
        );
    }

    #[test]
    fn self_closing_testcases_are_cases() {
        let report =
            parse(r#"<testsuite tests="1"><testcase name="a" classname="c"/></testsuite>"#);
        assert_eq!(report.total(), 1);
        assert!(report.is_green());
    }

    #[test]
    fn errors_count_as_failures() {
        let report = parse(
            r#"<testsuite><testcase name="a"><error type="ImportError">boom</error></testcase></testsuite>"#,
        );
        assert!(report.is_red());
    }

    #[test]
    fn empty_report_is_not_green() {
        assert!(!parse("").is_green());
    }

    #[test]
    fn aggregate_only_report_is_still_red() {
        let report = parse(r#"<testsuite tests="2" failures="1" errors="0"/>"#);
        assert_eq!(report.total(), 2);
        assert_eq!(report.failed_count(), 1);
    }

    #[test]
    fn token_boundary_helper() {
        assert!(contains_token("test_ac1_addition", "ac1"));
        assert!(!contains_token("test_ac10_addition", "ac1"));
        assert!(contains_token("ac1", "ac1"));
        assert!(contains_token("prefix.ac2", "ac2"));
    }
}
