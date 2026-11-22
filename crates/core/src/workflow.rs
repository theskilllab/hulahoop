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
}

/// One move out of a status: where it goes and why. A section lists as many as
/// the team needs, and each may carry its own sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub to: IssueStatus,
    /// The reason, from `Next: To do - when it is ready` or from the section's
    /// `When:` line when the move gives none.
    pub when: Option<String>,
}

/// One status section of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSection {
    pub status: IssueStatus,
    /// The heading as written, so the board can show the team's own wording.
    pub title: String,
    pub owner: Owner,
    /// The `Users:` line as written, people included, so the editor can put it
    /// back the way it was found instead of inventing a different sentence.
    pub users: Vec<String>,
    /// The agents named for this status. Empty means the project's binding decides.
    pub agents: Vec<String>,
    /// `Agent: any idle agent` — nobody specific, the project's binding decides.
    pub any_agent: bool,
    /// Every move out of this status, in the order the document lists them.
    pub transitions: Vec<Transition>,
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
            .map(|s| s.transitions.iter().map(|m| m.to).collect())
            .unwrap_or_default()
    }

    /// The moves out of a status with their reasons: what the diagram draws on
    /// each arrow and what the transitions table prints.
    pub fn transitions_of(&self, status: IssueStatus) -> Vec<Transition> {
        self.section(status)
            .map(|s| s.transitions.clone())
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

/// `any`, `any agent`, `any idle agent`: whoever the project binds to this column.
fn is_any(token: &str) -> bool {
    token.trim().to_ascii_lowercase().starts_with("any")
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

/// The sentence after ` - ` on a `Next:` line, when there is one.
fn reason_in(token: &str) -> Option<String> {
    let lower = token.to_lowercase();
    for marker in [" - ", " — ", " – "] {
        if let Some(at) = lower.find(marker) {
            let reason = token[at + marker.len()..].trim();
            if !reason.is_empty() {
                return Some(reason.to_string());
            }
        }
    }
    None
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
                        users: Vec::new(),
                        agents: Vec::new(),
                        any_agent: false,
                        transitions: Vec::new(),
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

        // `Users:` is the only metadata line about people. `human` means a
        // person does it; a name means that agent; `any` means whichever agent
        // the project binds to this column. One line, comma separated, `@`
        // optional — nothing else to remember, and nothing else to get wrong.
        if let Some(value) = line
            .strip_prefix("Users:")
            .or_else(|| line.strip_prefix("User:"))
        {
            for token in names_in(value) {
                section.users.push(token.clone());
                if is_person(&token) {
                    continue;
                } else if is_any(&token) {
                    section.any_agent = true;
                } else {
                    section.agents.push(strip_reason(&token).to_string());
                }
            }
        } else if let Some(value) = line.strip_prefix("Next:") {
            // `Next: In review - when the pull request is open` names one place
            // and its reason. The reason is taken off first, because a sentence
            // is allowed to contain commas — splitting the line before that
            // turned `- needs a decision, a harness, or a host` into three
            // statuses that never existed.
            let line_value = value.trim();
            let reason = reason_in(line_value);
            let names = [" - ", " — ", " – "]
                .iter()
                .find_map(|marker| line_value.split_once(marker))
                .map(|(head, _)| head.trim().to_string())
                .unwrap_or_else(|| line_value.to_string());
            for token in names_in(&names) {
                let name = token.trim();
                match status_by_title(name) {
                    Some(to) => {
                        if !section.transitions.iter().any(|move_| move_.to == to) {
                            let when = reason.clone();
                            section.transitions.push(Transition { to, when });
                        }
                    }
                    None => workflow.errors.push(format!(
                        "`Next: {name}` names no status. Use one of: {}.",
                        IssueStatus::board()
                            .iter()
                            .map(|s| s.title())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
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

    for section in &mut workflow.sections {
        // The owner is what the `Users:` line says, read back: names on the
        // line mean an agent works the status, and a line that names only people
        // (or no line at all) means a person does.
        section.owner = if section.agents.is_empty() && !section.any_agent {
            Owner::Human
        } else {
            Owner::Agent
        };
        if let Some(sentence) = section.when.clone() {
            for move_ in &mut section.transitions {
                if move_.when.is_none() {
                    move_.when = Some(sentence.clone());
                }
            }
        }
    }

    if workflow.sections.is_empty() {
        workflow
            .errors
            .push("the document has no `## Status` sections, so the board would be empty.".into());
    }
    for status in IssueStatus::board() {
        // A section with no `Users:` line is a person's column: the safe default,
        // because nothing is dispatched where nobody was named.
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

Users: human
Next: To do

Work you still want to do yourself. Nothing here is dispatched.

## To do

Users: Implementer
Next: In progress

Ready for the team. The agent picks a task up from here and writes the tests
that the acceptance criteria describe.

## In progress

Users: Implementer, Reviewer
Next: In review - when the pull request is open and the tests pass
Next: Blocked - when the agent needs a decision it cannot make itself

The agent works against the criteria. The reviewer reads the pull request when
it is open, and sends it back when the work is not there yet.

## Blocked

Users: human
Next: In progress - when the question is answered

The agent asked a question, or the work waits on somebody outside.

## In review

Users: human
Next: Done - when the pull request is merged

A pull request is open and the checks have run. A person reads it.

## Done

The pull request merged. Nothing is dispatched from here.
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
    fn a_section_without_users_is_a_persons_column_and_not_an_error() {
        let mut source = String::from(DEFAULT_SOURCE);
        source = source.replace("## Done\n\nThe pull request merged.", "## Done\n\nMerged.");
        let workflow = parse(&source);
        assert!(workflow.errors.is_empty(), "{:?}", workflow.errors);
        assert_eq!(
            workflow.section(IssueStatus::Done).map(|s| s.owner),
            Some(Owner::Human)
        );
    }

    #[test]
    fn next_lines_name_a_status() {
        let workflow = parse("## Backlog\nOwner: human\nNext: To do\n");
        let section = workflow.section(IssueStatus::Backlog).unwrap();
        assert_eq!(
            section.transitions,
            vec![Transition {
                to: IssueStatus::Todo,
                when: None
            }]
        );
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
            "## To do\n\nUsers: @TestAuthor, @Reviewer\nNext: In progress - when it is picked up\nNext: Blocked - when a decision is needed, and the agent cannot make it\n",
        );
        let section = workflow.section(IssueStatus::Todo).unwrap();
        assert_eq!(section.agents, vec!["TestAuthor", "Reviewer"]);
        assert_eq!(
            section.transitions.iter().map(|m| m.to).collect::<Vec<_>>(),
            vec![IssueStatus::InProgress, IssueStatus::Blocked]
        );
        assert_eq!(section.owner, Owner::Agent);
    }

    #[test]
    fn several_next_lines_each_keep_their_reason() {
        let lines = [
            "## In progress",
            "User: human, @Implementer, @Reviewer",
            "Next: In review - @Reviewer has read the work",
            "Next: Blocked - an agent needs a decision",
            "Next: In progress - @Reviewer found something to fix",
            "",
        ];
        let source = lines.join("\n");
        let workflow = parse(&source);
        let moves = workflow.transitions_of(IssueStatus::InProgress);
        assert_eq!(moves.len(), 3);
        assert_eq!(moves[0].to, IssueStatus::InReview);
        assert_eq!(
            moves[0].when.as_deref(),
            Some("@Reviewer has read the work")
        );
        assert_eq!(moves[2].to, IssueStatus::InProgress);
        // The document is short on purpose; only the moves are under test.
        assert!(
            !workflow
                .errors
                .iter()
                .any(|error| error.contains("names no status")),
            "{:?}",
            workflow.errors
        );
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
        // The shape a person actually writes: `Users:` with an `@`, two agents
        // on one status, a reason containing commas, `Next: none` at the end.
        let source = "\
# Workflows

## Backlog

Users: human
Next: To do

Work you still want to do yourself.

## To do

Users: @TestAuthor
Next: In progress

Ready for the team.

## In progress

Users: @Implementer, @Reviewer
Next: Blocked - needs a decision, a harness, or a host
Next: In review

An agent is working.

## Blocked

Users: human
Next: To do

A question was asked.

## In review

Users: any
Next: Done

A pull request is open.

## Done

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
    fn a_next_that_names_nothing_is_reported_with_the_options() {
        let workflow = parse("## Backlog\nNext: somewhere\n");
        let error = workflow
            .errors
            .iter()
            .find(|e| e.contains("somewhere"))
            .expect("an unknown status is an error");
        assert!(
            error.contains("Backlog"),
            "the message lists the statuses: {error}"
        );
    }

    #[test]
    fn a_reason_may_contain_commas() {
        let workflow = parse(
            "## In progress\nNext: Blocked - needs a decision, a harness, or a host system.\n",
        );
        assert!(
            !workflow
                .errors
                .iter()
                .any(|e| e.contains("names no status")),
            "a comma inside the reason is not a status: {:?}",
            workflow.errors
        );
        let move_ = &workflow
            .section(IssueStatus::InProgress)
            .unwrap()
            .transitions[0];
        assert_eq!(move_.to, IssueStatus::Blocked);
        assert_eq!(
            move_.when.as_deref(),
            Some("needs a decision, a harness, or a host system.")
        );
    }

    #[test]
    fn a_missing_section_is_reported() {
        let workflow = parse("## Backlog\nUsers: human\n");
        assert!(workflow.errors.iter().any(|e| e.contains("To do")));
        assert!(!workflow.is_complete());
    }

    #[test]
    fn the_body_keeps_its_wording() {
        let workflow = parse("## To do\nUsers: Implementer\n\nWrite a test first.\n");
        assert_eq!(
            workflow.section(IssueStatus::Todo).unwrap().body,
            "Write a test first."
        );
    }
}
