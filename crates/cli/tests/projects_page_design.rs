//! Acceptance tests for the revamped Projects page. The desktop app's component
//! tests are not part of `just test`, so these read the page source and stylesheet
//! and assert the structure the prototype in `Hulahoop 0.1.0.zip` prescribes
//! (`.pgrid` / `.pcard` / `.pc` / `.pgear` / `.pj-*`, `#pjDlg`).

use std::path::Path;

fn desktop(file: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src")
        .join(file);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn page() -> String {
    desktop("screens/Misc.tsx")
}
fn dialog() -> String {
    desktop("components/ProjectDialog.tsx")
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

fn has(text: &str, what: &[&str], file: &str) {
    for w in what {
        assert!(text.contains(w), "{file} lacks `{w}`");
    }
}

#[test]
fn header_has_a_primary_new_project_button_with_a_plus_icon() {
    let tsx = page();
    has(&tsx, &["New project", "name=\"plus\""], "Misc.tsx");
    assert!(!tsx.contains("subtitle={`${projects.length} projects`}"));
    assert!(!tsx.contains("className=\"ghost\" onClick={() => setManaging(\"new\")"));
}

#[test]
fn grid_and_card_use_the_prototype_geometry() {
    let css = css();
    let grid = rule(&css, ".pgrid").expect(".pgrid rule");
    assert!(
        grid.contains("repeat(auto-fill, minmax(280px, 1fr))"),
        "{grid}"
    );
    assert!(grid.contains("gap: var(--s4)"), "{grid}");
    let card = rule(&css, ".pcard").expect(".pcard rule");
    for want in [
        "border: 1px solid var(--line)",
        "border-radius: var(--r-lg)",
        "padding: var(--s4)",
        "gap: var(--s3)",
    ] {
        assert!(card.contains(want), ".pcard lacks `{want}`: {card}");
    }
    assert!(
        css.contains(".pcard:hover")
            && css[css.find(".pcard:hover").unwrap()..].contains("var(--muted)"),
        "hover border must be --muted"
    );
    let gear = rule(&css, ".pgear").expect(".pgear rule");
    assert!(gear.contains("position: absolute"), "{gear}");
    let bar = rule(&css, ".prog").expect(".prog rule");
    assert!(bar.contains("height"), "{bar}");
}

#[test]
fn card_structure_matches_the_prototype() {
    let tsx = page();
    has(
        &tsx,
        &[
            "pgrid",
            "pcard",
            "pgear",
            "Settings for ",
            "Open tasks",
            "Open PRs",
            "Agents running",
            "Progress",
            "role=\"progressbar\"",
            "No repositories yet",
            "No project",
            "Tasks and milestones that don't belong to a project.",
            "Open milestones",
            "onOpenTasks",
        ],
        "Misc.tsx",
    );
}

#[test]
fn card_numbers_come_from_a_pure_stats_helper() {
    let s = desktop("projectStats.ts");
    has(
        &s,
        &[
            "export function projectStats",
            "export function deriveKey",
            "export function validateKey",
            "export const PROJECT_COLORS",
            "#2f7ad8",
            "#1f9d7a",
            "#c2692b",
            "#6a5acd",
            "#d4558a",
            "#b88100",
            "#d03b3b",
            "#64748b",
            "Use 2 to 5 letters or numbers, starting with a letter.",
            "is already used by another project.",
        ],
        "projectStats.ts",
    );
}

#[test]
fn dialog_is_a_modal_with_the_prototype_copy() {
    let d = dialog();
    has(
        &d,
        &[
            "<dialog",
            "showModal",
            "New project",
            "Project settings",
            "Give the project a name.",
            "Task ID prefix",
            "Tasks will be numbered ",
            "Existing tasks keep this prefix, like ",
            "Color",
            "radiogroup",
            "Shown beside the project name in filters, lists, milestones and artifacts, so you can tell projects apart at a glance.",
            "Repositories",
            "No repositories linked yet.",
            "Link a repository",
            "Search your GitHub repositories",
            "No repositories match.",
            "Every repository is already linked.",
            "Create project",
            "Save changes",
            "Cancel",
            "maxLength={5}",
            "disabled",
            "unlinkRepo",
            "Fetch base",
        ],
        "ProjectDialog.tsx",
    );
}

#[test]
fn dialog_stylesheet_carries_the_pj_components() {
    let css = css();
    for sel in [".pj-prefix", ".pj-swatch", ".pj-repo"] {
        assert!(css.contains(sel), "styles.css lacks `{sel}`");
    }
    let prefix = rule(&css, ".pj-prefix").expect(".pj-prefix");
    assert!(prefix.contains("140px"), "{prefix}");
}

#[test]
fn inline_manager_and_old_card_footer_are_gone() {
    let css = css();
    assert!(
        !css.contains(".project-manager"),
        "old .project-manager rules remain"
    );
    assert!(
        !css.contains(".project-card"),
        "old .project-card rules remain"
    );
    assert!(
        !Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src/screens/ProjectManager.tsx")
            .exists(),
        "ProjectManager.tsx should be replaced by the dialog"
    );
    let tsx = page();
    assert!(!tsx.contains("project-card") && !tsx.contains("Open ${own[0].key}"));
    assert!(!tsx.contains("ProjectManager"));
}

#[test]
fn api_exposes_colour_and_unlink() {
    let api = desktop("api.ts");
    has(&api, &["color", "unlinkRepo"], "api.ts");
}

#[test]
fn page_stays_within_the_viewport() {
    let css = css();
    let grid = rule(&css, ".pgrid").expect(".pgrid");
    assert!(
        !grid.contains("min-width: 280px"),
        "a fixed min width overflows at 360px: {grid}"
    );
    let card = rule(&css, ".pcard").expect(".pcard");
    assert!(
        card.contains("min-width: 0") || css.contains(".pgrid > *") || grid.contains("minmax(0"),
        "cards must be shrinkable"
    );
}
