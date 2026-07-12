import type { RecentWorkspace } from "../types";

const RECENTS_KEY = "sinew.recentWorkspaces";
const LAST_KEY = "sinew.lastWorkspace";
const MAX_RECENTS = 12;

export function loadRecents(): RecentWorkspace[] {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as RecentWorkspace[];
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((r) => r && typeof r.path === "string")
      .sort(compareRecents);
  } catch {
    return [];
  }
}

// Pinned workspaces always come first (ordered by their most recent open),
// then everything else by recency. This way pinning a folder lifts it to
// the top and keeps it there even if you haven't touched it in a while.
function compareRecents(a: RecentWorkspace, b: RecentWorkspace): number {
  const ap = a.pinned ? 1 : 0;
  const bp = b.pinned ? 1 : 0;
  if (ap !== bp) return bp - ap;
  return b.lastOpenedMs - a.lastOpenedMs;
}

function persist(entries: RecentWorkspace[]): void {
  try {
    localStorage.setItem(RECENTS_KEY, JSON.stringify(entries));
  } catch {
    // ignore quota errors
  }
}

export function recordRecent(path: string, name: string): RecentWorkspace[] {
  const now = Date.now();
  const previous = loadRecents();
  const existingPinned = previous.find((r) => r.path === path)?.pinned ?? false;
  const filtered = previous.filter((r) => r.path !== path);
  // Pinned entries don't count against the recents cap — they're sticky on
  // purpose. The cap still applies to the unpinned tail so the list doesn't
  // grow forever.
  const pinned = filtered.filter((r) => r.pinned);
  const recent = filtered.filter((r) => !r.pinned);
  const next: RecentWorkspace[] = [
    { path, name, lastOpenedMs: now, pinned: existingPinned },
    ...pinned,
    ...recent,
  ].slice(0, MAX_RECENTS + pinned.length);
  next.sort(compareRecents);
  persist(next);
  try {
    localStorage.setItem(LAST_KEY, path);
  } catch {
    // ignore
  }
  return next;
}

export function removeRecent(path: string): RecentWorkspace[] {
  const next = loadRecents().filter((r) => r.path !== path);
  persist(next);
  return next;
}

export function toggleRecentPinned(path: string): RecentWorkspace[] {
  const next = loadRecents().map((r) =>
    r.path === path ? { ...r, pinned: !r.pinned } : r,
  );
  next.sort(compareRecents);
  persist(next);
  return next;
}

export function loadLastWorkspace(): string | null {
  try {
    return localStorage.getItem(LAST_KEY);
  } catch {
    return null;
  }
}

export function deriveName(path: string): string {
  const trimmed = path.replace(/\/$/, "");
  const idx = trimmed.lastIndexOf("/");
  return idx >= 0 ? trimmed.slice(idx + 1) || trimmed : trimmed;
}
