import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, MouseEvent, WheelEvent } from "react";
import { Icon } from "@iconify/react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../lib/ipc";
import type { BrowserViewFrame, BrowserViewTab } from "../types";

/// Event the chat listens to for an element picked in the panel.
export const BROWSER_PICK_EVENT = "sinew:browser-pick";
const TABS_POLL_MS = 1500;
const MOVE_THROTTLE_MS = 40;
const HOVER_THROTTLE_MS = 120;

type Highlight = { x: number; y: number; width: number; height: number; label: string };

/// CDP modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8.
function modifiers(event: { altKey: boolean; ctrlKey: boolean; metaKey: boolean; shiftKey: boolean }) {
  return (
    (event.altKey ? 1 : 0) | (event.ctrlKey ? 2 : 0) | (event.metaKey ? 4 : 0) | (event.shiftKey ? 8 : 0)
  );
}

const MOUSE_BUTTONS = ["left", "middle", "right"] as const;

/// The workspace's browser, live: the page the agent is driving, streamed
/// from a windowless Chrome. The user can click, type, scroll, change the
/// address, and point at an element to send it to the chat.
export function BrowserPanel({ workspacePath, active }: { workspacePath: string; active: boolean }) {
  const [frame, setFrame] = useState<BrowserViewFrame | null>(null);
  const [tabs, setTabs] = useState<BrowserViewTab[]>([]);
  const [address, setAddress] = useState("");
  const [editingAddress, setEditingAddress] = useState(false);
  const [picking, setPicking] = useState(false);
  const [highlight, setHighlight] = useState<Highlight | null>(null);
  const [error, setError] = useState<string | null>(null);
  const imageRef = useRef<HTMLImageElement | null>(null);
  const lastMove = useRef(0);
  const lastHover = useRef(0);

  // Stream only while the panel is visible.
  useEffect(() => {
    if (!active) return undefined;
    let cancelled = false;
    setError(null);
    const unlisten = listen<BrowserViewFrame>("browser-view-frame", (event) => {
      if (!cancelled && event.payload.workspacePath === workspacePath) setFrame(event.payload);
    });
    const started = api.browserViewStart(workspacePath).catch((err) => {
      if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      return null;
    });
    return () => {
      cancelled = true;
      void unlisten.then((stop) => stop());
      void started.then((viewId) => {
        if (viewId !== null) void api.browserViewStop(workspacePath, viewId).catch(() => undefined);
      });
    };
  }, [active, workspacePath]);

  useEffect(() => {
    if (!active) return undefined;
    const refresh = () => {
      api
        .browserViewTabs(workspacePath)
        .then(setTabs)
        .catch(() => undefined);
    };
    refresh();
    const timer = window.setInterval(refresh, TABS_POLL_MS);
    return () => window.clearInterval(timer);
  }, [active, workspacePath]);

  const activeTab = tabs.find((tab) => tab.active);
  useEffect(() => {
    if (!editingAddress) setAddress(activeTab?.url ?? "");
  }, [activeTab?.url, editingAddress]);

  /// The pointer in page CSS pixels, or null outside the page image.
  const pagePoint = useCallback(
    (event: { clientX: number; clientY: number }) => {
      const image = imageRef.current;
      if (!image || !frame) return null;
      const box = image.getBoundingClientRect();
      const scale = Math.min(box.width / frame.width, box.height / frame.height);
      const left = box.left + (box.width - frame.width * scale) / 2;
      const top = box.top + (box.height - frame.height * scale) / 2;
      const x = (event.clientX - left) / scale;
      const y = (event.clientY - top) / scale;
      if (x < 0 || y < 0 || x > frame.width || y > frame.height) return null;
      return { x, y, scale, left: left - box.left, top: top - box.top };
    },
    [frame],
  );

  const sendInput = useCallback(
    (event: Record<string, unknown>) => {
      void api.browserViewInput(workspacePath, event).catch(() => undefined);
    },
    [workspacePath],
  );

  const onMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    event.currentTarget.focus();
    const point = pagePoint(event);
    if (!point) return;
    event.preventDefault();
    if (picking) return;
    sendInput({
      kind: "mouse",
      action: "mousePressed",
      x: point.x,
      y: point.y,
      button: MOUSE_BUTTONS[event.button] ?? "left",
      clickCount: event.detail || 1,
      modifiers: modifiers(event),
    });
  };

  const onMouseUp = (event: MouseEvent<HTMLDivElement>) => {
    const point = pagePoint(event);
    if (!point) return;
    if (picking) {
      setPicking(false);
      setHighlight(null);
      void api
        .browserViewPick(workspacePath, point.x, point.y, true)
        .then((picked) => {
          if (picked) window.dispatchEvent(new CustomEvent(BROWSER_PICK_EVENT, { detail: picked }));
        })
        .catch((err) => setError(err instanceof Error ? err.message : String(err)));
      return;
    }
    sendInput({
      kind: "mouse",
      action: "mouseReleased",
      x: point.x,
      y: point.y,
      button: MOUSE_BUTTONS[event.button] ?? "left",
      clickCount: event.detail || 1,
      modifiers: modifiers(event),
    });
  };

  const onMouseMove = (event: MouseEvent<HTMLDivElement>) => {
    const point = pagePoint(event);
    if (!point) return;
    const now = Date.now();
    if (picking) {
      if (now - lastHover.current < HOVER_THROTTLE_MS) return;
      lastHover.current = now;
      void api
        .browserViewPick(workspacePath, point.x, point.y, false)
        .then((picked) => {
          if (!picked) return setHighlight(null);
          const [x, y, width, height] = picked.element.rect;
          setHighlight({
            x: point.left + x * point.scale,
            y: point.top + y * point.scale,
            width: width * point.scale,
            height: height * point.scale,
            label: picked.element.label,
          });
        })
        .catch(() => undefined);
      return;
    }
    if (now - lastMove.current < MOVE_THROTTLE_MS) return;
    lastMove.current = now;
    sendInput({
      kind: "mouse",
      action: "mouseMoved",
      x: point.x,
      y: point.y,
      modifiers: modifiers(event),
    });
  };

  const onWheel = (event: WheelEvent<HTMLDivElement>) => {
    const point = pagePoint(event);
    if (!point || picking) return;
    sendInput({
      kind: "mouse",
      action: "mouseWheel",
      x: point.x,
      y: point.y,
      deltaX: event.deltaX,
      deltaY: event.deltaY,
      modifiers: modifiers(event),
    });
  };

  const onKey = (action: "keyDown" | "keyUp") => (event: KeyboardEvent<HTMLDivElement>) => {
    if (!frame) return;
    if (event.key === "Escape" && picking) {
      setPicking(false);
      setHighlight(null);
      return;
    }
    event.preventDefault();
    const printable = event.key.length === 1 && !event.ctrlKey && !event.metaKey;
    sendInput({
      kind: "key",
      action,
      key: event.key,
      code: event.code,
      keyCode: event.keyCode,
      text: action === "keyDown" ? (printable ? event.key : event.key === "Enter" ? "\r" : undefined) : undefined,
      modifiers: modifiers(event),
    });
  };

  const navigate = (target: string) => {
    if (!target.trim()) return;
    setEditingAddress(false);
    void api
      .browserViewNavigate(workspacePath, target)
      .then(() => setError(null))
      .catch((err) => setError(err instanceof Error ? err.message : String(err)));
  };

  return (
    <div className="browser-panel">
      <div className="browser-panel__bar">
        <button type="button" className="browser-panel__btn" title="Back" onClick={() => navigate("back")}>
          <Icon icon="solar:arrow-left-linear" width={14} height={14} />
        </button>
        <button type="button" className="browser-panel__btn" title="Forward" onClick={() => navigate("forward")}>
          <Icon icon="solar:arrow-right-linear" width={14} height={14} />
        </button>
        <button type="button" className="browser-panel__btn" title="Reload" onClick={() => navigate("reload")}>
          <Icon icon="solar:refresh-linear" width={14} height={14} />
        </button>
        <input
          className="browser-panel__address"
          value={address}
          placeholder="localhost:5173 or a URL"
          spellCheck={false}
          onFocus={() => setEditingAddress(true)}
          onBlur={() => setEditingAddress(false)}
          onChange={(event) => setAddress(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") navigate(address);
            if (event.key === "Escape") {
              setEditingAddress(false);
              event.currentTarget.blur();
            }
          }}
        />
        {tabs.length > 1 && (
          <select
            className="browser-panel__tabs"
            value={activeTab?.id ?? ""}
            onChange={(event) =>
              void api.browserViewSwitchTab(workspacePath, event.target.value).catch(() => undefined)
            }
            title="Tabs"
          >
            {tabs.map((tab) => (
              <option key={tab.id} value={tab.id}>
                {tab.title || tab.url}
              </option>
            ))}
          </select>
        )}
        <button
          type="button"
          className="browser-panel__btn"
          data-active={picking ? "true" : "false"}
          title="Point at an element to send it to the chat"
          onClick={() => {
            setPicking((value) => !value);
            setHighlight(null);
          }}
        >
          <Icon icon="solar:cursor-square-linear" width={14} height={14} />
          <span>Pick</span>
        </button>
      </div>
      {error && <div className="browser-panel__error">{error}</div>}
      <div
        className="browser-panel__view"
        data-picking={picking ? "true" : "false"}
        tabIndex={0}
        onMouseDown={onMouseDown}
        onMouseUp={onMouseUp}
        onMouseMove={onMouseMove}
        onMouseLeave={() => setHighlight(null)}
        onWheel={onWheel}
        onKeyDown={onKey("keyDown")}
        onKeyUp={onKey("keyUp")}
        onContextMenu={(event) => event.preventDefault()}
      >
        {frame ? (
          <img
            ref={imageRef}
            className="browser-panel__frame"
            src={`data:image/jpeg;base64,${frame.data}`}
            alt="Live view of the browser"
            draggable={false}
          />
        ) : (
          !error && <div className="browser-panel__empty">Starting the browser…</div>
        )}
        {highlight && (
          <div
            className="browser-panel__highlight"
            style={{
              left: highlight.x,
              top: highlight.y,
              width: highlight.width,
              height: highlight.height,
            }}
          >
            <span className="browser-panel__highlight-label">{highlight.label}</span>
          </div>
        )}
      </div>
      {picking && (
        <div className="browser-panel__hint">Click an element to send it to the chat · Esc to cancel</div>
      )}
    </div>
  );
}
