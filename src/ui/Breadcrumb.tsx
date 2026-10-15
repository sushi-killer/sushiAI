/** The header of a page below a list: "Routines / Asana intake / Fri 17:00".
 * Every part but the last is a link back; the last is the page title. */
export function Breadcrumb({
  trail,
  current,
}: {
  trail: { label: string; onClick(): void }[];
  current: string;
}) {
  return (
    <nav className="ui-crumbs" aria-label="Breadcrumb">
      {trail.map((item, index) => (
        <span className="ui-crumb" key={`${index}:${item.label}`}>
          <button
            type="button"
            className="ui-crumb-link"
            title={item.label}
            onClick={item.onClick}
          >
            {item.label}
          </button>
          <span aria-hidden>/</span>
        </span>
      ))}
      <h1 title={current}>{current}</h1>
    </nav>
  );
}
