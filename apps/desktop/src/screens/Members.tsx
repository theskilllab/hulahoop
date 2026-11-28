import { useEffect, useState } from "react";
import {
  createInvite,
  loadMe,
  loadMembers,
  loadRegistration,
  removeMember,
  setMemberRole,
  setRegistration,
  type Me,
  type Member,
  type RegistrationMode,
  type Role,
} from "../api";

const MODES: { id: RegistrationMode; label: string; blurb: string }[] = [
  { id: "closed", label: "Closed", blurb: "Nobody can sign up. Owners can still invite people." },
  { id: "invite", label: "Invite only", blurb: "Sign-up needs an invite code from an owner." },
  { id: "open", label: "Open", blurb: "Anyone who can reach the server can sign up as a member." },
];

const messageOf = (cause: unknown) => String(cause instanceof Error ? cause.message : cause);

/** Settings → Registration: who may create an account. Owners only change it. */
function RegistrationBlock({ isOwner }: { isOwner: boolean }) {
  const [mode, setMode] = useState<RegistrationMode | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadRegistration().then(setMode).catch((cause) => setError(messageOf(cause)));
  }, []);

  async function choose(next: RegistrationMode) {
    setError(null);
    try {
      await setRegistration(next);
      setMode(next);
    } catch (cause) {
      setError(messageOf(cause));
    }
  }

  return (
    <div className="settings-block">
      <span className="field-label">Registration</span>
      <div className="run-opts" role="radiogroup" aria-label="Registration">
        {MODES.map((option) => (
          <label className={mode === option.id ? "run-opt on" : "run-opt"} key={option.id}>
            <input
              type="radio"
              name="registration"
              checked={mode === option.id}
              disabled={!isOwner || mode === null}
              onChange={() => void choose(option.id)}
            />
            <span className="stack">
              <strong>{option.label}</strong>
              <span className="dim small">{option.blurb}</span>
            </span>
          </label>
        ))}
      </div>
      {!isOwner && <p className="dim small">Only an owner can change this.</p>}
      {error && <p className="error" role="alert">{error}</p>}
    </div>
  );
}

/** Settings → Members: the people in the workspace, and the invite that adds one. */
export default function Members({ workspaceId }: { workspaceId: string }) {
  const [me, setMe] = useState<Me | null>(null);
  const [members, setMembers] = useState<Member[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [inviteRole, setInviteRole] = useState<Role>("member");
  const [invite, setInvite] = useState<string | null>(null);

  const reload = () => loadMembers(workspaceId).then(setMembers);

  useEffect(() => {
    Promise.all([loadMe(), loadMembers(workspaceId)])
      .then(([who, list]) => {
        setMe(who);
        setMembers(list);
      })
      .catch((cause) => setError(messageOf(cause)));
  }, [workspaceId]);

  async function act(change: () => Promise<unknown>) {
    setError(null);
    try {
      await change();
      await reload();
    } catch (cause) {
      setError(messageOf(cause));
    }
  }

  const isOwner = me?.role === "owner";

  return (
    <>
      <RegistrationBlock isOwner={isOwner} />
      <div className="settings-block">
        <span className="field-label">Members</span>
        <ul className="stack" aria-label="Members">
          {members.map((member) => (
            <li className="row" key={member.userId}>
              <span className="stack fill">
                <strong>{member.name}</strong>
                <span className="dim small">{member.email ?? "no email"}</span>
              </span>
              {isOwner ? (
                <>
                  <select
                    aria-label={`Role of ${member.name}`}
                    value={member.role}
                    onChange={(event) =>
                      void act(() => setMemberRole(workspaceId, member.userId, event.target.value as Role))
                    }
                  >
                    <option value="owner">Owner</option>
                    <option value="member">Member</option>
                  </select>
                  <button
                    type="button"
                    className="danger"
                    aria-label={`Remove ${member.name}`}
                    onClick={() => void act(() => removeMember(workspaceId, member.userId))}
                  >
                    Remove
                  </button>
                </>
              ) : (
                <span className="chip">{member.role}</span>
              )}
            </li>
          ))}
        </ul>
        {isOwner && (
          <div className="stack">
            <div className="row">
              <select aria-label="Role for the invite" value={inviteRole} onChange={(event) => setInviteRole(event.target.value as Role)}>
                <option value="member">Member</option>
                <option value="owner">Owner</option>
              </select>
              <button
                type="button"
                className="secondary"
                onClick={() => void createInvite(inviteRole).then(setInvite, (cause) => setError(messageOf(cause)))}
              >
                Create invite
              </button>
            </div>
            {invite && (
              <p className="small">
                Invite code (shown once, works once, expires in 7 days): <code className="mono">{invite}</code>
              </p>
            )}
          </div>
        )}
        {error && <p className="error" role="alert">{error}</p>}
      </div>
    </>
  );
}
