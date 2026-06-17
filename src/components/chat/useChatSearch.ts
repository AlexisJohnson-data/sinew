import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type RefObject,
} from "react";
import type { ChatMessage } from "../../types";

/**
 * In-chat search powered by the CSS Custom Highlight API.
 *
 * The hook walks the visible chat container DOM, locates every occurrence
 * of `query` inside text nodes, and exposes Range objects so the consumer
 * can navigate them. Highlights are painted declaratively via
 * `CSS.highlights` — that means we never mutate the rendered DOM (no
 * `<mark>` injection) and the highlights survive React re-renders without
 * fighting reconciliation.
 *
 * The hook re-walks the container whenever:
 *   - the query changes (debounced lightly so streaming turns stay fluid)
 *   - the history array reference changes (new message / streaming tick)
 */

const ALL_HIGHLIGHT_NAME = "chat-search";
const CURRENT_HIGHLIGHT_NAME = "chat-search-current";

// Minimal typing for the CSS Custom Highlight API. TypeScript's stock lib
// only knows it on very recent versions, so we keep an inline shim to
// avoid bumping the whole project.
type HighlightLike = {
  add(range: Range): void;
  clear(): void;
};
type HighlightCtor = { new (...ranges: Range[]): HighlightLike };
type HighlightRegistry = {
  set(name: string, highlight: HighlightLike): void;
  delete(name: string): boolean;
};
type CssWithHighlights = typeof CSS & {
  highlights?: HighlightRegistry;
};

function getHighlightRegistry(): HighlightRegistry | null {
  if (typeof CSS === "undefined") return null;
  const registry = (CSS as CssWithHighlights).highlights;
  return registry ?? null;
}

function makeHighlight(ranges: Range[]): HighlightLike | null {
  const ctor = (window as unknown as { Highlight?: HighlightCtor }).Highlight;
  if (!ctor) return null;
  return new ctor(...ranges);
}

/**
 * Walk every text node inside `container` and collect the ranges that
 * match `query` (case-insensitive). Returns an empty array when the
 * query is empty.
 */
function collectMatches(container: HTMLElement, query: string): Range[] {
  const needle = query.toLowerCase();
  if (!needle) return [];

  const out: Range[] = [];
  const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT, {
    acceptNode: (node) => {
      if (!node.nodeValue || node.nodeValue.length === 0) {
        return NodeFilter.FILTER_REJECT;
      }
      // Skip hidden subtrees so we don't highlight content the user can't
      // see (collapsed tool result expanders, off-screen virtual rows…).
      const parent = node.parentElement;
      if (parent && (parent.offsetParent === null && parent.tagName !== "BODY")) {
        return NodeFilter.FILTER_REJECT;
      }
      return NodeFilter.FILTER_ACCEPT;
    },
  });

  let node = walker.nextNode();
  while (node) {
    const value = node.nodeValue ?? "";
    const lower = value.toLowerCase();
    let from = 0;
    while (from <= lower.length - needle.length) {
      const idx = lower.indexOf(needle, from);
      if (idx === -1) break;
      const range = document.createRange();
      range.setStart(node, idx);
      range.setEnd(node, idx + needle.length);
      out.push(range);
      from = idx + needle.length;
    }
    node = walker.nextNode();
  }
  return out;
}

export type ChatSearchHandle = {
  total: number;
  current: number;
  next: () => void;
  prev: () => void;
  reset: () => void;
  supported: boolean;
};

export function useChatSearch(
  containerRef: RefObject<HTMLElement>,
  query: string,
  history: ChatMessage[],
  isStreaming: boolean,
): ChatSearchHandle {
  const [matches, setMatches] = useState<Range[]>([]);
  const [current, setCurrent] = useState(0);
  const supported = useMemo(() => getHighlightRegistry() !== null, []);
  const rafRef = useRef<number | null>(null);
  const trimmed = query.trim();

  // Re-collect matches whenever the inputs change. We let React batch
  // history updates from the stream and ride a single rAF so high-rate
  // streaming doesn't thrash the TreeWalker on every token.
  useEffect(() => {
    const container = containerRef.current;
    if (!container || !trimmed) {
      setMatches([]);
      return;
    }
    if (rafRef.current !== null) {
      window.cancelAnimationFrame(rafRef.current);
    }
    rafRef.current = window.requestAnimationFrame(() => {
      rafRef.current = null;
      setMatches(collectMatches(container, trimmed));
    });
    return () => {
      if (rafRef.current !== null) {
        window.cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [containerRef, trimmed, history, isStreaming]);

  // Push the match list into the global highlight registry. Use a single
  // Highlight for every match (paints them all) and a second Highlight
  // for the currently focused one so it can be styled differently.
  useEffect(() => {
    const registry = getHighlightRegistry();
    if (!registry) return;
    if (matches.length === 0) {
      registry.delete(ALL_HIGHLIGHT_NAME);
      registry.delete(CURRENT_HIGHLIGHT_NAME);
      return;
    }
    const allHighlight = makeHighlight(matches);
    if (allHighlight) registry.set(ALL_HIGHLIGHT_NAME, allHighlight);
    return () => {
      registry.delete(ALL_HIGHLIGHT_NAME);
      registry.delete(CURRENT_HIGHLIGHT_NAME);
    };
  }, [matches]);

  // Clamp the current index when the result set shrinks under our feet.
  useEffect(() => {
    if (matches.length === 0) {
      setCurrent(0);
      return;
    }
    setCurrent((index) => Math.min(index, matches.length - 1));
  }, [matches.length]);

  // Highlight the focused match and scroll it into view.
  useEffect(() => {
    const registry = getHighlightRegistry();
    if (!registry) return;
    if (matches.length === 0) {
      registry.delete(CURRENT_HIGHLIGHT_NAME);
      return;
    }
    const focused = matches[current];
    if (!focused) return;
    const currentHighlight = makeHighlight([focused]);
    if (currentHighlight) registry.set(CURRENT_HIGHLIGHT_NAME, currentHighlight);

    // Scroll the focused match into view if possible. `scrollIntoView` on
    // a Range isn't standard, but its start container's parent works fine.
    const anchor = focused.startContainer.parentElement;
    if (anchor) {
      anchor.scrollIntoView({ block: "center", behavior: "smooth" });
    }
  }, [current, matches]);

  const next = useCallback(() => {
    setCurrent((index) =>
      matches.length === 0 ? 0 : (index + 1) % matches.length,
    );
  }, [matches.length]);

  const prev = useCallback(() => {
    setCurrent((index) =>
      matches.length === 0
        ? 0
        : (index - 1 + matches.length) % matches.length,
    );
  }, [matches.length]);

  const reset = useCallback(() => {
    setCurrent(0);
    setMatches([]);
    const registry = getHighlightRegistry();
    if (registry) {
      registry.delete(ALL_HIGHLIGHT_NAME);
      registry.delete(CURRENT_HIGHLIGHT_NAME);
    }
  }, []);

  return {
    total: matches.length,
    current: matches.length === 0 ? 0 : current + 1,
    next,
    prev,
    reset,
    supported,
  };
}
