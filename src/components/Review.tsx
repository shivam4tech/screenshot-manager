import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type Proposal,
} from "../api";
import { Button, EmptyState } from "./ui";

const MEMBER_PREVIEW_CAP = 24;

/** Proposal names carry a " · N" suffix for scanning; strip it for editing. */
function cleanName(name: string): string {
  return name.replace(/\s·\s\d+$/, "").trim() || name;
}

/**
 * Review: suggested groups waiting for one click. Each proposal becomes a
 * regular collection on approval (renamable now or later from the sidebar),
 * and later screenshots keep getting suggested into it from the detail view.
 * All previews are local thumbnails — nothing is fetched from the web.
 */
export default function Review({
  onOpenDetail,
  onOpenCollection,
  refreshOrganize,
  refreshSuggestions,
  onNotify,
}: {
  onOpenDetail: (id: number) => void;
  onOpenCollection: (c: CollectionInfo) => void;
  refreshOrganize: () => void;
  refreshSuggestions: () => void;
  onNotify: (msg: string) => void;
}) {
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [suggestedCount, setSuggestedCount] = useState(0);
  const [firstSuggestedId, setFirstSuggestedId] = useState<number | null>(null);
  const [previews, setPreviews] = useState<Map<string, string>>(new Map());
  const [drafts, setDrafts] = useState<Map<string, string>>(new Map());
  const [creating, setCreating] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [memberThumbs, setMemberThumbs] = useState<Map<number, string>>(new Map());
  const [loadingMembers, setLoadingMembers] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      const o = await api.suggestionOverview();
      setProposals(o.proposals);
      setSuggestedCount(o.suggested_count);
      setFirstSuggestedId(o.first_suggested_id);
      setDrafts((prev) => {
        const next = new Map(prev);
        for (const p of o.proposals) {
          if (!next.has(p.key)) next.set(p.key, cleanName(p.name));
        }
        return next;
      });
      const fresh = new Map<string, string>();
      for (const p of o.proposals) {
        for (const h of p.preview_hashes) {
          if (!h || fresh.has(h)) continue;
          const url = await thumbnailUrl(h, 256);
          if (url) fresh.set(h, url);
        }
      }
      setPreviews(fresh);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  /** Expand a proposal to preview its members (local thumbnails only). */
  const toggleExpand = async (p: Proposal) => {
    if (expanded === p.key) {
      setExpanded(null);
      return;
    }
    setExpanded(p.key);
    setLoadingMembers(true);
    try {
      const fresh = new Map<number, string>();
      for (const id of p.member_ids.slice(0, MEMBER_PREVIEW_CAP)) {
        if (memberThumbs.has(id)) continue;
        const d = await api.getScreenshot(id);
        if (!d) continue;
        const url = await thumbnailUrl(d.content_hash, 256);
        if (url) fresh.set(id, url);
      }
      if (fresh.size > 0) setMemberThumbs((m) => new Map([...m, ...fresh]));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingMembers(false);
    }
  };

  /** One click: the proposal becomes a regular collection. Typing an
      existing collection name files the shots into it instead. */
  const approve = async (p: Proposal) => {
    const name = (drafts.get(p.key) ?? cleanName(p.name)).trim();
    if (!name || creating) return;
    setCreating(p.key);
    setError(null);
    try {
      const c = await api.createProposedCollection(
        name,
        p.member_ids,
        JSON.stringify({ kind: "suggested", key: p.key, reasons: p.reasons }),
        p.key
      );
      refreshOrganize();
      refreshSuggestions();
      await reload();
      onNotify(`Collection “${c.name}” created with ${p.member_count} screenshots`);
      onOpenCollection(c);
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(null);
    }
  };

  const dismiss = async (p: Proposal) => {
    try {
      await api.recordSuggestionFeedback(null, `proposal:${p.key}`, "dismiss");
      refreshSuggestions();
      await reload();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="review">
      <div className="page-head">
        <div>
          <h1 className="page-title">Review</h1>
          <p className="page-sub">
            Suggested groups from your unfiled screenshots. Approve one to make it a
            regular collection — rename it now or anytime, and new matching
            screenshots keep getting suggested into it.
          </p>
        </div>
      </div>

      {suggestedCount > 0 && (
        <div className="suggest-banner" role="status">
          <span>
            <strong>{suggestedCount}</strong> screenshot{suggestedCount === 1 ? "" : "s"} match
            {suggestedCount === 1 ? "es" : ""} your existing collections and tags
          </span>
          <span className="suggest-banner-actions">
            {firstSuggestedId != null && (
              <button className="link-btn" onClick={() => onOpenDetail(firstSuggestedId)}>
                Open first match
              </button>
            )}
          </span>
        </div>
      )}

      {error && <p className="error">{error}</p>}

      {proposals.length === 0 ? (
        <EmptyState
          title="All caught up"
          body="No suggested groups right now. As new screenshots arrive, groups form here — and single matches keep appearing in the detail view."
        />
      ) : (
        <ul className="review-list">
          {proposals.map((p) => {
            const draft = drafts.get(p.key) ?? cleanName(p.name);
            const isOpen = expanded === p.key;
            return (
              <li key={p.key} className="review-card">
                <div className="review-main">
                  <span className="burst-previews review-strip" aria-hidden="true">
                    {p.preview_hashes.slice(0, 4).map((h, i) =>
                      h && previews.has(h) ? (
                        <img key={i} src={previews.get(h)} alt="" loading="lazy" />
                      ) : (
                        <span key={i} className="burst-preview-empty" />
                      )
                    )}
                  </span>
                  <div className="review-info">
                    <input
                      className="review-name"
                      value={draft}
                      aria-label="Collection name for this suggestion"
                      onChange={(e) =>
                        setDrafts((m) => new Map(m).set(p.key, e.target.value))
                      }
                      onKeyDown={(e) => {
                        e.stopPropagation();
                        if (e.key === "Enter") void approve(p);
                      }}
                    />
                    <span className="muted small">
                      {p.member_count} screenshots · {p.reasons.slice(0, 2).join(" · ")}
                    </span>
                  </div>
                  <div className="review-actions">
                    <Button
                      size="sm"
                      disabled={!draft.trim() || creating === p.key}
                      onClick={() => void approve(p)}
                      title={`Create collection with ${p.member_count} screenshots`}
                    >
                      {creating === p.key ? "Adding…" : "Add as collection"}
                    </Button>
                    <button
                      className="btn btn-sm"
                      onClick={() => void toggleExpand(p)}
                      aria-expanded={isOpen}
                      title={isOpen ? "Hide members" : "Preview members"}
                    >
                      {isOpen ? "Hide" : "Preview"}
                    </button>
                    <button
                      className="icon-btn"
                      title="Dismiss this suggestion"
                      aria-label={`Dismiss suggestion ${p.name}`}
                      onClick={() => void dismiss(p)}
                    >
                      ✕
                    </button>
                  </div>
                </div>
                {isOpen && (
                  <div className="review-members">
                    {loadingMembers && memberThumbs.size === 0 ? (
                      <p className="muted small">Loading members…</p>
                    ) : (
                      p.member_ids.slice(0, MEMBER_PREVIEW_CAP).map((id) =>
                        memberThumbs.has(id) ? (
                          <button
                            key={id}
                            className="review-member"
                            onClick={() => onOpenDetail(id)}
                            title={`Open screenshot ${id}`}
                          >
                            <img src={memberThumbs.get(id)} alt="" loading="lazy" />
                          </button>
                        ) : (
                          <span key={id} className="review-member-empty" aria-hidden="true" />
                        )
                      )
                    )}
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
