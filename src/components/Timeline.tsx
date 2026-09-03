import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type ScreenshotRow,
  type TimelineDay,
  type TimelineMonth,
} from "../api";
import { useInfiniteLoader } from "./scroll";

const PAGE_SIZE = 60;

function monthLabel(m: TimelineMonth): string {
  return new Date(m.year, m.month - 1, 1).toLocaleString(undefined, {
    month: "long",
    year: "numeric",
  });
}

function dayLabel(date: string): string {
  const d = new Date(`${date}T12:00:00`);
  return isNaN(d.getTime())
    ? date
    : d.toLocaleDateString(undefined, {
        weekday: "short",
        month: "short",
        day: "numeric",
      });
}

/**
 * Timeline browser: month strip → day list → day grid. Drill-down over the
 * existing index; clicking a shot opens the detail overlay.
 */
export default function Timeline({ onOpenDetail }: { onOpenDetail: (id: number) => void }) {
  const [months, setMonths] = useState<TimelineMonth[]>([]);
  const [monthKey, setMonthKey] = useState<string | null>(null);
  const [days, setDays] = useState<TimelineDay[]>([]);
  const [date, setDate] = useState<string | null>(null);
  const [items, setItems] = useState<ScreenshotRow[]>([]);
  const [thumbs, setThumbs] = useState<Map<number, string>>(new Map());
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const resolveThumbs = useCallback(async (rows: ScreenshotRow[]) => {
    const fresh = new Map<number, string>();
    for (const row of rows) {
      const url = await thumbnailUrl(row.content_hash, 512);
      if (url) fresh.set(row.id, url);
    }
    setThumbs((m) => new Map([...m, ...fresh]));
  }, []);

  useEffect(() => {
    api
      .timelineMonths()
      .then(setMonths)
      .catch((e) => setError(String(e)));
  }, []);

  const selectMonth = (m: TimelineMonth) => {
    setMonthKey(m.key);
    setDate(null);
    setItems([]);
    setError(null);
    api
      .timelineDays(m.year, m.month)
      .then(setDays)
      .catch((e) => setError(String(e)));
  };

  const loadDay = useCallback(
    async (day: string, offset: number) => {
      setLoading(true);
      try {
        const page = await api.timelineItems(day, PAGE_SIZE, offset);
        setHasMore(page.length === PAGE_SIZE);
        setItems((r) => (offset === 0 ? page : [...r, ...page]));
        void resolveThumbs(page);
      } finally {
        setLoading(false);
      }
    },
    [resolveThumbs]
  );

  const selectDay = (d: TimelineDay) => {
    setDate(d.date);
    setItems([]);
    setError(null);
    loadDay(d.date, 0).catch((e) => setError(String(e)));
  };

  const loadMoreNext = useCallback(() => {
    if (date) loadDay(date, items.length).catch((e) => setError(String(e)));
  }, [date, items.length, loadDay]);
  const sentinel = useInfiniteLoader(date !== null && hasMore, loading, loadMoreNext);

  return (
    <div className="timeline">
      {error && <p className="error">{error}</p>}
      {months.length === 0 && !error ? (
        <div className="empty-state">
          <h2>No dated screenshots.</h2>
          <p>Shots with capture dates will appear here by month.</p>
        </div>
      ) : (
        <>
          <div className="chip-strip" role="tablist" aria-label="Months">
            {months.map((m) => (
              <button
                key={m.key}
                role="tab"
                aria-selected={monthKey === m.key}
                className={`chip-btn${monthKey === m.key ? " active" : ""}`}
                onClick={() => selectMonth(m)}
              >
                {monthLabel(m)} <span className="side-count">{m.count}</span>
              </button>
            ))}
          </div>

          {monthKey && (
            <div className="chip-strip" aria-label="Days">
              {days.map((d) => (
                <button
                  key={d.date}
                  className={`chip-btn${date === d.date ? " active" : ""}`}
                  onClick={() => selectDay(d)}
                >
                  {dayLabel(d.date)} <span className="side-count">{d.count}</span>
                </button>
              ))}
            </div>
          )}

          {date && (
            <>
              <div className="grid">
                {items.map((r) => (
                  <figure
                    key={r.id}
                    className="cell clickable"
                    title={r.filename}
                    onClick={() => onOpenDetail(r.id)}
                  >
                    <div className="thumb-box">
                      {thumbs.has(r.id) ? (
                        <img src={thumbs.get(r.id)} alt={r.filename} loading="lazy" />
                      ) : (
                        <div className="thumb-placeholder" aria-hidden="true" />
                      )}
                      {r.starred && (
                        <span className="star-badge" title="Starred">
                          ★
                        </span>
                      )}
                    </div>
                    <figcaption>{r.filename}</figcaption>
                  </figure>
                ))}
              </div>
              <div ref={sentinel} className="scroll-sentinel" aria-hidden="true">
                {loading ? "Loading…" : !hasMore && items.length > 0 ? "End." : ""}
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}
