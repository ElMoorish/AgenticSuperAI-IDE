/** The ASA mark: a hexagon with a sharp "A" and a live dot. Colours follow the active theme. */
export function Brand() {
  return (
    <span className="brand" data-tauri-drag-region>
      <svg className="logo" viewBox="0 0 32 32" fill="none" aria-hidden="true" data-tauri-drag-region>
        <defs>
          <linearGradient id="asa-logo-g" x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" style={{ stopColor: "var(--accent)" }} />
            <stop offset="1" style={{ stopColor: "var(--accent-2)" }} />
          </linearGradient>
        </defs>
        <path d="M16 2.5 28.5 9.5v13L16 29.5 3.5 22.5v-13z" stroke="url(#asa-logo-g)" strokeWidth="2.4" strokeLinejoin="round" />
        <path d="M9.5 21.5 16 9l6.5 12.5" stroke="var(--text)" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
        <path d="M13 17.5h6" stroke="var(--accent)" strokeWidth="2" strokeLinecap="round" />
        <circle cx="16" cy="22" r="1.8" fill="var(--ok)" />
      </svg>
      <span data-tauri-drag-region>ASA</span>
    </span>
  );
}
