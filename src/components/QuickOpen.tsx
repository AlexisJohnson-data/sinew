import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { Icon } from "@iconify/react";
import { api } from "../lib/ipc";
import { fuzzyRank, type FuzzyMatch } from "../lib/fuzzy";
import type { WorkspaceEntry } from "../types";

const MAX_RESULTS = 60;
const MAX_RECENTS = 12;
const RECENTS_STORAGE_PREFIX = "sinew:quick-open:recents:";

type Props = {
  open: boolean;
  workspacePath: string;
  /** Bumped whenever the workspace file tree might have changed; we
   *  use it to invalidate the local file-list cache. */
  refreshToken: number;
  onPick: (
    entry: WorkspaceEntry,
    line?: { number: number; column?: number },
  ) => void;
  onClose: () => void;
};

type Row = { entry: WorkspaceEntry; match: FuzzyMatch };

function recentsKey(workspacePath: string): string {
  return `${RECENTS_STORAGE_PREFIX}${workspacePath}`;
}

function loadRecents(workspacePath: string): string[] {
  try {
    const raw = window.localStorage.getItem(recentsKey(workspacePath));
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((value): value is string => typeof value === "string");
  } catch {
    return [];
  }
}

function pushRecent(workspacePath: string, relativePath: string): void {
  try {
    const current = loadRecents(workspacePath);
    const next = [relativePath, ...current.filter((p) => p !== relativePath)];
    if (next.length > MAX_RECENTS) next.length = MAX_RECENTS;
    window.localStorage.setItem(recentsKey(workspacePath), JSON.stringify(next));
  } catch {
    // Ignore storage failures; recents are nice-to-have, not load-bearing.
  }
}

/**
 * Parse a trailing `:line` or `:line:col` suffix and return the cleaned
 * query plus the desired editor position. Unrecognised suffixes are left
 * attached to the query so the user can still match files whose name
 * happens to contain a colon (rare, but possible).
 */
function splitQueryAndLine(raw: string): {
  query: string;
  line?: { number: number; column?: number };
} {
  const match = /^(.+?):(\d+)(?::(\d+))?\s*$/.exec(raw);
  if (!match) return { query: raw };
  const number = parseInt(match[2]!, 10);
  if (!Number.isFinite(number) || number < 1) return { query: raw };
  const colRaw = match[3];
  const col = colRaw ? parseInt(colRaw, 10) : NaN;
  return {
    query: match[1]!.trim(),
    line: { number, column: Number.isFinite(col) ? col : undefined },
  };
}

/**
 * Render a path with the fuzzy-matched characters highlighted. The
 * `positions` are indices into the raw `path` string in ascending order.
 */
function HighlightedPath({
  path,
  positions,
}: {
  path: string;
  positions: ReadonlyArray<number>;
}) {
  if (positions.length === 0) return <>{path}</>;
  const chunks: Array<{ text: string; highlight: boolean }> = [];
  let cursor = 0;
  for (const pos of positions) {
    if (pos > cursor) chunks.push({ text: path.slice(cursor, pos), highlight: false });
    chunks.push({ text: path.charAt(pos), highlight: true });
    cursor = pos + 1;
  }
  if (cursor < path.length) chunks.push({ text: path.slice(cursor), highlight: false });
  return (
    <>
      {chunks.map((chunk, i) =>
        chunk.highlight ? (
          <span key={i} className="quick-open__match">
            {chunk.text}
          </span>
        ) : (
          <span key={i}>{chunk.text}</span>
        ),
      )}
    </>
  );
}

