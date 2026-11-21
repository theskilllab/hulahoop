# DESIGN.md: Hulahoop

A handover for the next agent. Read this before you touch `index.html`. It says what the product is, what the owner has already decided, how the file is built, and what breaks if you are careless.

## 1. What this is

**Hulahoop** is a Mac-first workspace where one person (or a small team) runs autonomous coding agents next to ordinary task management. Tasks, projects, milestones, artifacts, agents with live diffs and transcripts, GitHub pull requests, an inbox, cost control, and an always-available assistant called **Lumi**.

- It is a **prototype**: one self-contained file, `index.html`, about 3,800 lines of HTML, CSS and vanilla JS. All data is sample data held in memory. Nothing is persisted except a few `localStorage` keys (`hh-palette`, `hh-server`, `hh-recent`, `hh-mode`).
- The real app will be built with **Tauri**, and it should feel as native to macOS as possible. Every design choice leans that way: arrow cursors, no text selection on chrome, thin scrollbars, translucent sidebar, sheets that drop from the top, a simulated menu bar.
- Lumi and every agent reply are **rule-based and scripted**, not a real model. Do not describe them as intelligent in copy.
- Reference products the owner named: Paperclip, Multica, Pivotal Tracker (for the dense board).

Other files in the project: `app-icon/` (the final Mac icon as `.icns`, a 1024 PNG and the SVG source) and this file.

## 2. Principles the owner has repeated

1. **Simple, but everything in one place.** Prefer fewer controls. Remove before you add.
2. **Tight and dense.** The board is the reference for density. Small text, little padding, no wasted lines, no border around every card. Thin bottom hairline only.
3. **One size scale.** Every control height, text size, radius and icon size comes from a token. If you need a new size, change the scale, do not hard-code a pixel value.
4. **Native feel over web feel.** If a web default looks like a website, replace it.
5. **Quick.** Transitions are 80 to 130 ms. Nothing should feel like it animates for its own sake.
6. **Plain language, no AI tells.** See section 9.
7. **Do not undo earlier decisions** unless asked. Section 10 lists them.

## 3. Layout

```
+-------------------------------------------------------------+
| (simulated macOS menu bar, hidden when html.tauri)          |
+----------+--------------------------------------------------+
| sidebar  |  toolbar (.bar): title, controls, Search ⌘K      |
|  232px   +--------------------------------------------------+
|          |  main view (#main)             | #detail drawer  |
| nav      |                                | 80% overlay,    |
| items    |                                | maximize or dock|
| workspace|                                |                 |
| settings |                                |                 |
+----------+--------------------------------------------------+
                                              Ask Lumi pill / panel, bottom right
```

- Sidebar items, in order: Inbox, Tasks, Projects, Milestones, Artifacts, Agents, Skills, Workflow, Connectors, Cost control, Pull requests, Activity. The footer is the workspace selector (no background, no border), which opens a menu with Settings.
- The sidebar collapses to an icon rail. Counts sit inside the row with right padding so they never touch the edge.
- Toolbar is `--toolbar-h` 44px and must line up with the top bar of the task drawer. `bar(title, sub, controls)` builds it, and always appends the global Search button.
- The task drawer `#detail` floats at about 80% width over the page, with a maximize button and a dock-beside-page toggle. Esc closes it.
- Dialogs are `<dialog>` sheets that drop from the top (`inset: 0 0 auto 0`).

## 4. Design tokens (in `:root`, top of the `<style>`)

Change a token and every control in that group follows.

| Group | Tokens |
|---|---|
| Text | `--fs-2xs 10`, `--fs-xs 11`, `--fs-sm 12`, `--fs-md 13` (body), `--fs-lg 15`, `--fs-xl 18`, `--fs-title 20`, `--fs-stat 24` |
| Spacing | `--s1 4`, `--s2 8`, `--s3 12`, `--s4 16`, `--s5 24`, `--s6 32` |
| Control heights | `--h-chip 20`, `--h-compact 24` (toolbar, menu rows), `--h-control 28` (buttons, fields, pills), `--h-row 36` (list rows), `--toolbar-h 44`, `--menubar-h 28` |
| Corners | `--r-xs 4`, `--r-sm 6`, `--r-md 8`, `--r-lg 12`, `--r-pill` |
| Icons | `--icon-2xs 10`, `--icon-xs 12`, `--icon-sm 14`, `--icon-md 16`, `--icon-lg 18` |
| Motion | `--t-fast 80ms`, `--t-med 130ms` |
| Fonts | `--font-ui` (SF stack), `--font-mono` (SF Mono) |
| Colour | `--bg`, `--bg-2`, `--bg-3`, `--line`, `--text`, `--text-2`, `--muted`, `--accent`, `--accent-fg`, `--accent-soft`, `--accent-text`, `--ok`/`--ok-soft`, `--warn`/`--warn-soft`, `--bad`/`--bad-soft`, diff colours `--add-*`, `--del-*`, `--hunk-bg`, `--shadow` |

