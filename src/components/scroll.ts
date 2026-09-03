import { useEffect, useRef } from "react";

/**
 * Infinite scroll: call `loadMore` when the sentinel scrolls into view.
 * Refs avoid stale closures; the sentinel stays mounted so loading more
 * never resets the grid (session state is preserved).
 */
export function useInfiniteLoader(
  hasMore: boolean,
  loading: boolean,
  loadMore: () => void
) {
  const sentinel = useRef<HTMLDivElement | null>(null);
  const state = useRef({ hasMore, loading, loadMore });
  state.current = { hasMore, loading, loadMore };

  useEffect(() => {
    const el = sentinel.current;
    if (!el) return;
    const ob = new IntersectionObserver(
      (entries) => {
        const s = state.current;
        if (entries.some((e) => e.isIntersecting) && s.hasMore && !s.loading) {
          s.loadMore();
        }
      },
      { rootMargin: "800px" }
    );
    ob.observe(el);
    return () => ob.disconnect();
  }, []);

  return sentinel;
}
