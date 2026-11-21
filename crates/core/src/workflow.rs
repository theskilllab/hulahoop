//! The workflow document: how a workspace moves work through its statuses.
//!
//! One markdown file per workspace, and every `## ` heading names a status. The
//! lines under it say who owns the status, which agent works it, and where it
//! leads next. The board is a view of this document, so changing the workflow is
//! editing a file rather than a form with twenty switches.

use crate::IssueStatus;

/// Who is responsible for a status while work sits in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Human,
    Agent,
}

impl Owner {
    pub fn as_str(self) -> &'static str {
        match self {
            Owner::Human => "human",
            Owner::Agent => "agent",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "human" | "you" | "person" => Some(Owner::Human),
            "agent" | "agents" => Some(Owner::Agent),
            _ => None,
        }
    }
}

/// One status section of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSection {
    pub status: IssueStatus,
    /// The heading as written, so the board can show the team's own wording.
    pub title: String,
    pub owner: Owner,
    /// An agent name, or `any` to mean the project's binding for that status.
    pub agent: Option<String>,
    pub next: Option<IssueStatus>,
    /// Why the move happens. `When:` is what the diagram writes on the arrow and
    /// what the transitions table shows, so the picture and the rule agree.
    pub when: Option<String>,
    /// Everything under the metadata lines, kept verbatim for the status screen.
    pub body: String,
}

/// A parsed workflow. `errors` are what the editor shows instead of a board it
/// cannot trust.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workflow {
    pub sections: Vec<StatusSection>,
    pub errors: Vec<String>,
}

impl Workflow {
    pub fn section(&self, status: IssueStatus) -> Option<&StatusSection> {
        self.sections.iter().find(|s| s.status == status)
    }

    /// The agent a status dispatches to, when the document gives one.
    pub fn agent_for(&self, status: IssueStatus) -> Option<&str> {
        self.section(status).and_then(|s| s.agent.as_deref())
    }

    /// The board order: the sections in the order the document lists them.
    pub fn board(&self) -> Vec<IssueStatus> {
        self.sections.iter().map(|s| s.status).collect()
    }

    /// Every section names a status the machine knows, and the machine knows no
    /// status the document omits; both halves have to agree for the board to be
    /// the whole truth.
    pub fn is_complete(&self) -> bool {
        self.errors.is_empty()
            && IssueStatus::board()
                .iter()
                .all(|status| self.section(*status).is_some())
    }
}

/// Parse `WORKFLOWS.md`.
///
/// The format is deliberately plain: `## To do` starts a section, and the three
/// lines `Owner:`, `Agent:` and `Next:` are metadata. A heading that names no
/// status, a missing owner, or a `Next` that names no status becomes an error
/// rather than a silent default.
pub fn parse(source: &str) -> Workflow {
    let mut workflow = Workflow::default();
    let mut current: Option<StatusSection> = None;
    let mut body_lines: Vec<String> = Vec::new();

    // Spaces are significant to people and irrelevant to the machine, so
    // `In progress`, `in-progress` and `InProgress` all name the same status.
    let key = |text: &str| -> String {
        text.trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect()
    };
    let known: Vec<(IssueStatus, String)> = IssueStatus::board()
        .iter()
        .map(|s| (*s, key(s.as_str())))
        .collect();

    let status_by_title = |title: &str| -> Option<IssueStatus> {
        let needle = key(title);
        known
            .iter()
            .find(|(_, words)| *words == needle)
            .map(|(status, _)| *status)
    };

    for line in source.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(section) = current.take() {
                let mut section = section;
                section.body = body_lines.join("\n").trim().to_string();
                workflow.sections.push(section);
            }
            body_lines.clear();
            match status_by_title(heading) {
                Some(status) => {
                    if workflow.sections.iter().any(|s| s.status == status) {
                        workflow.errors.push(format!(
                            "`{heading}` appears twice.",
                            heading = heading.trim()
                        ));
                    }
                    current = Some(StatusSection {
                        status,
                        title: heading.trim().to_string(),
                        owner: Owner::Human,
                        agent: None,
                        next: None,
                        when: None,
                        body: String::new(),
                    });
                }
                None => {
                    workflow.errors.push(format!(
                        "`{}` is not a status. The statuses are: {}.",
                        heading.trim(),
                        IssueStatus::board()
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                    // Keep collecting the body under no section, so one typo does
                    // not swallow the rest of the document.
                    current = None;
                }
            }
            continue;
        }

        let Some(section) = current.as_mut() else {
            continue;
        };

        if let Some(value) = line.strip_prefix("Owner:") {
            let value = value.trim();
            match Owner::parse(value) {
                Some(owner) => section.owner = owner,
                None => workflow
                    .errors
                    .push(format!("`Owner: {value}` should be `human` or `agent`.")),
            }
        } else if let Some(value) = line.strip_prefix("Agent:") {
            let value = value.trim();
            if !value.is_empty() && !value.eq_ignore_ascii_case("none") {
                section.agent = Some(value.to_string());
            }
        } else if let Some(value) = line.strip_prefix("Next:") {
            let value = value.trim();
            if value.eq_ignore_ascii_case("none") || value.is_empty() {
                section.next = None;
            } else {
                match status_by_title(value) {
                    Some(next) => section.next = Some(next),
                    None => workflow
                        .errors
                        .push(format!("`Next: {value}` names no status.")),
                }
            }
        } else if let Some(value) = line.strip_prefix("When:") {
            let value = value.trim();
            if !value.is_empty() {
                section.when = Some(value.to_string());
            }
        } else if !line.trim().is_empty() || !body_lines.is_empty() {
            body_lines.push(line.to_string());
        }
    }

    if let Some(section) = current.take() {
        let mut section = section;
        section.body = body_lines.join("\n").trim().to_string();
        workflow.sections.push(section);
    }

    if workflow.sections.is_empty() {
        workflow
            .errors
            .push("the document has no `## Status` sections, so the board would be empty.".into());
    }
    for status in IssueStatus::board() {
        if workflow.section(status).is_none() {
            workflow
                .errors
                .push(format!("`{}` has no section.", status.title()));
        }
    }

    workflow
}