Button text weight is 500 (the owner compared 500 and 600 and chose 500). Toolbar controls share one height variable (`--tb-h`); buttons, selects, search and the segmented control must stay identical in height.

### Themes

`<html data-theme="light|dark" data-palette="...">`. The `THEMES` array drives the picker (one scrollable column, grouped):

- **Default**: Light, Dark, Match system (follows the Mac).
- **Rosé Pine**: Dawn (light), Rosé Pine (dark), Moon (dark).
- **Catppuccin**: Latte (light), Frappé, Macchiato, Mocha (dark).

Each theme is either light or dark. Theme choice is personal (Settings, Personal tab) and saved in `hh-palette`.

### Colours that do not follow the theme

- **The logo and app icon** are fixed: a filled circle, `#6f86d6`. They must not change with the theme.
- **Chart colours** use `--c1` to `--c6` (`#2a78d6`, `#eb6834`, `#1baf7a`, `#eda100`, `#e87ba4`, `#008300`) with a `.cc` scope. **Never name chart variables `--s1`..`--s6`.** That collided with the spacing tokens once and removed all padding from the app.

## 5. Components and where to find them

Search the file for the class name. Reuse before you create.

- **Buttons**: `.btn`, `.btn.primary`, `.btn.danger`, `.icon-btn`, `.linkbtn`. 28px tall. Primary is the accent colour.
- **Pill family.** The app has four pill-shaped patterns. They differ on purpose, so pick by job and do not invent a fifth:
  - **Status chip** `.chip` (+ `.ok`, `.warn`, `.bad`, `.info`, `.dashed`): 20px, filled, not clickable. Shows a state: Ready (`.ok`), Unplanned (`.dashed`, an outline with no fill), agent, Connected. The planning label on task cards and in the drawer is this chip, not a toggle.
  - **Property pill** `.pill`: 28px, outlined, wraps a `<select>` with an icon. Used for Project, Priority and Assigned to in New task.
  - **Filter pill** `.tabbar button` and Lumi's suggestion `.as-chip`: 28px, outlined. Single choice. The selected tab fills with the text colour (inverted).
  - **Toggle chip** `.tgl`, grouped in `.tgl-group`: 28px, outlined, **multi-select**. Pressed (`aria-pressed="true"`) fills with `--accent-soft`, an accent border and `--accent-text`. Used wherever you attach several things to one item: handlers in the workflow panel, skills on an agent, agents on a skill, agents on a new skill. Always give the group a visible label and `role="group"`.
- **Fields**: `.input`, `.select`, `.textarea`, `.field-label`, `.err` (inline error, hidden by default).
- **Segmented control**: `.seg` (use `aria-pressed` or `aria-selected`).
- **Cards (board)**: `.card`, flush, no gap between cards, 0.5px bottom hairline. Meta row has the ID, badges, a flexible gap, then chips, priority and avatar.
- **Avatars**: `av(id)` returns the right avatar. Agents get a rounded-square bot icon, people get initials, Lumi (`'ai'`) gets the sparkle icon.
- **Status icon**: `sg(status)`. Status colours: progress warn, blocked bad, review accent, done ok.
- **Dialogs**: `.dlg` inside `<dialog>`, footer `.dlg-f`. Pattern: open function fills fields and calls `showModal()`, a save function validates and closes.
- **Popovers**: `.sel-pop` with `popover="manual"`. Used by the select list (`#sel-pop`), the project filter (`#pf-pop`) and the mention list (`#mn-pop`). When a popover must work inside a modal dialog, append it to that dialog first.
- **Tables**: `.md table`, `.wf-table`, `.cc-table`.
- **Diagram**: workflow SVG uses `.wf-node`, `.wf-edge`, `.wf-lab`, `.wf-hit`.
- **Empty states**: `.empty` with a `<strong>` title and one line of help.
- **Toast**: `toast(msg)`.

### Native-feel layer

- Every `<select>` opens a custom searchable list (`SEL` state, `selOpen`). Add `data-native` to opt out.
- Context menu and the simulated menu bar share `#ctx-menu` and the `MB` object (including the tray status mock).
- Tauri hooks: `data-tauri-drag-region` on bars, `.tauri` class on `<html>` when `window.__TAURI__` exists (hides the fake menu bar).
- Shortcuts: ⌘N new task, ⌘1 to ⌘9 views (Inbox, Tasks, Projects, Milestones, Artifacts, Agents, Cost, PRs, Activity), ⌘, settings, ⌥⌘S sidebar, ⌘K palette, ⌘J Lumi. Workflow and Connectors have no number shortcut.

