import type { IssueSummary, Milestone, Project } from "./api";
import { isLiveRun } from "./components/TaskBits";

/** The eight colours a project can wear, in the order a new project picks them. */
export const PROJECT_COLORS = [
  { name: "Blue", value: "#2f7ad8" },
  { name: "Teal", value: "#1f9d7a" },
  { name: "Orange", value: "#c2692b" },
  { name: "Violet", value: "#6a5acd" },
  { name: "Pink", value: "#d4558a" },
  { name: "Amber", value: "#b88100" },
  { name: "Red", value: "#d03b3b" },
  { name: "Slate", value: "#64748b" },
] as const;

const FINISHED = ["done", "cancelled"];

export interface ProjectStats {
  open: number;
  openPrs: number;
  running: number;
  /** Done tasks over all tasks, as a whole percent. */
  progress: number;
}

/** The numbers on a project card, from the tasks the workspace holds. */
export function projectStats(projectId: string | null, issues: IssueSummary[]): ProjectStats {
  const own = issues.filter((issue) => issue.projectId === projectId);
  const open = own.filter((issue) => !FINISHED.includes(issue.status));
  const done = own.filter((issue) => issue.status === "done").length;
  return {
    open: open.length,
    openPrs: open.filter((issue) => issue.prNumber !== null).length,
    running: own.filter((issue) => isLiveRun(issue.currentRun?.status)).length,
    progress: own.length ? Math.round((done / own.length) * 100) : 0,
  };
}

export function openMilestoneCount(milestones: Milestone[]): number {
  return milestones.filter((entry) => entry.projectId === null && entry.status === "open").length;
}

/** The first palette colour no project wears, or the palette's next turn when all are taken. */
export function firstUnusedColor(projects: Project[]): string {
  const free = PROJECT_COLORS.find((color) => !projects.some((project) => project.color === color.value));
  return (free ?? PROJECT_COLORS[projects.length % PROJECT_COLORS.length]).value;
}

/** `Bali Retreat` becomes `BAL`: letters and digits, upper case, starting with a letter. */
export function deriveKey(name: string): string {
  return name
    .replace(/[^a-z0-9]/gi, "")
    .replace(/^[0-9]+/, "")
    .slice(0, 3)
    .toUpperCase();
}

/** The message for a prefix nobody may use, or null. `others` are the other projects. */
export function validateKey(key: string, others: Project[]): string | null {
  if (!/^[A-Z][A-Z0-9]{1,4}$/.test(key)) return "Use 2 to 5 letters or numbers, starting with a letter.";
  if (others.some((project) => project.prefix === key)) return `${key} is already used by another project.`;
  return null;
}
