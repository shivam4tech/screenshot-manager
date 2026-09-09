import { forwardRef, memo, useCallback, useEffect, useRef, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from "react";
import { Icons, type IconName } from "./icons";

/* ---------- Buttons ---------- */

type BtnVariant = "primary" | "secondary" | "ghost" | "danger" | "icon";
type BtnSize = "sm" | "md";

interface BtnProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: BtnVariant;
  size?: BtnSize;
  icon?: IconName;
}

export function Button({ variant = "secondary", size = "md", icon, children, className = "", ...rest }: BtnProps) {
  const Icon = icon ? Icons[icon] : null;
  return (
    <button className={`btn btn-${variant} btn-${size}${className ? ` ${className}` : ""}`} {...rest}>
      {Icon && <span className="btn-icon"><Icon size={15} /></span>}
      {children}
    </button>
  );
}

export function IconButton({ icon, label, className = "", ...rest }: { icon: IconName; label: string } & ButtonHTMLAttributes<HTMLButtonElement>) {
  const Icon = Icons[icon];
  return (
    <button className={`iconbtn${className ? ` ${className}` : ""}`} title={label} aria-label={label} {...rest}>
      <Icon size={16} />
    </button>
  );
}

/* ---------- Search ---------- */

interface SearchProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  kbd?: string;
}

export const SearchBar = forwardRef<HTMLInputElement, SearchProps>(function SearchBar({ kbd, className = "", ...rest }, ref) {
  return (
    <div className={`searchbar${className ? ` ${className}` : ""}`}>
      <span className="searchbar-icon"><Icons.search size={15} /></span>
      <input ref={ref} type="search" className="searchbar-input" {...rest} />
      {kbd && <kbd className="searchbar-kbd">{kbd}</kbd>}
    </div>
  );
});

/* ---------- Filter pill ---------- */

export function FilterChip({ icon, label, active, children }: { icon?: IconName; label: string; active?: boolean; children: ReactNode }) {
  const Icon = icon ? Icons[icon] : null;
  return (
    <label className={`fchip${active ? " fchip-active" : ""}`}>
      {Icon && <span className="fchip-icon"><Icon size={13} /></span>}
      <span className="fchip-label">{label}</span>
      {children}
    </label>
  );
}

export const FilterSelect = memo(function FilterSelect({ ariaLabel, value, onChange, options }: {
  ariaLabel: string;
  value: string;
  onChange: (v: string) => void;
  options: Array<{ value: string; label: string }>;
}) {
  return (
    <select className="fchip-select" aria-label={ariaLabel} value={value} onChange={(e) => onChange(e.target.value)}>
      {options.map((o) => (
        <option key={o.value} value={o.value}>{o.label}</option>
      ))}
    </select>
  );
});

/* ---------- Dropdown (custom menu, one unified control) ---------- */

export interface DropdownOption { value: string; label: string; }

