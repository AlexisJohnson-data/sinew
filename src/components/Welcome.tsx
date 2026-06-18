import { useEffect, useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Icon } from "@iconify/react";
import { loadRecents } from "../lib/recents";
import { api } from "../lib/ipc";
import { labelForModelRef } from "../lib/models";
import { setPendingMigration } from "../lib/pendingMigration";
import type {
  ActiveTurnSummary,
  ActiveTurnsChangedPayload,
  ModeModelSettings,
  RecentWorkspace,
} from "../types";
import { MigrationDialog } from "./MigrationDialog";
import { SinewMark } from "./SinewMark";
import { WindowControls, isWindowsPlatform } from "./WindowControls";

type Props = {
  onPick: (path: string) => void;
  error: string | null;
  deriveName: (path: string) => string;
};

const MAX_VISIBLE_RECENTS = 5;
const IS_WINDOWS = isWindowsPlatform();

/// Collapse a list of `ActiveTurnSummary` items down to the set of workspace
/// paths that currently have an in-flight agent turn. The backend reports
/// these globally (across every Sinew window), so a recent workspace can be
/// "live" even when it's owned by a sibling window.
function activeWorkspaceSet(turns: ActiveTurnSummary[]): Set<string> {
  return new Set(turns.map((turn) => turn.workspaceId));
}

