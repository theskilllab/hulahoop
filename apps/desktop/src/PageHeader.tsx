import Icon from "./Icon";

/**
 * Every page opens the same way: a title, a hairline, and the search field on the
 * right. The row is also the window's drag handle, because the title bar is an
 * overlay and a frameless-feeling window still has to move.
 */
export default function PageHeader({
  title,
  back,
  subtitle,
  children,
  actions,
}: {
  title: string;
  back?: { label: string; onClick: () => void };
  subtitle?: string;
  children?: React.ReactNode;
  actions?: React.ReactNode;
}) {
  return (
    <header className="page-head" data-tauri-drag-region>
      {back ? (
        <button className="ghost small page-back" onClick={back.onClick}>
          {back.label}
        </button>
      ) : null}
      <h1 data-tauri-drag-region>
        {title}
        {subtitle || children ? (
          <small data-tauri-drag-region>{subtitle ?? children}</small>
        ) : null}
      </h1>
      <div className="page-tools">
        <button
          className="page-search"
          onClick={() => window.dispatchEvent(new CustomEvent("hulahoop:search"))}
          title="Search (⌘K)"
        >
          <Icon name="search" size={13} />
          <span>Search</span>
          <kbd>⌘K</kbd>
        </button>
        {actions}
      </div>
    </header>
  );
}
