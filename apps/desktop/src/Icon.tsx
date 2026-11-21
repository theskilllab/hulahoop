/// The sidebar icons, traced from the prototype's set: 20×20 box, 1.5 stroke,
/// round caps. One place, because a stray path char breaks silently.
const PATHS: Record<string, string> = {
  board: "M3.5 4.5h4.6v15H3.5zM9.6 4.5h4.6v9H9.6zM15.7 4.5h4.6v11h-4.6z",
  inbox: "M3.5 12.5 6 5.5h12l2.5 7v6h-17zM3.5 12.5h4l1 2h7l1-2h4",
  tasks: "M4.5 6.5h15M4.5 12h15M4.5 17.5h9",
  projects: "M3.5 6.5a2 2 0 0 1 2-2h3l2 2.5h7a2 2 0 0 1 2 2v8.5a2 2 0 0 1-2 2h-12a2 2 0 0 1-2-2z",
  milestones: "M6 3.5v17M6 5h10l-2 3.5 2 3.5H6",
  artifacts: "M5.5 3.5h7l6 6v11h-13zM12.5 3.5v6h6",
  agents: "M12 8.2a3.3 3.3 0 1 1 0 6.6 3.3 3.3 0 0 1 0-6.6zM4.5 19.5c1.4-3.2 4.1-4.8 7.5-4.8s6.1 1.6 7.5 4.8",
  skills: "M12 3.5l2.3 5 5.2.6-3.9 3.6 1.1 5.3L12 15.4 7.3 18l1.1-5.3L4.5 9.1l5.2-.6z",
  workflow: "M4.5 5.5h6v5h-6zM13.5 13.5h6v5h-6zM7.5 10.5v5h6",
  connectors: "M9 4.5v4M15 4.5v4M6.5 8.5h11v4a5.5 5.5 0 0 1-11 0zM12 18v2.5",
  cost: "M12 3.5v17M16 7.5c0-1.7-1.8-3-4-3s-4 1.3-4 3 1.8 2.6 4 3 4 1.3 4 3-1.8 3-4 3-4-1.3-4-3",
  pull: "M6.5 7.5h7a3 3 0 0 1 3 3v6M6.5 16.5v-13M4.2 10.2 6.5 7.9l2.3 2.3M14.2 14.2l2.3 2.3 2.3-2.3",
  activity: "M3.5 12.5h3.5l2.5-6 3 11 2.5-5h5.5",
  settings: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM12 3.5v2M12 18.5v2M4.9 7.8l1.7 1M17.4 15.2l1.7 1M4.9 16.2l1.7-1M17.4 8.8l1.7-1",
  plus: "M12 5.5v13M5.5 12h13",
  search: "M10.5 4.5a6 6 0 1 1 0 12 6 6 0 0 1 0-12zM15 15l4 4",
  close: "M6 6l12 12M18 6 6 18",
  chevron: "M9 6l6 6-6 6",
  dot: "M12 10.5a1.5 1.5 0 1 1 0 3 1.5 1.5 0 0 1 0-3z",
};

export default function Icon({ name, size = 16 }: { name: keyof typeof PATHS | string; size?: number }) {
  const path = PATHS[name] ?? PATHS.dot;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={path} />
    </svg>
  );
}
