import Icon from "./Icon";

/**
 * Every page opens the same way: a title, the fact that qualifies it, and the
 * search field on the right. The row is also the window's drag handle, because
 * the title bar is an overlay and a frameless-feeling window still has to move.
 *
 * One component, one height for its controls (`--tb-h`), and one place to put a
 * page's own buttons: `actions`. A page that inflates the bar (a sentence for a
 * subtitle, a control taller than 24px) is what makes two screens look unalike.
 */
export default function PageHeader({
  title,
  back,
  size,
  subtitle,
  actions,
  ownSearch,
}: {
  title: string;
  back?: { label: string; onClick: () => void };
  /// A task's own page is the only one the prototype sets large.
  size?: "big";
  /// The fact beside the title: a count, a state, a source. Keep it short.
  subtitle?: string;
  actions?: React.ReactNode;
  /// The page brings its own search field, so the ⌘K door is left out.
  ownSearch?: boolean;
}) {
  return (
    <header
      className={size === "big" ? "page-head big" : "page-head"}
      data-drag
    >
      {back ? (
        <button className="ghost small page-back" onClick={back.onClick}>
          {back.label}
        </button>
      ) : null}
      <h1 data-drag aria-label={title}>
        {title}
        {subtitle ? <small data-drag>{subtitle}</small> : null}
      </h1>
      <div className="page-tools">
        {actions}
        {ownSearch ? null : (
          <button
            className="page-search"
            onClick={() =>
              window.dispatchEvent(new CustomEvent("hulahoop:search"))
            }
            title="Search (⌘K)"
          >
            <Icon name="search" size={13} />
            <span>Search</span>
            <kbd>⌘K</kbd>
          </button>
        )}
      </div>
    </header>
  );
}