export function QuickOpen({
  open,
  workspacePath,
  refreshToken,
  onPick,
  onClose,
}: Props) {
  const [rawQuery, setRawQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const [files, setFiles] = useState<WorkspaceEntry[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Reset transient state every time the modal is (re)opened.
  useEffect(() => {
    if (!open) return;
    setRawQuery("");
    setSelected(0);
    inputRef.current?.focus();
  }, [open]);

  // Load the file list lazily and re-load when the workspace tree refreshes.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setLoadError(null);
    api
      .listAllFiles(workspacePath)
      .then((entries) => {
        if (cancelled) return;
        setFiles(entries.filter((e) => e.kind === "file"));
      })
      .catch((err) => {
        if (cancelled) return;
        setLoadError(err instanceof Error ? err.message : String(err));
        setFiles([]);
      });
    return () => {
      cancelled = true;
    };
  }, [open, workspacePath, refreshToken]);

  const { query, line } = useMemo(() => splitQueryAndLine(rawQuery), [rawQuery]);

  const rows: Row[] = useMemo(() => {
    if (!files) return [];
    if (!query.trim()) {
      // Empty query: surface recents first, then the next files alphabetically
      // so the user always has *something* to pick.
      const recents = loadRecents(workspacePath);
      const byPath = new Map(files.map((f) => [f.relativePath, f]));
      const recentRows: Row[] = [];
      const seen = new Set<string>();
      for (const rel of recents) {
        const entry = byPath.get(rel);
        if (!entry) continue;
        recentRows.push({ entry, match: { score: 0, positions: [] } });
        seen.add(rel);
        if (recentRows.length >= MAX_RECENTS) break;
      }
      const filler: Row[] = files
        .filter((f) => !seen.has(f.relativePath))
        .slice(0, MAX_RESULTS - recentRows.length)
        .map((entry) => ({ entry, match: { score: 0, positions: [] } }));
      return [...recentRows, ...filler];
    }
    const ranked = fuzzyRank(query, files, (f) => f.relativePath);
    return ranked.slice(0, MAX_RESULTS).map(({ item, match }) => ({
      entry: item,
      match,
    }));
  }, [files, query, workspacePath]);

  // Clamp the selected index whenever the result set shrinks.
  useEffect(() => {
    setSelected((current) => {
      if (rows.length === 0) return 0;
      return Math.min(current, rows.length - 1);
    });
  }, [rows.length]);

  // Keep the highlighted row visible in the scroll container.
  useEffect(() => {
    if (!listRef.current) return;
    const el = listRef.current.querySelector<HTMLElement>(
      `[data-row-index="${selected}"]`,
    );
    el?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const commit = useCallback(
    (row: Row) => {
      pushRecent(workspacePath, row.entry.relativePath);
      onPick(row.entry, line);
      onClose();
    },
    [line, onClose, onPick, workspacePath],
  );

  const handleKey = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === "ArrowDown" || (event.ctrlKey && event.key === "j")) {
        event.preventDefault();
        setSelected((i) => (rows.length === 0 ? 0 : Math.min(i + 1, rows.length - 1)));
        return;
      }
      if (event.key === "ArrowUp" || (event.ctrlKey && event.key === "k")) {
        event.preventDefault();
        setSelected((i) => Math.max(i - 1, 0));
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        const row = rows[selected];
        if (row) commit(row);
        return;
      }
    },
    [commit, onClose, rows, selected],
  );

  if (!open) return null;

  const recentsActive = !query.trim();

  return (
    <div
      className="quick-open__backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Quick Open"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      onKeyDown={handleKey}
    >
      <div className="quick-open">
        <div className="quick-open__input-row">
          <Icon icon="solar:magnifer-linear" width={14} height={14} />
          <input
            ref={inputRef}
            className="quick-open__input"
            value={rawQuery}
            onChange={(event) => setRawQuery(event.target.value)}
            placeholder="Open file…  (suffix with :42 to jump to a line)"
            spellCheck={false}
            autoComplete="off"
          />
          {line && (
            <span className="quick-open__line-hint" title="Will jump to this line">
              :{line.number}
              {line.column ? `:${line.column}` : ""}
            </span>
          )}
        </div>

        <div className="quick-open__list" ref={listRef} role="listbox">
          {files === null && !loadError && (
            <div className="quick-open__muted">Loading files…</div>
          )}
          {loadError && (
            <div className="quick-open__error">
              <Icon icon="solar:danger-triangle-linear" width={13} height={13} />
              <span>{loadError}</span>
            </div>
          )}
          {files !== null && rows.length === 0 && !loadError && (
            <div className="quick-open__muted">
              No files match <code>{query}</code>.
            </div>
          )}
          {rows.map((row, index) => {
            const path = row.entry.relativePath;
            const tailStart = path.lastIndexOf("/") + 1;
            return (
              <button
                key={path}
                type="button"
                role="option"
                aria-selected={index === selected}
                data-row-index={index}
                data-active={index === selected ? "true" : "false"}
                className="quick-open__row"
                onMouseEnter={() => setSelected(index)}
                onClick={() => commit(row)}
              >
                <Icon
                  icon="solar:document-linear"
                  width={13}
                  height={13}
                  className="quick-open__row-icon"
                />
                <span className="quick-open__row-name">
                  <HighlightedPath
                    path={row.entry.name}
                    positions={row.match.positions
                      .filter((p) => p >= tailStart)
                      .map((p) => p - tailStart)}
                  />
                </span>
                <span className="quick-open__row-path">
                  <HighlightedPath path={path} positions={row.match.positions} />
                </span>
              </button>
            );
          })}
        </div>

        <div className="quick-open__footer">
          <span>
            {recentsActive ? "Recents" : `${rows.length} match${rows.length === 1 ? "" : "es"}`}
          </span>
          <span className="quick-open__hints">
            <kbd>↑</kbd>
            <kbd>↓</kbd> to navigate · <kbd>↵</kbd> to open · <kbd>esc</kbd> to close
          </span>
        </div>
      </div>
    </div>
  );
}
