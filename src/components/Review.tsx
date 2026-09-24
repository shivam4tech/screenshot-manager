import { useCallback, useEffect, useState } from "react";
import {
  api,
  thumbnailUrl,
  type CollectionInfo,
  type Proposal,
} from "../api";
import Cull, { type CullItem } from "./Cull";
import Detail from "./Detail";
import { Icons } from "./icons";
import { Button, EmptyState, useConfirm, type ToastAction } from "./ui";

/** Expanded groups load full rows (capped) so Cull + prev/next work on them. */
const MEMBER_LOAD_CAP = 100;

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
  onNotify: (msg: string, action?: ToastAction) => void;
}) {
  const [proposals, setProposals] = useState<Proposal[]>([]);
  const [suggestedCount, setSuggestedCount] = useState(0);
  const [firstSuggestedId, setFirstSuggestedId] = useState<number | null>(null);
  const [previews, setPreviews] = useState<Map<string, string>>(new Map());
  const [drafts, setDrafts] = useState<Map<string, string>>(new Map());
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");
  const [creating, setCreating] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [members, setMembers] = useState<Map<string, CullItem[]>>(new Map());
  const [memberThumbs, setMemberThumbs] = useState<Map<number, string>>(new Map());
  const [loadingMembers, setLoadingMembers] = useState(false);
  const [cullingKey, setCullingKey] = useState<string | null>(null);
  const [detail, setDetail] = useState<{ key: string; id: number } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { confirm, confirmNode } = useConfirm();

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

  /** Load full member rows for an expanded proposal (Cull + prev/next). */
  const loadMembers = useCallback(
    async (p: Proposal) => {
      setLoadingMembers(true);
      try {
        const rows: CullItem[] = [];
        for (const id of p.member_ids.slice(0, MEMBER_LOAD_CAP)) {
          const d = await api.getScreenshot(id);
          if (!d) continue;
          rows.push({ id, filename: d.filename, content_hash: d.content_hash });
        }
        setMembers((m) => new Map(m).set(p.key, rows));
        const fresh = new Map<number, string>();
        for (const r of rows) {
          if (memberThumbs.has(r.id)) continue;
          const url = await thumbnailUrl(r.content_hash, 256);
          if (url) fresh.set(r.id, url);
        }
        if (fresh.size > 0) setMemberThumbs((m) => new Map([...m, ...fresh]));
        return rows;
      } catch (e) {
        setError(String(e));
        return [];
      } finally {
        setLoadingMembers(false);
      }
    },
    [memberThumbs]
  );

  const toggleExpand = (p: Proposal) => {
    if (expanded === p.key) {
      setExpanded(null);
      return;
    }
    setExpanded(p.key);
    if (!members.has(p.key)) void loadMembers(p);
  };

  const commitEdit = (key: string) => {
    const v = editValue.trim();
    if (v) setDrafts((m) => new Map(m).set(key, v));
    setEditingKey(null);
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

  /** One click from the action row: load members if needed, then triage. */
  const startCull = async (p: Proposal) => {
    let rows = members.get(p.key);
    if (!rows) rows = await loadMembers(p);
    if (rows.length > 0) setCullingKey(p.key);
  };

  /** Trash every screenshot in the group (confirm first, undo offered). */
  const deleteGroup = async (p: Proposal) => {
    const name = drafts.get(p.key) ?? cleanName(p.name);
    const ok = await confirm({
      title: `Trash all ${p.member_count} screenshots in “${name}”?`,
      body: "Files go to the OS trash (recoverable) and the suggestion is dismissed.",
      confirmLabel: "Trash all",
      danger: true,
    });
    if (!ok) return;
    try {
      const s = await api.deleteScreenshots(p.member_ids);
      const trashed = p.member_ids.filter((id) => !s.failed.some((f) => f.id === id));
      await api.recordSuggestionFeedback(null, `proposal:${p.key}`, "dismiss");
      if (trashed.length > 0) {
        onNotify(
          `${trashed.length} screenshot${trashed.length === 1 ? "" : "s"} moved to trash`,
          {
            label: "Undo",
            fn: () => {
              void (async () => {
                await api.restoreScreenshots(trashed);
                await reload();
                refreshOrganize();
                refreshSuggestions();
              })();
            },
          }
        );
      }
      if (s.failed.length > 0) setError(s.failed[0].message);
      refreshOrganize();
      refreshSuggestions();
      await reload();
    } catch (e) {
      setError(String(e));
    }
  };

  const afterCull = async (key: string, trashedIds: number[]) => {
    setCullingKey(null);
    if (trashedIds.length > 0) {
      setMembers((m) => {
        const next = new Map(m);
        next.set(key, (next.get(key) ?? []).filter((r) => !trashedIds.includes(r.id)));
        return next;
      });
      onNotify(
        `${trashedIds.length} screenshot${trashedIds.length === 1 ? "" : "s"} moved to trash`,
        {
          label: "Undo",
          fn: () => {
            void (async () => {
              await api.restoreScreenshots(trashedIds);
              const p = proposals.find((x) => x.key === key);
              if (p) await loadMembers(p);
              await reload();
              refreshOrganize();
              refreshSuggestions();
            })();
          },
        }
      );
    }
    refreshOrganize();
    refreshSuggestions();
    await reload();
  };

  const detailRows = detail ? (members.get(detail.key) ?? []) : [];
  const detailIndex = detail ? detailRows.findIndex((r) => r.id === detail.id) : -1;

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
            const rows = members.get(p.key) ?? [];
            return (
              <li key={p.key} className="review-card">
                <div className="review-main">
                  <button
                    className="review-strip-btn"
                    onClick={() => toggleExpand(p)}
                    aria-expanded={isOpen}
                    title={isOpen ? "Collapse members" : "Expand to view members"}
                  >
                    <span className="burst-previews review-strip" aria-hidden="true">
                      {p.preview_hashes.slice(0, 4).map((h, i) =>
                        h && previews.has(h) ? (
                          <img key={i} src={previews.get(h)} alt="" loading="lazy" />
                        ) : (
                          <span key={i} className="burst-preview-empty" />
                        )
                      )}
                    </span>
                    <Icons.chevronR size={16} className={isOpen ? "review-chevron open" : "review-chevron"} />
                  </button>
                  <div className="review-info">
                    {editingKey === p.key ? (
                      <span className="review-edit">
                        <input
                          className="review-name-input"
                          value={editValue}
                          autoFocus
                          aria-label="Edit suggested collection name"
                          onChange={(e) => setEditValue(e.target.value)}
                          onKeyDown={(e) => {
                            e.stopPropagation();
                            if (e.key === "Enter") commitEdit(p.key);
                            else if (e.key === "Escape") setEditingKey(null);
                          }}
                        />
                        <button
                          className="icon-btn"
                          title="Save name"
                          aria-label="Save name"
                          onClick={() => commitEdit(p.key)}
                        >
                          <Icons.check size={14} />
                        </button>
                        <button
                          className="icon-btn"
                          title="Cancel"
                          aria-label="Cancel editing"
                          onClick={() => setEditingKey(null)}
                        >
                          <Icons.x size={14} />
                        </button>
                      </span>
                    ) : (
                      <span className="review-title">
                        <span className="review-title-text" title={p.reasons.join(" · ")}>
                          {draft}
                        </span>
                        <button
                          className="icon-btn review-edit-btn"
                          title="Rename this suggestion"
                          aria-label={`Rename suggestion ${draft}`}
                          onClick={() => {
                            setEditValue(draft);
                            setEditingKey(p.key);
                          }}
                        >
                          <Icons.pen size={13} />
                        </button>
                      </span>
                    )}
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
                      disabled={loadingMembers}
                      onClick={() => void startCull(p)}
                      title="Keyboard triage this group: → keep, x trash, u undo"
                    >
                      Cull
                    </button>
                    <button
                      className="icon-btn"
                      title={`Trash all ${p.member_count} screenshots in this group`}
                      aria-label={`Trash all screenshots in ${draft}`}
                      onClick={() => void deleteGroup(p)}
                    >
                      <Icons.trash size={14} />
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
                  <div className="review-open">
                    <div className="review-open-bar">
                      <span className="muted small">
                        {rows.length > 0
                          ? `${rows.length} shown${p.member_count > rows.length ? ` of ${p.member_count}` : ""} — click any shot to walk through with ← →`
                          : loadingMembers ? "Loading members…" : ""}
                      </span>
                    </div>
                    <div className="review-members">
                      {rows.map((r) => (
                        <button
                          key={r.id}
                          className="review-member"
                          onClick={() => setDetail({ key: p.key, id: r.id })}
                          title={`Open ${r.filename}`}
                        >
                          {memberThumbs.has(r.id) ? (
                            <img src={memberThumbs.get(r.id)} alt="" loading="lazy" />
                          ) : (
                            <span className="review-member-empty" aria-hidden="true" />
                          )}
                        </button>
                      ))}
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {cullingKey && (members.get(cullingKey) ?? []).length > 0 && (
        <Cull
          items={members.get(cullingKey) ?? []}
          onDone={(trashed) => void afterCull(cullingKey, trashed)}
        />
      )}

      {detail && detailIndex >= 0 && (
        <Detail
          id={detail.id}
          onClose={() => setDetail(null)}
          onChanged={() => {
            const p = proposals.find((x) => x.key === detail.key);
            if (p) void loadMembers(p);
            void reload();
            refreshOrganize();
            refreshSuggestions();
          }}
          onNotify={onNotify}
          onPrev={
            detailIndex > 0
              ? () => setDetail({ key: detail.key, id: detailRows[detailIndex - 1].id })
              : undefined
          }
          onNext={
            detailIndex >= 0 && detailIndex < detailRows.length - 1
              ? () => setDetail({ key: detail.key, id: detailRows[detailIndex + 1].id })
              : undefined
          }
          position={`${detailIndex + 1} / ${detailRows.length}`}
        />
      )}
      {confirmNode}
    </div>
  );
}
