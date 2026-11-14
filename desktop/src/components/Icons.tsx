// Line icons on a 16px grid, drawn for WatchSync. They follow currentColor.
const base = {
  width: 16,
  height: 16,
  viewBox: "0 0 16 16",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
};

/** Two play triangles, one behind the other: the WatchSync mark. */
export function Mark({ size = 20 }: { size?: number }) {
  return (
    <svg className="mark" width={size} height={size} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M4 5.6c0-1 1-1.6 1.9-1.1l8.4 5.1c.8.5.8 1.7 0 2.2l-8.4 5.1C5 17.4 4 16.8 4 15.8z" fill="currentColor" opacity="0.35" />
      <path d="M9 5.6c0-1 1-1.6 1.9-1.1l8.4 5.1c.8.5.8 1.7 0 2.2l-8.4 5.1c-.9.5-1.9-.1-1.9-1.1z" fill="currentColor" />
    </svg>
  );
}

export const PlayIcon = () => (
  <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
    <path d="M5.5 3.8c0-.8.9-1.3 1.6-.9l7.6 4.6c.7.4.7 1.4 0 1.8l-7.6 4.6c-.7.4-1.6-.1-1.6-.9z" fill="currentColor" />
  </svg>
);

export const PauseIcon = () => (
  <svg width="18" height="18" viewBox="0 0 18 18" aria-hidden="true">
    <rect x="4.5" y="3.5" width="3" height="11" rx="1" fill="currentColor" />
    <rect x="10.5" y="3.5" width="3" height="11" rx="1" fill="currentColor" />
  </svg>
);

/** Circular arrow with "10", backwards or forwards. */
export function Skip10({ back }: { back?: boolean }) {
  return (
    <svg width="22" height="22" viewBox="0 0 22 22" fill="none" aria-hidden="true">
      <g transform={back ? undefined : "translate(22 0) scale(-1 1)"}>
        <path d="M6 5.8A7 7 0 1 1 4 11" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        <path d="M6.4 2.6 6 5.8l3.2.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
      </g>
      <text x="11" y="14" textAnchor="middle" fontSize="6.5" fontWeight="600" fill="currentColor" fontFamily="Geist Variable, system-ui">
        10
      </text>
    </svg>
  );
}

export const CopyIcon = () => (
  <svg {...base}>
    <rect x="5.5" y="5.5" width="8" height="8" rx="1.6" />
    <path d="M10.5 3.2V3a1 1 0 0 0-1-1H3.5a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.2" />
  </svg>
);

export const CheckIcon = () => (
  <svg {...base}>
    <path d="m3.5 8.5 3 3 6-7" />
  </svg>
);

export const ArrowUpIcon = () => (
  <svg {...base}>
    <path d="M8 13V3.5M4 7.5l4-4 4 4" />
  </svg>
);

export const LeaveIcon = () => (
  <svg {...base}>
    <path d="M9.5 3H4a1 1 0 0 0-1 1v8a1 1 0 0 0 1 1h5.5M7 8h6.5M11 5.5 13.5 8 11 10.5" />
  </svg>
);

export const FilmIcon = () => (
  <svg {...base}>
    <rect x="2.5" y="3" width="11" height="10" rx="1.6" />
    <path d="M5.5 3v10M10.5 3v10M2.5 6h3M2.5 10h3M10.5 6h3M10.5 10h3" />
  </svg>
);
