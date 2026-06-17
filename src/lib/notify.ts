import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { getCurrentWindow, UserAttentionType } from "@tauri-apps/api/window";

/**
 * Thin wrappers around Tauri's notification + window-attention APIs.
 *
 * The whole module is best-effort: any error from the underlying plugin
 * is swallowed silently so a misconfigured environment (denied OS
 * permission, missing plugin in a stripped build…) never breaks the
 * surrounding feature flow. Callers should treat these as fire-and-forget.
 *
 * A process-wide toggle (`setNotificationsEnabled(false)`) lets the
 * desktop shell kill the whole subsystem from a single Settings hook
 * without each call site needing to check the user preference itself.
 */

let enabled = true;
let permissionPromise: Promise<boolean> | null = null;
let lastFiredAt = 0;
const DEDUPE_WINDOW_MS = 500;

/** Toggled by the desktop shell whenever ToolSettings.notificationsEnabled
 *  changes. Off ⇒ both `notify` and `flashTaskbar` become no-ops. */
export function setNotificationsEnabled(value: boolean): void {
  enabled = value;
}

async function ensurePermission(): Promise<boolean> {
  if (permissionPromise) return permissionPromise;
  permissionPromise = (async () => {
    try {
      const granted = await isPermissionGranted();
      if (granted) return true;
      const result = await requestPermission();
      return result === "granted";
    } catch {
      return false;
    }
  })();
  return permissionPromise;
}

/** Returns whether the Sinew window currently has the OS focus. We use
 *  it to skip notifications when the user is already looking at the app. */
export async function isWindowFocused(): Promise<boolean> {
  try {
    return await getCurrentWindow().isFocused();
  } catch {
    // If the API isn't available, behave as if focused — that's the
    // safer default (no spurious pings).
    return true;
  }
}

/** Flash the taskbar (Windows) / bounce the dock (macOS) / set the
 *  attention hint (Linux). Cheap and unobtrusive; safe to chain with
 *  `notify`. */
export function flashTaskbar(): void {
  if (!enabled) return;
  try {
    void getCurrentWindow()
      .requestUserAttention(UserAttentionType.Informational)
      .catch(() => undefined);
  } catch {
    // ignore — see module docstring
  }
}

/** Send a system notification. Skipped if disabled, if the window is
 *  focused, if the OS denied permission, or if another notification
 *  fired within `DEDUPE_WINDOW_MS` (so a burst of agent events from a
 *  single turn coalesces into a single ping). */
export async function notify(
  title: string,
  body: string,
  options?: { force?: boolean },
): Promise<void> {
  if (!enabled) return;
  const now = Date.now();
  if (!options?.force && now - lastFiredAt < DEDUPE_WINDOW_MS) return;
  if (!options?.force) {
    const focused = await isWindowFocused();
    if (focused) return;
  }
  const granted = await ensurePermission();
  if (!granted) return;
  try {
    sendNotification({ title, body });
    lastFiredAt = now;
  } catch {
    // ignore — see module docstring
  }
}

/** Convenience: ping the user with both the notification and the
 *  attention hint. The flash fires unconditionally (so the taskbar
 *  blinks even on systems where the OS notification is denied), the
 *  notification respects the focus / dedupe rules. */
export async function pingUserAttention(
  title: string,
  body: string,
): Promise<void> {
  if (!enabled) return;
  // We still skip the flash when the window is focused; flashing a
  // focused window does nothing visible on most OSes anyway, and
  // explicitly skipping keeps the behaviour predictable.
  const focused = await isWindowFocused();
  if (focused) return;
  flashTaskbar();
  await notify(title, body);
}
