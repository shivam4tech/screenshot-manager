import { memo } from "react";
import { Icons } from "./icons";

export interface CardItem {
  id: number;
  filename: string;
  created_ts: number | null;
  status: string;
  content_hash: string | null;
  starred: boolean;
  snippet?: string | null;
}

export function dateLabel(ts: number | null) {
  if (!ts) return "";
  return new Date(ts * 1000).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/**
 * Presentation-only shortening: every screenshot is a screenshot, so drop
 * the repetitive "Screenshot From " prefix for scanning. Files untouched;
 * the full name stays in the tooltip.
 */
export function displayName(filename: string): string {
  return filename.replace(/^screenshot from /i, "");
}

function Snippet({ text }: { text: string }) {
  const parts = text.split(/(\[[^\]]*\])/g);
  return (
    <>
      {parts.map((p, i) =>
        p.startsWith("[") && p.endsWith("]") ? <b key={i}>{p.slice(1, -1)}</b> : p
      )}
    </>
  );
}

interface Props {
  row: CardItem;
  thumbUrl: string | undefined;
  selected: boolean;
  onOpen: (id: number) => void;
  onToggleSelect: (id: number) => void;
  onToggleStar?: (id: number, next: boolean) => void;
  selectable?: boolean;
  /** Ref callback so grids can implement arrow-key roving focus. */
  cardRef?: (id: number, el: HTMLElement | null) => void;
  extraAction?: React.ReactNode;
}

export const ScreenshotCard = memo(function ScreenshotCard({
  row, thumbUrl, selected, onOpen, onToggleSelect, onToggleStar, selectable = true, cardRef, extraAction,
}: Props) {
  return (
    <figure
      className={`shot-card${selected ? " selected" : ""}`}
      title={row.filename}
      tabIndex={0}
      role="button"
      aria-label={`${displayName(row.filename)}, ${dateLabel(row.created_ts)}${row.status !== "available" ? `, ${row.status}` : ""}${row.starred ? ", starred" : ""}`}
      ref={(el) => cardRef?.(row.id, el)}
      onClick={() => onOpen(row.id)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || (e.key === " " && e.target === e.currentTarget)) {
          e.preventDefault();
          onOpen(row.id);
        }
      }}
    >
      <div className="shot-thumb">
        {thumbUrl ? (
          <img src={thumbUrl} alt={row.filename} loading="lazy" draggable={false} />
        ) : (
          <div className="shot-thumb-empty" aria-hidden="true">
            <Icons.image size={22} />
          </div>
        )}
        {selectable && (
          <span className="shot-check" onClick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              checked={selected}
              onChange={() => onToggleSelect(row.id)}
              aria-label={`Select ${row.filename}`}
            />
          </span>
        )}
        {onToggleStar && (
          <button
            className={`shot-star${row.starred ? " on" : ""}`}
            title={row.starred ? "Unstar" : "Star"}
            aria-label={`${row.starred ? "Unstar" : "Star"} ${row.filename}`}
            aria-pressed={row.starred}
            onClick={(e) => { e.stopPropagation(); onToggleStar(row.id, !row.starred); }}
          >
            ★
          </button>
        )}
        {row.status !== "available" && (
          <span className={`shot-badge badge-${row.status}`}>{row.status}</span>
        )}
        {extraAction}
      </div>
      <figcaption className="shot-meta">
        <span className="shot-name">{displayName(row.filename)}</span>
        {row.snippet ? (
          <span className="shot-snippet"><Snippet text={row.snippet} /></span>
        ) : (
          <span className="shot-date">{dateLabel(row.created_ts)}</span>
        )}
      </figcaption>
    </figure>
  );
});