export function Welcome({ onPick, error, deriveName }: Props) {
  const [recents, setRecents] = useState<RecentWorkspace[]>([]);
  const [picking, setPicking] = useState(false);
  const [activeWorkspaces, setActiveWorkspaces] = useState<Set<string>>(
    () => new Set(),
  );
  const [migrateOpen, setMigrateOpen] = useState(false);
  // Default mode/model assignment surfaced as a footer hint — so the user
  // (especially on first launch) knows which LLM is going to handle the
  // turn before they pick a workspace or trigger a migration.
  const [modeModels, setModeModels] = useState<ModeModelSettings | null>(null);

  useEffect(() => {
    setRecents(loadRecents());
  }, []);

  useEffect(() => {
    let cancelled = false;
    void api
      .listDefaultModeModelSettings()
      .then((settings) => {
        if (!cancelled) setModeModels(settings);
      })
      .catch(() => {
        if (!cancelled) setModeModels(null);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const modelLabels = useMemo(() => {
    if (!modeModels) return null;
    const act = labelForModelRef(modeModels.act);
    const plan = labelForModelRef(modeModels.plan);
    const goal = labelForModelRef(modeModels.goal);
    const unique = Array.from(new Set([act, plan, goal].filter(Boolean))) as string[];
    return { act, goal, unique };
  }, [modeModels]);

  // Surface running agent turns on the recents list. We seed from
  // `list_active_turns` (so the loader is correct the moment Welcome paints)
  // then keep in sync with the `active-turns-changed` event the backend
  // fans out whenever a turn starts or finishes anywhere in the app.
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | null = null;

    void api
      .listActiveTurns()
      .then((turns) => {
        if (!cancelled) setActiveWorkspaces(activeWorkspaceSet(turns));
      })
      .catch(() => {
        // Non-fatal: leave the set empty so the regular folder icon shows.
      });

    (async () => {
      const u = await listen<ActiveTurnsChangedPayload>(
        "active-turns-changed",
        (event) => {
          setActiveWorkspaces(activeWorkspaceSet(event.payload.activeTurns));
        },
      );
      if (cancelled) {
        u();
      } else {
        unlisten = u;
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const pickFolder = async () => {
    if (picking) return;
    setPicking(true);
    try {
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === "string") {
        onPick(selected);
      }
    } catch {
      // user cancelled or platform error
    } finally {
      setPicking(false);
    }
  };

  // Open the Settings window from Welcome (no workspace yet). The pane
  // handles `workspacePath: undefined` gracefully — providers, MCP and
  // skills don't need a workspace; tool/sub-agent panes show a hint.
  const openSettingsWindow = () => {
    void api
      .openSecondaryWindow({ view: "settings" })
      .catch((err) =>
        console.error("[settings-window] failed to open", err),
      );
  };

  return (
    <div className="welcome">
      {IS_WINDOWS && (
        /* Drag region + custom window controls for the frameless Windows
           shell. The wrapper itself is the drag handle (it owns
           `data-tauri-drag-region`); buttons inside opt out via
           `data-tauri-drag-region="false"` set by <WindowControls />. */
        <div
          className="welcome__titlebar"
          data-tauri-drag-region
        >
          <WindowControls />
        </div>
      )}
      {/* Settings entry point. Pinned top-left so it's nowhere near the
         window close button on the right. Visible on every platform. */}
      <button
        type="button"
        className="welcome__settings-btn welcome__settings-btn--standalone"
        onClick={openSettingsWindow}
        title="Settings"
        aria-label="Settings"
        data-tauri-drag-region="false"
      >
        <Icon icon="solar:settings-linear" width={15} height={15} />
      </button>
      <main className="welcome__stage">
        <header className="welcome__head">
          <span className="welcome__mark-dot" aria-hidden="true">
            <span className="welcome__mark-inner">
              <SinewMark size={22} className="welcome__mark-glyph" />
            </span>
          </span>
          <h1 className="welcome__title">
            Sinew<span className="welcome__title-dot">.</span>
          </h1>
          <p className="welcome__tag">Your personal Agentic IDE</p>
        </header>

        <button
          className="welcome__cta"
          onClick={pickFolder}
          disabled={picking}
        >
          <span className="welcome__cta-icon">
            <Icon icon="solar:folder-with-files-bold-duotone" width={22} height={22} />
          </span>
          <span className="welcome__cta-body">
            <span className="welcome__cta-title">Open a folder</span>
            <span className="welcome__cta-sub">
              {picking ? "Opening…" : "Choose any directory to start a session"}
            </span>
          </span>
          <span className="welcome__cta-chev">
            <Icon icon="solar:alt-arrow-right-linear" width={16} height={16} />
          </span>
        </button>

        {IS_WINDOWS && (
          <button
            className="welcome__cta welcome__cta--secondary"
            onClick={() => setMigrateOpen(true)}
          >
            <span className="welcome__cta-icon">
              <Icon
                icon="solar:transfer-horizontal-linear"
                width={20}
                height={20}
              />
            </span>
            <span className="welcome__cta-body">
              <span className="welcome__cta-title">
                Migrate a Windows project to WSL
              </span>
              <span className="welcome__cta-sub">
                Copy a folder over and let an agent handle the cleanup
              </span>
            </span>
            <span className="welcome__cta-chev">
              <Icon icon="solar:alt-arrow-right-linear" width={16} height={16} />
            </span>
          </button>
        )}

        {error && (
          <div className="welcome__error">{error}</div>
        )}

        {modelLabels && modelLabels.unique.length > 0 && (
          <div
            className="welcome__model"
            role="note"
            title="Switch models from the chat header once a workspace is open."
          >
            <Icon
              icon="solar:cpu-bolt-linear"
              width={13}
              height={13}
              aria-hidden="true"
            />
            <span>
              {modelLabels.unique.length === 1 ? (
                <>Default model: <strong>{modelLabels.unique[0]}</strong></>
              ) : (
                <>
                  Models — Act <strong>{modelLabels.act}</strong> · Goal{" "}
                  <strong>{modelLabels.goal}</strong>
                </>
              )}
            </span>
          </div>
        )}

        <MigrationDialog
          open={migrateOpen}
          onCancel={() => setMigrateOpen(false)}
          onConfirm={({
            targetWindows,
            sourceWindows,
            prompt,
            model,
            thinking,
          }) => {
            setPendingMigration({
              prompt,
              sourceWindows,
              model,
              thinking,
            });
            setMigrateOpen(false);
            onPick(targetWindows);
          }}
        />

        {recents.length > 0 ? (
          <section className="welcome__section">
            <h2 className="welcome__section-heading">Recent</h2>
            <div className="welcome__recents">
              {recents.slice(0, MAX_VISIBLE_RECENTS).map((recent) => {
                const isActive = activeWorkspaces.has(recent.path);
                return (
                  <button
                    key={recent.path}
                    className="welcome__recent"
                    data-active={isActive ? "true" : "false"}
                    onClick={() => onPick(recent.path)}
                  >
                    <span className="welcome__recent-icon">
                      {isActive ? (
                        <span
                          className="welcome__recent-spinner"
                          role="status"
                          aria-label="Agent running"
                        />
                      ) : (
                        <Icon
                          icon="solar:folder-bold-duotone"
                          width={18}
                          height={18}
                        />
                      )}
                    </span>
                    <span className="welcome__recent-body">
                      <span className="welcome__recent-name">
                        {recent.name || deriveName(recent.path)}
                      </span>
                      <span className="welcome__recent-path">{recent.path}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </section>
        ) : (
          <div className="welcome__empty">
            No recent workspaces yet. Pick a folder to get started.
          </div>
        )}
      </main>
    </div>
  );
}
