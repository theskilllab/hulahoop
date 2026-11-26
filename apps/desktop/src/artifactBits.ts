import type { Artifact, IssueSummary, Milestone } from "./api";
import type { SelectOption } from "./components/Select";

export const MAX_FILE_BYTES = 25 * 1024 * 1024;

export const ARTIFACT_TYPES: { value: Artifact["type"]; label: string; plural: string }[] = [
  { value: "plan", label: "Plan", plural: "Plans" },
  { value: "spec", label: "Spec", plural: "Specs" },
  { value: "report", label: "Report", plural: "Reports" },
  { value: "notes", label: "Notes", plural: "Notes" },
  { value: "file", label: "File", plural: "Files" },
];

export const typeLabel = (type: string) => ARTIFACT_TYPES.find((entry) => entry.value === type)?.label ?? type;

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** One "Belongs to" list: a Tasks group of `KEY · title`, then a Milestones group. Values are `issue:<id>` or `milestone:<id>`. */
export function ownerOptions(issues: IssueSummary[], milestones: Milestone[]): SelectOption[] {
  return [
    ...issues.map((issue) => ({ value: `issue:${issue.id}`, label: `${issue.key} · ${issue.title}`, group: "Tasks" })),
    ...milestones.map((milestone) => ({ value: `milestone:${milestone.id}`, label: milestone.title, group: "Milestones" })),
  ];
}

export function ownerIds(owner: string): { issueId: string | null; milestoneId: string | null } {
  const [kind, id] = owner.split(":");
  return { issueId: kind === "issue" ? id : null, milestoneId: kind === "milestone" ? id : null };
}

export const ownerOf = (artifact: Pick<Artifact, "issueId" | "milestoneId">) =>
  artifact.issueId ? `issue:${artifact.issueId}` : artifact.milestoneId ? `milestone:${artifact.milestoneId}` : "";
