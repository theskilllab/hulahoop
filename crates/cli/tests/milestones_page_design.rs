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

fn dialog() -> String {
    desktop("components/NewMilestoneDialog.tsx")
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

// Criterion 3: existing navigation and actions are kept. Creating a milestone
// moved from the page into `NewMilestoneDialog`, so the two files are read together.
#[test]
fn existing_milestone_actions_survive() {
    let tsx = format!("{}\n{}", page(), dialog());
    for needle in [
        "createMilestone",
        "updateMilestone",
        "setIssueMilestone",
        "Close milestone",
        "Reopen milestone",
        "New milestone",
    ] {
        assert!(tsx.contains(needle), "Milestones page lost `{needle}`");
    }
}

// Bug-fix ticket: New milestone is a sheet like New task, not an inline panel.
#[test]
fn new_milestone_opens_a_dialog_like_new_task() {
    let tsx = page();
    assert!(
        tsx.contains("NewMilestoneDialog"),
        "Milestones.tsx must open NewMilestoneDialog"
    );
    assert!(
        !tsx.contains("ms-new"),
        "the inline `.ms-new` panel must be gone"
    );
    let dialog = dialog();
    for needle in [
        "<dialog",
        "settings-dialog",
        "new-milestone-dialog",
        "showModal()",
        "createMilestone",
        "uploadArtifact",
    ] {
        assert!(
            dialog.contains(needle),
            "NewMilestoneDialog lacks `{needle}`"
        );
    }
}

#[test]
fn new_milestone_copy_follows_the_prototype() {
    let dialog = dialog();
    for copy in [
        "Due date",
        "Description",
        "(optional)",
        "No project",
        "or choose files to attach",
        "Name the milestone so people know what it is working toward.",
        "You can drag and drop files here to attach them.",
    ] {
        assert!(
            dialog.contains(copy),
            "NewMilestoneDialog lacks the copy `{copy}`"
        );
    }
    for old in [
        "target date",
        "what it is for",
        "creating…\"",
        "create milestone",
    ] {
        assert!(
            !dialog.contains(old) && !page().contains(old),
            "the old copy `{old}` is still there"
        );
    }
}

#[test]
fn description_is_a_drop_area_for_artifacts() {
    let dialog = dialog();
    for needle in [
        "new-milestone-text",
        "onDragOver",
        "onDrop",
        "type=\"file\"",
        "milestoneId: created.id",
    ] {
        assert!(
            dialog.contains(needle),
            "NewMilestoneDialog lacks `{needle}`"
        );
    }
}

#[test]
fn dialog_fields_use_the_prototype_field_styles() {
    let dialog = dialog();
    assert!(
        dialog.contains("field-label"),
        "labels use the prototype's .field-label"
    );
    let css = css();
    assert!(
        css.contains(".new-milestone-dialog"),
        "dialog styles are scoped under .new-milestone-dialog"
    );
    assert!(
        css.contains(".new-milestone-text.over") || css.contains(".new-milestone-text.over,"),
        "the drop area highlights while a file is dragged over it"
    );
}

// Bug-fix ticket: the detail progress is as wide as the lists below it.
#[test]
fn detail_progress_is_as_wide_as_the_rest_of_the_pane() {
    let css = css();
    let progress = rule(&css, ".ms-progress").expect(".ms-progress rule");
    assert!(
        !progress.contains("max-width"),
        "`.ms-progress` must fill `.msd`, not stop at 640px: {progress}"
    );
}

// Bug-fix ticket: a milestone with no due date says so once.
#[test]
fn list_row_does_not_print_no_date_beside_no_due_date() {
    let tsx = page();
    assert!(
        !tsx.contains("\"No date\""),
        "the list row must not print a placeholder date before `No due date`"
    );
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
