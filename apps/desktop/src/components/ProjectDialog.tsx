import { useEffect, useId, useMemo, useRef, useState } from "react";
import {
  addRepo,
  createProject,
  fetchRepo,
  loadGithubRepos,
  setBaseBookmark,
  setProjectPrefix,
  unlinkRepo,
  updateProject,
  type GithubRepo,
  type IssueSummary,
  type NewRepo,
  type Project,
  type Workspace,
} from "../api";
import Icon from "../Icon";
import { deriveKey, firstUnusedColor, PROJECT_COLORS, validateKey } from "../projectStats";

/** A repository in the dialog: `id` is set once it exists on the server. */
interface DraftRepo extends NewRepo {
  id?: string;
}

const repoName = (url: string) => url.trim().split("/").filter(Boolean).pop()?.replace(/\.git$/, "") ?? "repo";

/**
 * New project / Project settings: a modal with the project's name, task prefix,
 * colour and linked repositories. Nothing is saved until the footer button, so
 * Cancel leaves the project as it was.
 */
export default function ProjectDialog({
  workspace,
  project,
  projects,
  issues,
  reload,
  notify,
  onClose,
}: {
  workspace: Workspace;
  /** Null creates a project. */
  project: Project | null;
  projects: Project[];
  issues: IssueSummary[];
  /** Reads the projects again, after a save or a save that failed half way. */
  reload: () => Promise<void>;
  notify: (message: string) => void;
  onClose: () => void;
}) {
  const ids = { name: useId(), key: useId(), keyHint: useId() };
  const dialog = useRef<HTMLDialogElement>(null);
  const others = useMemo(() => projects.filter((entry) => entry.id !== project?.id), [projects, project]);
  const locked = project !== null && issues.some((issue) => issue.projectId === project.id);
  const [name, setName] = useState(project?.name ?? "");
  const [key, setKey] = useState(project?.prefix ?? "");
  const [keyEdited, setKeyEdited] = useState(project !== null);
  const [color, setColor] = useState(project?.color ?? firstUnusedColor(projects));
  const [repos, setRepos] = useState<DraftRepo[]>(
    project?.repos.map((repo) => ({ id: repo.id, name: repo.name, remoteUrl: repo.remoteUrl ?? "", baseBookmark: repo.baseBookmark })) ?? [],
  );
  const [tried, setTried] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.showModal();
    return () => {
      dialog.current?.close();
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  const nameError = name.trim() ? null : "Give the project a name.";
  const keyError = locked ? null : validateKey(key, others);
  const shownNameError = tried ? nameError : null;
  const shownKeyError = tried || keyEdited ? keyError : null;

  function changeName(value: string) {
    setName(value);
    if (!keyEdited) setKey(deriveKey(value));
  }

  async function save() {
    setTried(true);
    if (busy || nameError || keyError) return;
    setBusy(true);
    setError(null);
    const title = name.trim();
    try {
      if (project) await saveChanges(project, title);
      else await createProject(workspace.id, { name: title, prefix: key, color, repos });
      await reload();
      notify(project ? `Saved “${title}”` : `Created project “${title}”`);
      onClose();
    } catch (cause) {
      // An edit can fail after earlier ones landed; show where things stand.
      if (project) await reload().catch(() => undefined);
      setError(cause instanceof Error ? cause.message : String(cause));
      setBusy(false);
    }
  }

  async function saveChanges(current: Project, title: string) {
    if (title !== current.name || color !== current.color) await updateProject(current.id, title, color);
    if (!locked && key !== current.prefix) await setProjectPrefix(current.id, key);
    for (const repo of current.repos) {
      if (!repos.some((entry) => entry.id === repo.id)) await unlinkRepo(repo.id);
    }
    for (const repo of repos) {
      const before = current.repos.find((entry) => entry.id === repo.id);
      if (!before) await addRepo(current.id, repo);
      else if (before.baseBookmark !== repo.baseBookmark) await setBaseBookmark(before.id, repo.baseBookmark);
    }
  }

  return (
    <dialog
      ref={dialog}
      id="pjDlg"
      aria-labelledby="pj-title"
      className="settings-dialog pj-dialog"
      onCancel={(event) => {
        event.preventDefault();
        if (!busy) onClose();
      }}
    >
      <form
        className="settings-form"
        noValidate
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div className="settings-head">
          <h2 id="pj-title">{project ? "Project settings" : "New project"}</h2>
          <button type="button" className="icon-btn" aria-label="Close" onClick={onClose} disabled={busy}>
            <Icon name="close" size={14} />
          </button>
        </div>
        <div className="settings-content">
          <div className="field">
            <label className="field-label" htmlFor={ids.name}>
              Name
            </label>
            <input
              id={ids.name}
              type="text"
              autoFocus
              value={name}
              aria-invalid={shownNameError !== null}
              onChange={(event) => changeName(event.target.value)}
            />
            {shownNameError ? <p className="pj-error">{shownNameError}</p> : null}
          </div>
          <div className="field">
            <label className="field-label" htmlFor={ids.key}>
              Task ID prefix
            </label>
            <input
              id={ids.key}
              className="pj-prefix mono"
              type="text"
              maxLength={5}
              value={key}
              disabled={locked}
              aria-invalid={shownKeyError !== null}
              aria-describedby={ids.keyHint}
              onChange={(event) => {
                setKey(event.target.value.toUpperCase());
                setKeyEdited(true);
              }}
            />
            {shownKeyError ? <p className="pj-error">{shownKeyError}</p> : null}
            <p className="pj-hint" id={ids.keyHint}>
              {locked
                ? `Existing tasks keep this prefix, like ${key}-1.`
                : `Tasks will be numbered ${key || "KEY"}-1, ${key || "KEY"}-2 and so on. Pick something short you will recognise.`}
            </p>
          </div>
          <div className="field">
            <span className="field-label" id="pj-color-label">
              Color
            </span>
            <ColorPicker value={color} onChange={setColor} />
            <p className="pj-hint">
              Shown beside the project name in filters, lists, milestones and artifacts, so you can tell projects apart at a glance.
            </p>
          </div>
          <RepoLinker repos={repos} projects={projects} self={project} onChange={setRepos} />
          {error ? (
            <p className="error" role="alert">
              {error}
            </p>
          ) : null}
        </div>
        <div className="settings-actions">
          <button type="button" className="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="submit" className="primary" disabled={busy}>
            {project ? "Save changes" : "Create project"}
          </button>
        </div>
      </form>
    </dialog>
  );
}

function ColorPicker({ value, onChange }: { value: string; onChange: (color: string) => void }) {
  return (
    <div className="pj-swatches" role="radiogroup" aria-labelledby="pj-color-label">
      {PROJECT_COLORS.map((color) => (
        <button
          key={color.value}
          type="button"
          role="radio"
          aria-checked={value === color.value}
          aria-label={color.name}
          className="pj-swatch"
          style={{ background: color.value }}
          onClick={() => onChange(color.value)}
        >
          {value === color.value ? <Icon name="check" size={12} /> : null}
        </button>
      ))}
    </div>
  );
}

/** The linked repositories, the search panel that adds more, and the per-repository base bookmark. */
function RepoLinker({
  repos,
  projects,
  self,
  onChange,
}: {
  repos: DraftRepo[];
  projects: Project[];
  self: Project | null;
  onChange: (repos: DraftRepo[]) => void;
}) {
  const [panel, setPanel] = useState(false);
  const [query, setQuery] = useState("");
  const [local, setLocal] = useState("");
  const [available, setAvailable] = useState<GithubRepo[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [fetchState, setFetchState] = useState<string | null>(null);

  useEffect(() => {
    if (!panel) return;
    let live = true;
    loadGithubRepos()
      .then((found) => live && setAvailable(found))
      .catch((cause: Error) => live && setLoadError(cause.message));
    return () => {
      live = false;
    };
  }, [panel]);

  const linked = (url: string) => repos.some((repo) => repo.remoteUrl === url);
  const usedBy = (url: string) =>
    projects.find((entry) => entry.id !== self?.id && entry.repos.some((repo) => repo.remoteUrl === url))?.name;
  const unlinked = available.filter((entry) => !linked(entry.cloneUrl));
  const matches = unlinked.filter((entry) => entry.fullName.toLowerCase().includes(query.trim().toLowerCase()));

  function link(repo: DraftRepo) {
    onChange([...repos, repo]);
    setPanel(false);
    setQuery("");
    setLocal("");
  }

  function edit(index: number, baseBookmark: string) {
    onChange(repos.map((repo, at) => (at === index ? { ...repo, baseBookmark } : repo)));
  }

  async function fetchBase(id: string) {
    setFetchState("Fetching…");
    try {
      await fetchRepo(id);
      setFetchState("Fetch queued.");
    } catch (cause) {
      setFetchState(cause instanceof Error ? cause.message : String(cause));
    }
  }

  return (
    <div className="field">
      <div className="pj-repos-head">
        <span className="field-label">Repositories</span>
        <span className="pj-hint">{repos.length} linked</span>
      </div>
      <ul className="pj-repolist" aria-label="Linked repositories">
        {repos.map((repo, index) => (
          <li key={repo.id ?? repo.remoteUrl} className="pj-repo">
            <div className="pj-repo-row">
              <Icon name="branch" size={13} />
              <button
                type="button"
                className="pj-repo-name mono truncate"
                aria-expanded={open === repo.remoteUrl}
                onClick={() => setOpen(open === repo.remoteUrl ? null : repo.remoteUrl)}
              >
                {repo.name}
              </button>
              <button
                type="button"
                className="icon-btn"
                aria-label={`Unlink ${repo.name}`}
                onClick={() => onChange(repos.filter((_, at) => at !== index))}
              >
                <Icon name="close" size={12} />
              </button>
            </div>
            {open === repo.remoteUrl ? (
              <div className="pj-repo-more">
                <p className="pj-hint truncate">{repo.remoteUrl}</p>
                <label className="field-label" htmlFor={`pj-base-${index}`}>
                  Base bookmark
                </label>
                <div className="pj-inline">
                  <input
                    id={`pj-base-${index}`}
                    type="text"
                    className="mono"
                    value={repo.baseBookmark}
                    onChange={(event) => edit(index, event.target.value)}
                  />
                  {repo.id ? (
                    <button type="button" onClick={() => void fetchBase(repo.id!)}>
                      Fetch base
                    </button>
                  ) : null}
                </div>
                {fetchState ? <p className="pj-hint">{fetchState}</p> : null}
              </div>
            ) : null}
          </li>
        ))}
        {repos.length === 0 ? <li className="pj-empty">No repositories linked yet.</li> : null}
      </ul>
      {panel ? (
        <div className="pj-panel">
          <input
            type="search"
            autoFocus
            aria-label="Search your GitHub repositories"
            placeholder="Search your GitHub repositories"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <ul className="pj-results">
            {matches.map((entry) => (
              <li key={entry.cloneUrl}>
                <span className="mono truncate">{entry.fullName}</span>
                {usedBy(entry.cloneUrl) ? <span className="chip">in {usedBy(entry.cloneUrl)}</span> : null}
                <button
                  type="button"
                  className="link"
                  onClick={() => link({ name: entry.name, remoteUrl: entry.cloneUrl, baseBookmark: entry.defaultBranch })}
                >
                  Link
                </button>
              </li>
            ))}
            {matches.length === 0 ? (
              <li className="pj-empty">{unlinked.length ? "No repositories match." : "Every repository is already linked."}</li>
            ) : null}
          </ul>
          {loadError ? <p className="pj-hint">{loadError}</p> : null}
          <div className="pj-inline">
            <input
              type="text"
              className="mono"
              aria-label="Add a local repository"
              placeholder="Add a local repository: a path or git url"
              value={local}
              onChange={(event) => setLocal(event.target.value)}
            />
            <button
              type="button"
              disabled={!local.trim() || linked(local.trim())}
              onClick={() => link({ name: repoName(local), remoteUrl: local.trim(), baseBookmark: "main" })}
            >
              Add
            </button>
          </div>
          <button type="button" className="link" onClick={() => setPanel(false)}>
            Cancel
          </button>
        </div>
      ) : (
        <button type="button" className="pj-link-btn" onClick={() => setPanel(true)}>
          <Icon name="plus" size={12} /> Link a repository
        </button>
      )}
    </div>
  );
}
