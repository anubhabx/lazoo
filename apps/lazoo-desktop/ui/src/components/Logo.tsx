/** The Lazoo mark: an L and a cursor block on a 10 by 10 grid. The cursor blinks while work is live. */
export function LogoMark({
  size = 16,
  live = false,
}: {
  size?: number;
  live?: boolean;
}) {
  return (
    <svg
      className={`logo-mark${live ? " logo-mark--live" : ""}`}
      width={size}
      height={size}
      viewBox="3 3 10 10"
      shapeRendering="crispEdges"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M3 3h3v7h3v3H3z" fill="currentColor" />
      <path
        className="logo-mark__cursor"
        d="M10 5h3v8h-3z"
        fill="var(--lz-color-accent)"
      />
    </svg>
  );
}
