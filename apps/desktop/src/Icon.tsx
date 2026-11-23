/// The icon set is the design handoff's own: 24-unit paths, 1.75 stroke, round
/// caps. Naming follows the prototype so a screen reads like the file it came from.
const PATHS: Record<string, string> = {
  inbox: '<path d="M3 13l2.5-7h13L21 13v6H3z"/><path d="M3 13h5l1.5 2.5h5L16 13h5"/>',
  tasks: '<rect x="4" y="4" width="16" height="16" rx="3"/><path d="M8.5 12.5l2.5 2.5 4.5-5"/>',
  folder: '<path d="M3 7a2 2 0 012-2h4l2 2h8a2 2 0 012 2v8a2 2 0 01-2 2H5a2 2 0 01-2-2z"/>',
  flag: '<path d="M5 21V4M5 4h11l-2 4 2 4H5"/>',
  file: '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/>',
  bot: '<rect x="4" y="8" width="16" height="11" rx="3"/><path d="M12 4v4M9 13v1.5M15 13v1.5"/>',
  book: '<path d="M5 4.5A1.5 1.5 0 016.5 3H19v15H6.5A1.5 1.5 0 005 19.5z"/><path d="M5 19.5A1.5 1.5 0 006.5 21H19v-3"/>',
  flow: '<rect x="3.5" y="4" width="6" height="5" rx="1.5"/><rect x="14.5" y="15" width="6" height="5" rx="1.5"/><path d="M9.5 6.5h3a2 2 0 012 2V15"/>',
  plug: '<path d="M9 3v5M15 3v5M6 8h12v3a6 6 0 01-12 0zM12 17v4"/>',
  coin: '<circle cx="12" cy="12" r="8.5"/><path d="M14.6 9.6c-.5-.8-1.5-1.3-2.6-1.3-1.5 0-2.6.8-2.6 2s1.1 1.7 2.6 2 2.6.8 2.6 2-1.1 2-2.6 2c-1.1 0-2.1-.5-2.6-1.3M12 6.2v2.1M12 15.7v2.1"/>',
  pr: '<circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M6 8.5v7M18 15.5V10a3 3 0 00-3-3h-3"/>',
  activity: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2.5l1.3 2.4a7.5 7.5 0 012.4 1l2.6-.8 1.9 3.3-2 1.8a7.5 7.5 0 010 2.6l2 1.8-1.9 3.3-2.6-.8a7.5 7.5 0 01-2.4 1L12 21.5l-1.3-2.4a7.5 7.5 0 01-2.4-1l-2.6.8-1.9-3.3 2-1.8a7.5 7.5 0 010-2.6l-2-1.8 1.9-3.3 2.6.8a7.5 7.5 0 012.4-1z"/>',
  search: '<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.2-4.2"/>',
  sidebar: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M9.5 4.5v15"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  close: '<path d="M6 6l12 12M18 6L6 18"/>',
  chevron: '<path d="M9 6l6 6-6 6"/>',
  chevdown: '<path d="M6 9l6 6 6-6"/>',
  updown: '<path d="M8 10l4-4 4 4M8 14l4 4 4-4"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
  download: '<path d="M12 4v11M7 11l5 5 5-5M5 20h14"/>',
  alert: '<path d="M12 4l9 16H3z"/><path d="M12 10v4M12 17v.5"/>',
  msg: '<path d="M4 5h16v11H9l-5 4z"/>',
  play: '<path d="M8 5.5v13l11-6.5z"/>',
  refresh: '<path d="M20 11a8 8 0 00-14-4L4 9M4 5v4h4M4 13a8 8 0 0014 4l2-2M20 19v-4h-4"/>',
  branch: '<circle cx="6" cy="5" r="2.5"/><circle cx="6" cy="19" r="2.5"/><circle cx="18" cy="9" r="2.5"/><path d="M6 7.5v9M18 11.5c0 4-12 2-12 5"/>',
  spark:
    '<circle cx="12" cy="12" r="8"/><circle cx="12" cy="12" r="2.6" fill="currentColor" stroke="none"/>',
  expand: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  shrink: '<path d="M9 4v5H4M15 4v5h5M9 20v-5H4M15 20v-5h5"/>',
};

export type IconName = keyof typeof PATHS;

export default function Icon({ name, size = 16 }: { name: string; size?: number }) {
  const path = PATHS[name] ?? PATHS.flow;
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      dangerouslySetInnerHTML={{ __html: path }}
    />
  );
}