## 6. Domain model (state lives in one object, `S`)

- **Tasks** `S.tasks`: `id`, `t`, `s` (status), `p` (priority), `a` (assignee id), `proj`, `d` (markdown description), optional `due`, `pr`, `ms` (milestone), `cm` (comments), `runs`, `files`, `ready`, `plannedBy`, `blockedBy`.
- **Statuses** `ST`: Backlog, To do, In progress, **Blocked**, In review, Done. Blocked can be set by a person or an agent.
- **Planning label**, on Backlog and To do cards only: **Ready** (green) or **Unplanned** (dashed). Two states only. An edit after planning does not change it. Planning happens by mentioning an agent or Lumi in a comment with the word "plan", or by asking Lumi. Every plan has Undo.
- **Projects** `PROJECTS`: `id`, `name`, `color`, `key` (task ID prefix), `repos[]`. A virtual **No project** (`NOPROJ`, id `none`, prefix `TSK`) exists everywhere a project can be chosen. Milestone project values: a project id, `none`, or `all` (cross-project, only on the two old sample milestones; the New milestone dialog no longer offers it).
- **Agents** `S.agents`: `id`, `name`, `harness` (`claude`, `codex`, `pi`), `model`, `md` (AGENTS.md text), `skills` (ids from the shared library), `state` (running, review, blocked, idle), `task`, `branch`, `run` (live diff), `log`. Sample agents: Ada (a1), Bram (a2), Cole (a3), Dana (a4).
- **Skills** `S.skills`: a shared workspace library, each `{id, md}` where `md` is a SKILL.md with a `name:` and `description:` block at the top. Name and description are always read from that block (`skParse`), so there is one source of truth. A skill is attached to agents through `agent.skills`, and one skill can be on any number of agents. AGENTS.md is read on every run, a skill is loaded only when its description fits the task. Name must be lowercase hyphenated and unique.
- **Harnesses** `HARNESS`: Claude Code, Codex, Pi, each with a model list. A harness is available only if a connector that powers it is connected.
- **Connectors** `CONNECTORS` and `S.cn`: GitHub (GitHub App with App ID plus `.pem` key, or personal access token), Claude Code subscription, Claude API key, Codex subscription, Pi login. Helper functions: `cnOn`, `harnessOk`, `harnessVia`, `harnessNeeds`.
- **Workspaces** `S.workspaces`: `name`, `members`, `runner` (local or docker), `lumi` `{harness, model}`.
- **Workflow** `S.wf`: `src` (the WORKFLOWS.md text), `saved`, `view`, `sel`, `undo`. `wfParse(src)` turns the file into states and issues, `wfSvg` draws it, `wfEdit` rewrites the file from the form controls.
- **Other**: PRs, inbox, milestones, artifacts (owned by a task or a milestone, can hold uploaded files), `COSTRUNS` (seeded random data, 60 days), Lumi messages `S.as`.

### WORKFLOWS.md format

```
## Status name
Owner: you
Agent: Ada, Cole (optional note)        # or "any idle agent" / "any agent"
Next: Other status - when it moves
```
Each `##` heading is a status, `Owner:` or `Agent:` says who handles it, each `Next:` is an arrow. The page flags undefined targets, unreachable statuses, unknown agent names, and board columns missing from the file. Clicking a status or arrow opens a panel that edits the file for you and keeps every other line.

## 7. Screens and what each needs to keep doing

- **Inbox**: act in place (approve, request changes), with a link back to the ticket. Request changes lets you click diff lines to comment.
- **Tasks**: board or list. Project filter is multi-select with "No project". Done tasks are **not loaded by default** (`S.showDone`, an Active / All switch in the toolbar, with "+N" for the hidden done tasks). Anything that fetches tasks for the board or list must respect this. No separate filter box, because global search covers it.
- **Task drawer**: description is not boxed, rich markdown, tasks list with comments and agent runs (summary first, expand for everything), live transcript (follow the agent like a prompt), turns and tool-call counts per task and per run.
- **Agents**: roster left, live diff, transcript and log right. New agent dialog has name, harness, model (depends on harness), AGENTS.md.
- **Skills**: list on the left (name, description, which agents use it), editor on the right with a Used by toggle per agent and the SKILL.md text. New skill takes a SKILL.md (type it or drop a file) and can be attached to agents at creation. Deleting a skill removes it from every agent. New agent and Edit agent dialogs have a Skills chip row and a link to this page.
- **Workflow**: markdown on the left, state diagram on the right, checks and a transitions table below the diagram.
- **Connectors**: grouped list with Connect and Disconnect. Disconnecting removes harness availability and shows warnings on agents and the PR page.
- **Cost control**: tokens and cost over time, per agent, per model, failed runs, cost per task, subscription budgets versus API spend. Built to the dataviz rules: validated categorical palette, stacked columns, ranked bars, meters, and a table twin for every chart.
- **Settings** (one item in the workspace menu): Personal tab (theme) and Workspace tab (name, members, runner, Lumi harness and model).
- **Welcome / connect screen**: connect to this Mac or to a server. Simulated errors for hosts containing "offline" or "down", password `wrong`, token `invalid`.