export function Dropdown({ value, options, onChange, ariaLabel, prefix, active, placeholder, disabled }: {
  value: string;
  options: DropdownOption[];
  onChange: (v: string) => void;
  ariaLabel: string;
  prefix?: string;
  active?: boolean;
  /** Fixed trigger label for action-menus (selection doesn't stick). */
  placeholder?: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open ]);

  const current = options.find((o) => o.value === value);
  const isActive = active ?? (value !== "" && options.length > 0 && value !== options[0].value);

  return (
    <div className="dd" ref={ref}>
      <button
        className={`dd-trigger${isActive ? " dd-active" : ""}`}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
        disabled={disabled}
        onClick={() => setOpen((o) => !o)}
      >
        {prefix && <span className="dd-prefix">{prefix}</span>}
        <span className="dd-label">{placeholder ?? current?.label ?? value}</span>
        <span className="dd-caret"><Icons.chevronD size={13} /></span>
      </button>
      {open && (
        <ul className="dd-menu" role="listbox" aria-label={ariaLabel}>
          {options.map((o) => (
            <li key={o.value} role="presentation">
              <button
                role="option"
                aria-selected={o.value === value}
                className={`dd-item${o.value === value ? " selected" : ""}`}
                onClick={() => { onChange(o.value); setOpen(false); }}
              >
                <span className="dd-item-label">{o.label}</span>
                {o.value === value && <span className="dd-check"><Icons.check size={13} /></span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

/* ---------- States ---------- */

export function EmptyState({ title, body, action }: { title: string; body?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <h2>{title}</h2>
      {body && <p>{body}</p>}
      {action && <div className="empty-action">{action}</div>}
    </div>
  );
}

export function SkeletonCard() {
  return (
    <figure className="shot-card" aria-hidden="true">
      <div className="shot-thumb shimmer" />
      <div className="shot-sk-line" />
    </figure>
  );
}

/* ---------- Badges / chips ---------- */

export function CountBadge({ n }: { n: number | string }) {
  return <span className="count-badge">{n}</span>;
}

export function TagPill({ label, tone = "default" }: { label: string; tone?: "default" | "accent" | "danger" }) {
  return <span className={`tagpill tagpill-${tone}`}>{label}</span>;
}

/* ---------- Toggle switch ---------- */

export function Toggle({ checked, onChange, label, disabled }: {
  checked: boolean;
  onChange: () => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      title={label}
      disabled={disabled}
      className={`switch${checked ? " on" : ""}`}
      onClick={onChange}
    >
      <span className="switch-knob" />
    </button>
  );
}

/* ---------- Toasts ---------- */

export interface ToastAction { label: string; fn: () => void; }
export interface ToastItem { id: number; message: string; action?: ToastAction; }

let toastSeq = 1;

export function useToasts() {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const timers = useRef(new Map<number, ReturnType<typeof setTimeout>>());

  const dismiss = useCallback((id: number) => {
    setToasts((t) => t.filter((x) => x.id !== id));
    const tm = timers.current.get(id);
    if (tm) { clearTimeout(tm); timers.current.delete(id); }
  }, []);

  const push = useCallback((message: string, action?: ToastAction) => {
    const id = toastSeq++;
    setToasts((t) => [...t.slice(-3), { id, message, action }]);
    timers.current.set(id, setTimeout(() => dismiss(id), 6000));
  }, [dismiss]);

  const pause = useCallback((id: number) => {
    const tm = timers.current.get(id);
    if (tm) { clearTimeout(tm); timers.current.delete(id); }
  }, []);

  const resume = useCallback((id: number) => {
    timers.current.set(id, setTimeout(() => dismiss(id), 2500));
  }, [dismiss]);

  useEffect(() => () => timers.current.forEach((tm) => clearTimeout(tm)), []);

  return { toasts, push, dismiss, pause, resume };
}

export function ToastStack({ toasts, onClose, onPause, onResume }: {
  toasts: ToastItem[];
  onClose: (id: number) => void;
  onPause: (id: number) => void;
  onResume: (id: number) => void;
}) {
  if (toasts.length === 0) return null;
  return (
    <div className="toast-stack" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className="toast"
          onMouseEnter={() => onPause(t.id)}
          onMouseLeave={() => onResume(t.id)}
        >
          <span className="toast-msg">{t.message}</span>
          {t.action && (
            <button
              className="toast-action"
              onClick={() => { t.action!.fn(); onClose(t.id); }}
            >
              {t.action.label}
            </button>
          )}
          <button className="toast-x" onClick={() => onClose(t.id)} aria-label="Dismiss">✕</button>
        </div>
      ))}
    </div>
  );
}

/* ---------- Confirm dialog (replaces native confirm()) ---------- */

export interface ConfirmOptions {
  title: string;
  body: string;
  confirmLabel?: string;
  danger?: boolean;
}

export function useConfirm() {
  const [req, setReq] = useState<null | (ConfirmOptions & { resolve: (v: boolean) => void })>(null);

  const confirm = useCallback(
    (opts: ConfirmOptions) =>
      new Promise<boolean>((resolve) => setReq({ ...opts, resolve })),
    []
  );

  const settle = useCallback(
    (v: boolean) => setReq((r) => {
      r?.resolve(v);
      return null;
    }),
    []
  );

  // Escape dismisses as cancel (parents skip their own Esc while open).
  useEffect(() => {
    if (!req) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") settle(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [req, settle]);

  const node = req && (
    <div className="detail-backdrop confirm-backdrop" onClick={() => settle(false)} role="alertdialog" aria-modal="true" aria-label={req.title}>
      <div className="confirm-panel" onClick={(e) => e.stopPropagation()}>
        <h3>{req.title}</h3>
        <p className="muted">{req.body}</p>
        <div className="confirm-actions">
          <Button size="sm" variant="ghost" onClick={() => settle(false)} autoFocus>
            Cancel
          </Button>
          <Button
            size="sm"
            variant={req.danger ? "danger" : "primary"}
            onClick={() => settle(true)}
          >
            {req.confirmLabel ?? "Confirm"}
          </Button>
        </div>
      </div>
    </div>
  );

  return { confirm, confirmNode: node };
}

/* ---------- Shortcuts overlay ---------- */

const SHORTCUTS: Array<[string, string]> = [
  ["Ctrl K", "Focus search"],
  ["← → ↑ ↓", "Move between screenshots"],
  ["Enter", "Open focused screenshot"],
  ["Esc", "Back / close dialog"],
  ["Ctrl A", "Select all screenshots"],
  ["Delete", "Move selection to trash"],
  ["Ctrl ,", "Open settings"],
  ["?", "Show this panel"],
];

export function ShortcutsOverlay({ onClose }: { onClose: () => void }) {
  return (
    <div className="detail-backdrop" onClick={onClose} role="dialog" aria-modal="true" aria-label="Keyboard shortcuts">
      <div className="shortcuts-panel" onClick={(e) => e.stopPropagation()}>
        <div className="detail-head">
          <h3>Keyboard shortcuts</h3>
          <IconButton icon="x" label="Close shortcuts" onClick={onClose} />
        </div>
        <dl className="shortcuts-list">
          {SHORTCUTS.map(([keys, desc]) => (
            <div className="shortcuts-row" key={keys}>
              <dt>
                {keys.split(" ").map((k, i) => (
                  <kbd className="kbd" key={i}>{k}</kbd>
                ))}
              </dt>
              <dd>{desc}</dd>
            </div>
          ))}
        </dl>
      </div>
    </div>
  );
}
