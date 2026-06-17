import {
  useCallback,
  useEffect,
  useRef,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { Icon } from "@iconify/react";

type Props = {
  query: string;
  total: number;
  current: number;
  onQueryChange: (next: string) => void;
  onNext: () => void;
  onPrev: () => void;
  onClose: () => void;
};

/**
 * In-pane search bar for the chat panel. Mirrors browsers' built-in
 * find UX (input + counter + nav arrows + close). Stays a pure render
 * component — all match maths live in `useChatSearch`.
 */
export function ChatSearch({
  query,
  total,
  current,
  onQueryChange,
  onNext,
  onPrev,
  onClose,
}: Props) {
  const inputRef = useRef<HTMLInputElement>(null);

  // Auto-focus the input on mount so the user can start typing right
  // away after pressing Ctrl+F.
  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const handleKey = useCallback(
    (event: ReactKeyboardEvent<HTMLInputElement>) => {
      if (event.key === "Escape") {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        if (event.shiftKey) onPrev();
        else onNext();
        return;
      }
    },
    [onClose, onNext, onPrev],
  );

  const empty = query.trim().length === 0;

  return (
    <div className="chat-search" role="search" aria-label="Search this conversation">
      <Icon icon="solar:magnifer-linear" width={13} height={13} />
      <input
        ref={inputRef}
        className="chat-search__input"
        type="search"
        value={query}
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={handleKey}
        placeholder="Search this conversation…"
        spellCheck={false}
        autoComplete="off"
      />
      <span
        className="chat-search__counter"
        data-empty={empty ? "true" : "false"}
        data-zero={!empty && total === 0 ? "true" : "false"}
      >
        {empty ? "" : `${current} / ${total}`}
      </span>
      <button
        type="button"
        className="chat-search__btn"
        title="Previous match (Shift+Enter)"
        onClick={onPrev}
        disabled={total === 0}
      >
        <Icon icon="solar:alt-arrow-up-linear" width={12} height={12} />
      </button>
      <button
        type="button"
        className="chat-search__btn"
        title="Next match (Enter)"
        onClick={onNext}
        disabled={total === 0}
      >
        <Icon icon="solar:alt-arrow-down-linear" width={12} height={12} />
      </button>
      <button
        type="button"
        className="chat-search__btn chat-search__btn--close"
        title="Close (Esc)"
        onClick={onClose}
      >
        <Icon icon="solar:close-square-linear" width={12} height={12} />
      </button>
    </div>
  );
}
