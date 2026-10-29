import { useEffect } from "react";
import { Icon } from "@iconify/react";
import { SettingsPane } from "./SettingsPane";

type Props = {
  /// Workspace the tool and sub-agent settings apply to; empty on Welcome.
  workspacePath: string;
  /// Section shown first ("providers", "usage"…).
  section?: string;
  onClose: () => void;
};

/// Settings as a window inside the app window rather than a separate OS
/// window: it dims the app, closes on Escape, the close button or a click
/// outside, and keeps the app's size and position.
export function SettingsDialog({ workspacePath, section, onClose }: Props) {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !event.defaultPrevented) onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose]);

  return (
    <div
      className="settings-dialog__backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="settings-dialog" role="dialog" aria-modal="true" aria-label="Settings">
        <header className="settings-dialog__head">
          <span className="settings-dialog__title">Settings</span>
          <button
            type="button"
            className="settings-dialog__close"
            title="Close (Esc)"
            aria-label="Close settings"
            onClick={onClose}
          >
            <Icon icon="solar:close-circle-linear" width={16} height={16} />
          </button>
        </header>
        <div className="settings-dialog__body">
          <SettingsPane workspacePath={workspacePath} initialSection={section} />
        </div>
      </div>
    </div>
  );
}
