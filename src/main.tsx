import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { SettingsPane } from "./components/SettingsPane";
import { RemotePanel } from "./components/RemotePanel";
import "./styles.css";
import "./lib/customIcons";
import { api } from "./lib/ipc";

// Suppress the native WebKit context menu everywhere except inside text
// inputs (where the OS-level copy/paste menu is still useful). Components
// that want a context menu must intercept the event themselves and call
// `event.preventDefault()` *before* the listener below — they then render
// their own custom menu (this is how Monaco and our own ImageContextMenu
// behave). This mirrors what VSCode does: WebKit's menu is half-broken
// inside an embedded WKWebView (no download, no "open in new window", no
// share), so we hide it and serve our own actions.
window.addEventListener(
  "contextmenu",
  (event) => {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement | null;
    if (!target) {
      event.preventDefault();
      return;
    }
    if (target.closest("input, textarea, [contenteditable=\"true\"], [contenteditable=\"\"]")) {
      return;
    }
    event.preventDefault();
  },
  { capture: false },
);

// Route every left or middle click on an `<a href="http(s)://…">` anchor
// through Tauri's `open_external_url` command. Without this, plain anchors
// silently fail inside the WKWebView/wry shell:
//   • `target="_blank"` needs `webView(_:createWebViewWith:…)` to be wired
//     up on the native side, which wry does not do by default.
//   • a same-window navigation to https:// gets blocked by Tauri's default
//     navigation policy.
// So any component that just writes `<a href="https://…">` (Discord and
// GitHub buttons in Settings, markdown links, …) gets a working handler
// for free instead of having to remember to wire `api.openExternalUrl` by
// hand.
const openAnchorExternally = (event: MouseEvent) => {
  if (event.defaultPrevented) return;
  // `click` only fires for the primary (left) button; `auxclick` is used
  // for the middle button. We deliberately ignore right-click here — that
  // path goes through the contextmenu handler above.
  if (event.type === "auxclick" && event.button !== 1) return;
  const target = event.target;
  if (!(target instanceof Element)) return;
  const anchor = target.closest("a");
  if (!anchor) return;
  const href = anchor.getAttribute("href");
  if (!href) return;
  const trimmed = href.trim();
  if (!/^https?:\/\//i.test(trimmed)) return;
  event.preventDefault();
  void api
    .openExternalUrl(trimmed)
    .catch((err) => console.error("[external-link] failed to open", trimmed, err));
};
window.addEventListener("click", openAnchorExternally);
window.addEventListener("auxclick", openAnchorExternally);

// Secondary-window routing. The Tauri backend spawns dedicated windows
// for Settings / Remote with `?view=settings|remote` in the URL. We
// short-circuit React here so those windows skip the boot / updater
// flow and render only the requested pane — no editor, no chat,
// nothing else competing for the space.
function pickRoot(): React.ReactNode {
  const params = new URLSearchParams(window.location.search);
  const view = params.get("view");
  if (view === "settings") {
    return (
      <div className="app-secondary app-secondary--settings">
        <SettingsPane workspacePath={params.get("workspace") ?? ""} />
      </div>
    );
  }
  if (view === "remote") {
    return (
      <div className="app-secondary app-secondary--remote">
        <RemotePanel />
      </div>
    );
  }
  return <App />;
}

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{pickRoot()}</React.StrictMode>,
);
