import { useCallback, useEffect, useRef } from "react";

/**
 * Infinite scroll: call `loadMore` when the sentinel scrolls into view.
 *
 * Uses a callback ref (not a mount-time effect) so the observer attaches
 * even when the sentinel mounts later — e.g. after the first page loads.
 * Refs avoid stale closures; the sentinel stays mounted so loading more
 * never resets the grid (session state is preserved).
 */
export function useInfiniteLoader(
  hasMore: boolean,
  loading: boolean,
  loadMore: () => void
) {
  const state = useRef({ hasMore, loading, loadMore });
  state.current = { hasMore, loading, loadMore };

  const observer = useRef<IntersectionObserver | null>(null);
  useEffect(() => {
    observer.current = new IntersectionObserver(
      (entries) => {
        const s = state.current;
        if (entries.some((e) => e.isIntersecting) && s.hasMore && !s.loading) {
          s.loadMore();
        }
      },
      { rootMargin: "800px" }
    );
    const ob = observer.current;
    return () => ob?.disconnect();
  }, []);

  const sentinel = useCallback((el: HTMLDivElement | null) => {
    if (el) observer.current?.observe(el);
  }, []);
  return sentinel;
}
