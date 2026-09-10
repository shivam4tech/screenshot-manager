/** Single icon family — inline Lucide-style SVGs, zero dependencies. */

type P = { size?: number; className?: string };

function base(size: number, children: React.ReactNode, className?: string) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.8}
      strokeLinecap="round"
      strokeLinejoin="round"
      className={className}
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

export const Icons = {
  grid: (p: P) => base(p.size ?? 16, <>
    <rect x="3" y="3" width="7" height="7" rx="1.5" />
    <rect x="14" y="3" width="7" height="7" rx="1.5" />
    <rect x="3" y="14" width="7" height="7" rx="1.5" />
    <rect x="14" y="14" width="7" height="7" rx="1.5" />
  </>, p.className),
  star: (p: P) => base(p.size ?? 16, <>
    <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
  </>, p.className),
  clock: (p: P) => base(p.size ?? 16, <>
    <circle cx="12" cy="12" r="9" /><polyline points="12 7 12 12 15.5 13.5" />
  </>, p.className),
  copy: (p: P) => base(p.size ?? 16, <>
    <rect x="9" y="9" width="12" height="12" rx="2" />
    <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
  </>, p.className),
  layers: (p: P) => base(p.size ?? 16, <>
    <polygon points="12 2 2 7 12 12 22 7 12 2" />
    <polyline points="2 12 12 17 22 12" />
  </>, p.className),
  zap: (p: P) => base(p.size ?? 16, <>
    <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
  </>, p.className),
  settings: (p: P) => base(p.size ?? 16, <>
    <circle cx="12" cy="12" r="3" />
    <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
  </>, p.className),
  folder: (p: P) => base(p.size ?? 16, <>
    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
  </>, p.className),
  search: (p: P) => base(p.size ?? 16, <>
    <circle cx="11" cy="11" r="7" /><line x1="21" y1="21" x2="16.5" y2="16.5" />
  </>, p.className),
  x: (p: P) => base(p.size ?? 16, <><line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" /></>, p.className),
  plus: (p: P) => base(p.size ?? 16, <><line x1="12" y1="5" x2="12" y2="19" /><line x1="5" y1="12" x2="19" y2="12" /></>, p.className),
  trash: (p: P) => base(p.size ?? 16, <>
    <polyline points="3 6 5 6 21 6" />
    <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
  </>, p.className),
  check: (p: P) => base(p.size ?? 16, <><polyline points="20 6 9 17 4 12" /></>, p.className),
  chevronR: (p: P) => base(p.size ?? 16, <><polyline points="9 18 15 12 9 6" /></>, p.className),
  chevronL: (p: P) => base(p.size ?? 16, <><polyline points="15 18 9 12 15 6" /></>, p.className),
  chevronD: (p: P) => base(p.size ?? 16, <><polyline points="6 9 12 15 18 9" /></>, p.className),
  dots: (p: P) => base(p.size ?? 16, <>
    <circle cx="5" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="12" cy="12" r="1.4" fill="currentColor" stroke="none" />
    <circle cx="19" cy="12" r="1.4" fill="currentColor" stroke="none" />
  </>, p.className),
  sun: (p: P) => base(p.size ?? 16, <>
    <circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" />
  </>, p.className),
  moon: (p: P) => base(p.size ?? 16, <><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" /></>, p.className),
  image: (p: P) => base(p.size ?? 16, <>
    <rect x="3" y="3" width="18" height="18" rx="2" />
    <circle cx="8.5" cy="8.5" r="1.5" /><polyline points="21 15 16 10 5 21" />
  </>, p.className),
  list: (p: P) => base(p.size ?? 16, <>
    <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
    <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
  </>, p.className),
  keyboard: (p: P) => base(p.size ?? 16, <>
    <rect x="2" y="6" width="20" height="12" rx="2" />
    <path d="M6 10h.01M10 10h.01M14 10h.01M18 10h.01M7 14h10" />
  </>, p.className),
  tag: (p: P) => base(p.size ?? 16, <>
    <path d="M20.6 13.4 11 3.8A2 2 0 0 0 9.6 3.2H4a1 1 0 0 0-1 1v5.6c0 .5.2 1 .6 1.4l9.6 9.6a2 2 0 0 0 2.8 0l4.6-4.6a2 2 0 0 0 0-2.8z" />
    <circle cx="7.5" cy="7.5" r="1" fill="currentColor" />
  </>, p.className),
  bookmark: (p: P) => base(p.size ?? 16, <><path d="M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" /></>, p.className),
  expand: (p: P) => base(p.size ?? 16, <><path d="M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7" /></>, p.className),
  fileText: (p: P) => base(p.size ?? 16, <>
    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
    <polyline points="14 2 14 8 20 8" /><line x1="16" y1="13" x2="8" y2="13" /><line x1="16" y1="17" x2="8" y2="17" />
  </>, p.className),
  drive: (p: P) => base(p.size ?? 16, <>
    <rect x="2" y="4" width="20" height="7" rx="2" />
    <rect x="2" y="13" width="20" height="7" rx="2" />
    <line x1="6" y1="7.5" x2="6.01" y2="7.5" />
    <line x1="6" y1="16.5" x2="6.01" y2="16.5" />
  </>, p.className),
};

export type IconName = keyof typeof Icons;