/// The workflow a new workspace starts from: the human queues, an agent works,
/// a reviewer reads, the merge closes it.
pub const DEFAULT_SOURCE: &str = "\
# Workflows

## Backlog

Owner: human
Agent: none
Next: To do
When: it has been planned

Work you still want to do yourself. Nothing here is dispatched.

## To do

Owner: agent
Agent: any
Next: In progress
When: an agent picks it up

Ready for the team. An agent picks a task up from here.

## In progress

Owner: agent
Agent: any
Next: In review
When: a pull request is opened

An agent is working. It opens a pull request when it hands over.

## Blocked

Owner: human
Agent: none
Next: In progress
When: the question is answered

The agent asked a question, or the work waits on somebody outside.

## In review

Owner: human
Agent: any
Next: Done
When: you approve and the pull request is merged

A pull request is open and the checks have run. A person reads it.

## Done

Owner: human
Agent: none
Next: none

The pull request merged.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_source_is_complete() {
        let workflow = parse(DEFAULT_SOURCE);
        assert!(workflow.errors.is_empty(), "{:?}", workflow.errors);
        assert!(workflow.is_complete());
        assert_eq!(workflow.board().first(), Some(&IssueStatus::Backlog));
        assert_eq!(workflow.agent_for(IssueStatus::Todo), Some("any"));
        assert_eq!(workflow.agent_for(IssueStatus::Backlog), None);
    }

    #[test]
    fn next_lines_name_a_status() {
        let workflow = parse("## Backlog\nOwner: human\nNext: To do\n");
        let section = workflow.section(IssueStatus::Backlog).unwrap();
        assert_eq!(section.next, Some(IssueStatus::Todo));
    }

    #[test]
    fn an_unknown_heading_is_an_error_not_a_column() {
        let workflow = parse("## Reviewing\nOwner: human\n");
        assert!(workflow.errors.iter().any(|e| e.contains("Reviewing")));
        assert!(workflow.sections.is_empty());
    }

    #[test]
    fn a_missing_owner_names_the_line() {
        let workflow = parse("## Backlog\nOwner: robot\n");
        assert!(workflow.errors.iter().any(|e| e.contains("Owner: robot")));
    }

    #[test]
    fn a_missing_section_is_reported() {
        let workflow = parse("## Backlog\nOwner: human\n");
        assert!(workflow.errors.iter().any(|e| e.contains("To do")));
        assert!(!workflow.is_complete());
    }

    #[test]
    fn the_body_keeps_its_wording() {
        let workflow = parse("## To do\nOwner: agent\n\nWrite a test first.\n");
        assert_eq!(
            workflow.section(IssueStatus::Todo).unwrap().body,
            "Write a test first."
        );
    }
}
