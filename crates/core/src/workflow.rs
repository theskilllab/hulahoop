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
    /// The agents named for this status. Empty means the project's binding decides.
    pub agents: Vec<String>,
    /// Where this status leads. A document may name several places, which is
    /// what makes the board a graph instead of a queue.
    pub next: Vec<IssueStatus>,
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
        self.section(status)
            .and_then(|s| s.agents.first())
            .map(|name| name.as_str())
    }

    /// Every place a status leads, in the order the document lists them. The
    /// board uses the first, the diagram draws all of them.
    pub fn next_of(&self, status: IssueStatus) -> Vec<IssueStatus> {
        self.section(status)
            .map(|s| s.next.clone())
            .unwrap_or_default()
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

/// The names in a metadata line: comma or semicolon separated, `@` optional,
/// and the words that mean "nobody" dropped.
fn names_in(value: &str) -> Vec<String> {
    value
        .split([',', ';'])
        .map(|token| token.trim().trim_start_matches('@').trim())
        .filter(|token| {
            !token.is_empty()
                && !token.eq_ignore_ascii_case("none")
                && !token.eq_ignore_ascii_case("none.")
                && !token.eq_ignore_ascii_case("any")
                && !token.eq_ignore_ascii_case("noone")
                && !token.eq_ignore_ascii_case("no one")
                && !token.eq_ignore_ascii_case("nobody")
        })
        .map(|token| token.to_string())
        .collect()
}

/// The words people use for themselves.
fn is_person(token: &str) -> bool {
    matches!(
        token.trim().to_ascii_lowercase().as_str(),
        "human" | "you" | "me" | "i" | "person" | "people" | "user" | "us"
    )
}

/// `To do - when it is ready` keeps `To do`; the sentence belongs to `When:`.
fn strip_reason(token: &str) -> &str {
    let mut cut = token.len();
    for marker in [" - ", " — ", " – ", " (", " when "] {
        if let Some(at) = token.to_lowercase().find(marker) {
            cut = cut.min(at);
        }
    }
    token[..cut].trim()
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
                        agents: Vec::new(),
                        next: Vec::new(),
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
        } else if let Some(value) = line
            .strip_prefix("User:")
            .or_else(|| line.strip_prefix("Users:"))
        {
            // `User: human, @Implementer, @Reviewer` — who handles this status,
            // on one line. A person and agents may both be named.
            let mut people = false;
            for token in names_in(value) {
                if is_person(&token) {
                    people = true;
                } else {
                    section.agents.push(token);
                }
            }
            section.owner = if section.agents.is_empty() {
                if people {
                    Owner::Human
                } else {
                    section.owner
                }
            } else {
                Owner::Agent
            };
        } else if let Some(value) = line.strip_prefix("Agent:") {
            // People write `@TestAuthor`, `Test Author`, `@Implementer, @Reviewer`
            // and `any`. Only real names survive: the rest is the project's call.
            for token in names_in(value) {
                section.agents.push(token);
            }
        } else if let Some(value) = line.strip_prefix("Next:") {
            // A `Next:` line may name several places and may carry the reason
            // inline (`Next: To do - when it is ready`). Only the names are read.
            for token in names_in(value) {
                let name = strip_reason(&token);
                match status_by_title(name) {
                    Some(next) => {
                        if !section.next.contains(&next) {
                            section.next.push(next);
                        }
                    }
                    None => workflow
                        .errors
                        .push(format!("`Next: {name}` names no status.")),
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
        match workflow.section(status) {
            None => workflow
                .errors
                .push(format!("`{}` has no section.", status.title())),
            Some(section) if section.owner == Owner::Agent && section.agents.is_empty() => {
                workflow.errors.push(format!(
                    "`{}` is worked by an agent, but names none. Write `Agent: <agent name>`.",
                    section.title
                ))
            }
            Some(section) if section.owner == Owner::Human && !section.agents.is_empty() => {
                // A section that names agents is agent-owned; saying so twice is
                // noise, so the names win and the line is corrected silently.
                let index = workflow
                    .sections
                    .iter()
                    .position(|s| s.status == section.status);
                if let Some(index) = index {
                    workflow.sections[index].owner = Owner::Agent;
                }
            }
            _ => {}
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
Agent: Implementer
Next: In progress
When: an agent picks it up

Ready for the team. An agent picks a task up from here.

## In progress

Owner: agent
Agent: Implementer
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
Agent: Implementer
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
        assert_eq!(workflow.agent_for(IssueStatus::Todo), Some("Implementer"));
        assert_eq!(workflow.agent_for(IssueStatus::Backlog), None);
    }

    #[test]
    fn an_agent_owned_status_must_name_its_agent() {
        let workflow = parse("## Backlog\nOwner: agent\nAgent: any\n");
        assert!(
            workflow
                .errors
                .iter()
                .any(|error| error.contains("names none")),
            "{:?}",
            workflow.errors
        );
    }

    #[test]
    fn next_lines_name_a_status() {
        let workflow = parse("## Backlog\nOwner: human\nNext: To do\n");
        let section = workflow.section(IssueStatus::Backlog).unwrap();
        assert_eq!(section.next, vec![IssueStatus::Todo]);
    }

    #[test]
    fn an_unknown_heading_is_an_error_not_a_column() {
        let workflow = parse("## Reviewing\nOwner: human\n");
        assert!(workflow.errors.iter().any(|e| e.contains("Reviewing")));
        assert!(workflow.sections.is_empty());
    }

    #[test]
    fn a_document_written_by_hand_is_read() {
        let workflow = parse(
            "## To do\n\nAgent: @TestAuthor, @Reviewer\nNext: In progress - when it is picked up, Blocked\n",
        );
        let section = workflow.section(IssueStatus::Todo).unwrap();
        assert_eq!(section.agents, vec!["TestAuthor", "Reviewer"]);
        assert_eq!(
            section.next,
            vec![IssueStatus::InProgress, IssueStatus::Blocked]
        );
        assert_eq!(section.owner, Owner::Agent);
    }

    #[test]
    fn one_user_line_names_a_person_and_agents() {
        let workflow =
            parse("## In progress\nUser: human, @Implementer, @Reviewer\nNext: In review\n");
        let section = workflow.section(IssueStatus::InProgress).unwrap();
        assert_eq!(section.owner, Owner::Agent);
        assert_eq!(section.agents, vec!["Implementer", "Reviewer"]);

        let only_people = parse("## In review\nUser: human\nNext: Done\n");
        let section = only_people.section(IssueStatus::InReview).unwrap();
        assert_eq!(section.owner, Owner::Human);
        assert!(section.agents.is_empty());
    }

    #[test]
    fn a_team_written_document_loads_without_complaint() {
        // The shape a person actually writes: `Owner:` sometimes absent, agents
        // with an `@`, two agents on one status, `Next: none` at the end.
        let source = "\
# Workflows

## Backlog

Owner: human
Agent: none
Next: To do

Work you still want to do yourself.

## To do

Agent: @TestAuthor
Next: In progress

Ready for the team.

## In progress

Owner: human
Agent: @Implementer, @Reviewer
Next: In review

An agent is working.

## Blocked

Owner: human
Agent: none
Next: To do

A question was asked.

## In review

Owner: human
Agent: any
Next: Done

A pull request is open.

## Done

Owner: human
Agent: none
Next: none

Merged.
";
        let workflow = parse(source);
        assert!(workflow.errors.is_empty(), "{:?}", workflow.errors);
        assert!(workflow.is_complete());
        assert_eq!(workflow.agent_for(IssueStatus::Todo), Some("TestAuthor"));
        assert_eq!(
            workflow.section(IssueStatus::InProgress).unwrap().owner,
            Owner::Agent
        );
        assert!(workflow.next_of(IssueStatus::Done).is_empty());
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
