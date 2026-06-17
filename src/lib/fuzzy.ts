/**
 * Tiny fuzzy matcher tuned for file paths (à la VS Code / Cursor "Go to file").
 *
 * Returns `null` when the haystack doesn't contain every character of the
 * query in order, otherwise a numeric score and the byte indices that were
 * matched in `haystack` (so the caller can render highlights).
 *
 * Scoring (higher is better):
 *   - Exact substring                                    : huge bonus + early exit.
 *   - Match right after a path separator (/  -  _  .) : segment-start bonus.
 *   - Consecutive matches                                : streak bonus.
 *   - Match in the file name (last segment) vs in the
 *     surrounding folder path                            : tail bonus.
 *   - Shorter haystack at equal coverage                 : light length penalty.
 *
 * Implementation is intentionally inline and dependency-free — Sinew aims at
 * parsimony, and a 1k-file workspace ranks in well under one frame on any
 * laptop with this. Callers should still cap the result list (~50 items) so
 * the React render stays cheap.
 */

export type FuzzyMatch = {
  /** Higher = better. Negative numbers are valid (rare, long paths). */
  score: number;
  /** Byte indices into `haystack` that were matched, in ascending order. */
  positions: number[];
};

const SEGMENT_BOUNDARIES = new Set(["/", "\\", "-", "_", ".", " "]);

/**
 * @param query     — what the user is typing. Spaces are ignored.
 * @param haystack  — the candidate (typically a relative file path).
 */
export function fuzzyMatch(query: string, haystack: string): FuzzyMatch | null {
  const q = query.trim();
  if (!q) return { score: 0, positions: [] };
  if (!haystack) return null;

  const qLower = q.replace(/\s+/g, "").toLowerCase();
  if (!qLower) return { score: 0, positions: [] };
  const hLower = haystack.toLowerCase();

  // Substring fast-path: huge bonus + tight positions.
  const sub = hLower.indexOf(qLower);
  if (sub !== -1) {
    const positions = Array.from({ length: qLower.length }, (_, i) => sub + i);
    const tailStart = haystack.lastIndexOf("/") + 1;
    const inTail = sub >= tailStart;
    const segmentStart =
      sub === 0 || SEGMENT_BOUNDARIES.has(haystack[sub - 1] ?? "");
    let score = 1_000 - haystack.length;
    if (inTail) score += 400;
    if (segmentStart) score += 200;
    return { score, positions };
  }

  // Greedy in-order scan.
  const positions: number[] = [];
  let score = 0;
  let qi = 0;
  let streak = 0;
  const tailStart = haystack.lastIndexOf("/") + 1;

  for (let hi = 0; hi < hLower.length && qi < qLower.length; hi += 1) {
    if (hLower[hi] !== qLower[qi]) {
      streak = 0;
      continue;
    }
    positions.push(hi);

    // Base hit.
    score += 16;
    // Match in the file name itself is much more relevant than in the path.
    if (hi >= tailStart) score += 8;
    // Match at the beginning of a path segment.
    const prev = hi === 0 ? "" : haystack[hi - 1];
    if (hi === 0 || SEGMENT_BOUNDARIES.has(prev ?? "")) score += 22;
    // Reward streaks of consecutive matches.
    streak += 1;
    if (streak > 1) score += streak * 6;

    qi += 1;
  }

  if (qi < qLower.length) return null;

  // Light penalty for very long haystacks so that the tighter match wins.
  score -= Math.min(haystack.length, 200) / 4;
  return { score, positions };
}

/**
 * Convenience: run `fuzzyMatch` over a list, filter out misses, and sort by
 * descending score. Stable on tie via the source order.
 */
export function fuzzyRank<T>(
  query: string,
  items: ReadonlyArray<T>,
  toHaystack: (item: T) => string,
): Array<{ item: T; match: FuzzyMatch }> {
  const out: Array<{ item: T; match: FuzzyMatch; index: number }> = [];
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    const match = fuzzyMatch(query, toHaystack(item));
    if (match) out.push({ item, match, index });
  }
  out.sort((a, b) => {
    if (a.match.score !== b.match.score) return b.match.score - a.match.score;
    return a.index - b.index;
  });
  return out.map(({ item, match }) => ({ item, match }));
}