## 8. How to edit and check your work

1. Make **small direct edits**. The file is large, so use targeted replacements. For anything over a few lines, write a small script that asserts each target string occurs exactly once before replacing it.
2. After every edit, syntax-check the script block: extract the last `<script>` and run `node --check` on it.
3. A stub-DOM run is worth doing for new views: run the script in a `vm` context with a proxy `document`, then call view functions (`tasksView()`, `wfOut()`, and so on) and assert on the HTML they return.
4. Say honestly in the hand-off that nothing was opened in a browser if that is the case.

### Traps that have already bitten

- **Name collisions.** Everything is a global in one script. `planFor(a, t)` already exists for agent plans, so Lumi's planner is `lumiPlanFor`. Search for a name before you define it.
- **Temporal dead zone.** `logoSvg` must be a hoisted `function`, because the welcome block runs before later `const` declarations. A `const` used early crashed the whole app once and hid the logo and Lumi.
- **Regex over-matching** deleted three icons from the icon dictionary once. Check `P` (icons) after bulk edits.
- **The `render()` function** replaces `#main` and the drawer. Anything that must keep focus or open state (popovers, typing) needs care. `render()` closes the mention list and re-renders Lumi.
- **`.md` inline rendering** also turns known `@names` into mention tags through `inline()`.
- **Dialog popovers**: a popover opened from inside a modal `<dialog>` must be a child of that dialog.

## 9. Voice and copy

- Plain, short, specific. Say what the thing does. Contractions in anything a person or agent "says" (`I'll`, `I'm`, `it doesn't`).
- No em dashes in UI copy (use a comma, colon or full stop). The only dash allowed is the empty-value dash in tables.
- Avoid: seamless, powerful, unlock, leverage, effortless, "Let's", "Here's", "honest". Avoid tidy triplets and bold-headed bullet lists in chat replies.
- Prefer concrete reasons over hedging ("a spreadsheet will try to run them" beats "this may cause issues").
- The sparkle icon is only for Lumi's own avatar and pill. Do not put it on buttons.
- Labels the owner picked: **Ask Lumi** (pill), **Plan** via comment (no Plan button), **Ready** and **Unplanned**, **Blocked**, **No project**, **Workflow**, **Connectors**, **Settings**.
- Error text is inline under the field (`.err`), never an alert.

## 10. Decisions already made (do not reverse without asking)

- Project list is **not** in the sidebar. Projects are a page and a task filter.
- The "New task" box has no title field. The first line becomes the title. Project, priority and assignee sit on one line at the bottom, right-aligned. Buttons: Create and Create and add another. Files are dragged in, with no Attach button (same for New artifact).
- Task drawer is a **floating 80% overlay** with maximize and a dock option.
- Board cards are flush with a thin bottom border only. Cards have top and bottom padding.
- No border or background on the workspace selector.
- The GitHub-sync button at the bottom left was replaced by the workspace selector.
- Dark mode lives under **Personal** settings, and themes are personal. The original dark theme must stay as "Dark" under Default.
- Menu bar icon is simple and colour coded: idle ring, running ring with dot, unread disc. **Unread beats running.** The menu bar window shows the newest unread items, and each item opens the right place.
- Lumi: bottom-right pill that expands to a panel. It proposes changes with checkboxes and applies after you press Apply, except for planning, which applies at once and offers Undo.
- Logo and icon: a plain filled circle in `#6f86d6`, same everywhere. The app icon is a white rounded square with the dot at about 53% of the tile width.
- Plan state is two-valued. "Plan outdated" was removed on purpose.
- The Lumi conversation on REL-110 is a deliberate sample of "ask Lumi to plan and give feedback".

## 11. Open items and suggested next steps

- Mentions are visual only. They do not yet notify an agent's transcript or the inbox.
- The saved workflow is a drawing and validator. It does not yet decide which agent may take which task.
- Agents cannot block tasks live. The blocked samples (REL-106 by a person, ATL-112 by Dana) are static.
- Lumi's harness and model setting only changes what the panel displays.
- Skills are not yet used by any run. Importing a skill from a GitHub repository or a folder, and per-harness checks that the skill format is supported, are not built.
- Everything is unverified in a real browser. First job for the next agent: open `index.html`, click through each screen in light and dark, and fix what is off.
- Tauri wiring, real GitHub and agent runners, and persistence are all still to build.
