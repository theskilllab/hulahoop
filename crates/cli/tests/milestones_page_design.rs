//! Acceptance tests for the revamped Milestones page. The desktop app has no JS
//! test runner, so these read the page source and stylesheet and assert the
//! structure the prototype in `Hulahoop 0.1.0.zip` (master list + detail pane,
//! `.msplit` / `.mslist` / `.msrow` / `.msdetail` / `.msd`) prescribes.

use std::path::Path;

fn desktop(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn page() -> String {
    desktop("screens/Milestones.tsx")
}

fn css() -> String {
    desktop("styles.css")
}

/// The body of the first rule whose selector list contains `selector`.
fn rule(css: &str, selector: &str) -> Option<String> {
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let head = &rest[..open];
        let close = rest[open..].find('}')? + open;
        if head
            .rsplit('}')
            .next()
            .unwrap_or(head)
            .split(',')
            .any(|s| s.trim() == selector)
        {
            return Some(rest[open + 1..close].to_string());
        }
        rest = &rest[close + 1..];
    }
    None
}

// Criterion 1: layout, typography, spacing, colors, component styling.
#[test]
fn page_uses_the_prototype_master_detail_layout() {
    let tsx = page();
    for class in ["msplit", "mslist", "msrow", "msdetail", "msd"] {
        assert!(
            tsx.contains(class),
            "Milestones.tsx lacks the prototype class `{class}`"
        );
    }
}

#[test]
fn stylesheet_carries_the_prototype_milestone_rules() {
    let css = css();
    let split = rule(&css, ".msplit").expect(".msplit rule");
    assert!(
        split.contains("minmax(300px, 360px)"),
        "msplit columns: {split}"
    );
    let row = rule(&css, ".msrow").expect(".msrow rule");
    assert!(
        row.contains("padding: var(--s3) var(--s4)"),
        "msrow spacing: {row}"
    );
    let detail = rule(&css, ".msd").expect(".msd rule");
    assert!(detail.contains("max-width: 820px"), "msd width: {detail}");
    let fill = rule(&css, ".prog > i").expect(".prog > i rule");
    assert!(
        fill.contains("background: var(--ok)"),
        "progress fill color: {fill}"
    );
    assert!(
        css.contains(".msrow[aria-current=\"true\"]"),
        "selected row style"
    );
    assert!(css.contains(".ms-meta"), "meta line style");
}

// Criterion 2: titles, statuses and dates come from the workspace data.
#[test]
fn page_renders_title_status_and_target_date_from_data() {
    let tsx = page();
    for field in [
        "milestone.title",
        "milestone.status",
        "milestone.targetDate",
    ] {
        assert!(
            tsx.contains(field) || tsx.contains(&field.replace("milestone.", "selected.")),
            "page does not render `{field}`"
        );
    }
    assert!(
        tsx.contains("issue.milestoneId"),
        "members must be filtered by milestoneId"
    );
}

// Criterion 3: existing navigation and actions are kept.
#[test]
fn existing_milestone_actions_survive() {
    let tsx = page();
    for needle in [
        "createMilestone",
        "updateMilestone",
        "setIssueMilestone",
        "Close milestone",
        "Reopen milestone",
        "New milestone",
    ] {
        assert!(tsx.contains(needle), "Milestones.tsx lost `{needle}`");
    }
}

// Criterion 4: mobile width and keyboard focus.
#[test]
fn layout_collapses_at_mobile_width() {
    let css = css();
    let at = css
        .find(".screen:has(.msplit)")
        .expect("mobile msplit block");
    let block = &css[at..];
    assert!(
        block.contains(".msplit { grid-template-columns: minmax(0, 1fr)"),
        "msplit must stack on mobile"
    );
    assert!(block.contains(".mslist"), "mslist must adapt on mobile");
}

#[test]
fn rows_are_keyboard_operable_buttons() {
    let tsx = page();
    let at = tsx.find("className=\"msrow\"").expect("msrow element");
    let opening = tsx[..at].rfind('<').expect("opening tag");
    assert!(
        tsx[opening..].starts_with("<button"),
        "msrow must be a <button>, not a div, to take focus"
    );
    assert!(
        tsx.contains("aria-current"),
        "selected row must be exposed with aria-current"
    );
    assert!(
        !tsx.contains("role=\"listbox\""),
        "a listbox needs arrow-key handling the rows do not have"
    );
}

#[test]
fn task_status_uses_the_shared_status_dots() {
    let tsx = page();
    assert!(
        tsx.contains("<StatusIcon status={issue.status} />"),
        "task statuses must use the shared status icons"
    );
}

#[test]
fn task_statuses_follow_the_board_order_and_labels() {
    let tsx = page();
    assert!(
        tsx.contains("columns.map((column) => column.status)"),
        "status order must come from the workflow columns"
    );
    assert!(
        tsx.contains("column.title") || tsx.contains("entry.status === status"),
        "status labels must match the board's wording"
    );
}
