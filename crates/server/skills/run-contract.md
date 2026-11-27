---
name: run-contract
description: Read before anything else in every turn. How a Hulahoop turn starts, what it may change, and how it ends so the server can read the result.
---

# Run contract

This is one turn. Nothing continues after your final message: no follow-up, no
second message, no waiting for an answer.

## What you were given

- `$HULAHOOP_INPUT` is a JSON file: the task, the repository, the limits, and —
  when this is a rework — the previous turn under `parent`. Read it first.
  Nothing else in the prompt describes the work.
- `files` lists the files attached to the task, each with an absolute `path`
  you can read. They are part of the brief: open the ones that matter.
- `comments` holds what people (and Lumi) wrote on the task since the previous
  turn started, oldest first. A comment can change the brief; read them all.
- Your checkout is a workspace of the repository, parked on the current base.
  Work there. `jj` records it; you do not commit or push by hand.
- The task keeps that checkout for all of its turns. Ignored build output
  (`target/`, `node_modules`, `.venv`) from an earlier turn is still there and
  makes builds faster, but the tracked files are reset to this turn's start.
  Do not rely on anything else left over: untracked scratch files from an
  earlier turn are gone, and an ignored file may be stale. Rebuild or
  regenerate what you depend on.
- When `repo` in the input is `null`, the task has no repository: your working
  directory is an empty scratch directory, private to this turn and deleted when
  it ends. See "No repository" below.

## House rules

- `direnv exec . just test` must pass before you finish, and so must the house
  rules the repository declares (`just fmt-check`, `just lint`). Run them.
- Do not edit `flake.nix`, `flake.lock`, `justfile` or `.envrc`. Those are the
  contract; a change there is a change to the loop itself.
- `just setup` has already run. If a dependency is genuinely missing, say so and
  stop.
- Run the whole suite rather than a subset. A passing subset says little.
- Stop only processes you started, by the PID you got when you started them
  (`kill <pid>`). Never use `pkill -f`, `killall` or another kill by name or
  pattern: other agents, the runner and the person's own shells run on the same
  machine, and a pattern such as `pkill -f "just test"` kills them too, your own
  agent process included. Wait for a long command to finish rather than killing
  it; if one hangs, say so in your final message.

## No repository

A task that belongs to no project has no repository. Then there is no checkout,
nothing to commit, no pull request, and no `just test`; the house rules above do
not apply. Work in the current directory as scratch space. Your final message is
the result: put the answer, findings or deliverable in it. A file a person needs
goes into `$HULAHOOP_ARTIFACT_DIR` (kept with the run) or is attached to the task
if your tools allow it; name it in the summary. End with the same last line as
any turn.

## Memory

Hulahoop keeps short notes across turns and tasks. When you learn something
that will still be true next week and would save a later turn from learning it
again, add one line to your closing message:

```
HULAHOOP_MEMORY: <scope> <kind> <title> — <body>
```

- `scope`: `workspace`, `project`, `agent` (you), `agent:<name>` or `person:<name>`.
- `kind`: `decision`, `convention`, `preference`, `fact` or `gotcha`.
- `title` at most 120 characters, `body` at most 600, separated by ` — `.

Save lasting lessons only: how this project behaves, how a person likes to work,
a decision already made. Never save task details or progress. At most three
lines count per turn, and a note that repeats an existing one updates it. A
reviewer who sends work back saves the general lesson behind it.

## How a turn ends

Your final message is the handover: the next agent, and the person reading the
task, see it as the turn's summary. Say what you did and why, name the files, and
quote the final result of `just test`.

If you changed files, name the commit on its own line before the last one:

```
HULAHOOP_COMMIT: Refuse to unlink a repository that has run history
```

That line is the commit's title, so make it say what *this turn* changed: an
imperative sentence of at most 72 characters, specific enough to tell this turn
from the others on the task. Skip the task key, prefixes like `implement:` or
`fix:`, and a trailing period. A turn that changed nothing leaves it out.

Then print exactly one of these as the last line:

```
HULAHOOP_VERDICT: done
```

when the work is complete — or, if your instructions ask you to judge the work
before you rather than add to it, name the judgement:

```
HULAHOOP_VERDICT: approved
HULAHOOP_VERDICT: changes_requested
```

`approved` hands the task on and `changes_requested` leaves it where it is for a
rework; that line is the review. Or:

```
HULAHOOP_QUESTION: <one sentence a person can answer>
```

when a decision only a person can make is blocking you. `done` with a failing
suite is a lie that costs a whole review cycle.
