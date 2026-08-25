import { useEffect, useMemo, useRef, useState } from "react";
import { Icon } from "@iconify/react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { api } from "../lib/ipc";
import {
  availableModelsForProviders,
  labelForModelRef,
  modelIdFromRef,
  modelRefFromId,
  modelRefWithThinking,
  type ModelEntry,
  type ModelId,
} from "../lib/models";
import type { ModelRef, OpenRouterModel, ThinkingLevel } from "../types";

type Props = {
  open: boolean;
  /** Pre-fill the source field (used when the user clicks the sidebar
   *  icon on an already-open Windows workspace). When undefined, the
   *  user picks the source via the file picker. */
  initialSourcePath?: string;
  /** Where projects should land on the WSL side. Defaults to the
   *  current user's `~/projects` shown as a UNC path. */
  defaultTargetParent?: string;
  onCancel: () => void;
  /** Called after `prepare_migration_target` succeeded with the paths
   *  the parent needs to switch workspace and feed the agent prompt. */
  onConfirm: (result: {
    sourceWindows: string;
    targetWindows: string;
    prompt: string;
    /** Model the user explicitly picked for the migration agent. The
     *  parent forwards this so the new conversation runs on it. */
    model?: ModelRef;
    thinking?: ThinkingLevel;
  }) => void;
};

// Neutral fallback used only when the app can't resolve the real WSL home
// (WSL not installed, or resolution failed). The dialog resolves the actual
// per-user path at runtime via `default_wsl_projects_parent`, so no username
// is baked into the source.
const DEFAULT_WSL_PROJECTS_PARENT = "\\\\wsl$\\Ubuntu\\home";

function basename(path: string): string {
  const cleaned = path.replace(/[\\/]+$/, "");
  const idx = Math.max(cleaned.lastIndexOf("\\"), cleaned.lastIndexOf("/"));
  return idx >= 0 ? cleaned.slice(idx + 1) : cleaned;
}

function suggestTarget(sourcePath: string, parent: string): string {
  const name = basename(sourcePath);
  if (!name) return parent;
  const sep = parent.endsWith("\\") || parent.endsWith("/") ? "" : "\\";
  return `${parent}${sep}${name}`;
}

export function MigrationDialog({
  open,
  initialSourcePath,
  defaultTargetParent = DEFAULT_WSL_PROJECTS_PARENT,
  onCancel,
  onConfirm,
}: Props) {
  const [source, setSource] = useState(initialSourcePath ?? "");
  const [target, setTarget] = useState(() =>
    initialSourcePath
      ? suggestTarget(initialSourcePath, defaultTargetParent)
      : "",
  );
  const [busy, setBusy] = useState(false);
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [overwriteAck, setOverwriteAck] = useState(false);
  // Goal mode model used by the migration agent. We seed the dropdown
  // from the global default, then let the user pick anything else among
  // the providers they have configured. The chosen model travels with
  // the migration handoff so the agent runs on it from the first turn.
  const [defaultGoalModel, setDefaultGoalModel] = useState<ModelRef | null>(null);
  const [resolvedParent, setResolvedParent] = useState(defaultTargetParent);
  const [selectedModelId, setSelectedModelId] = useState<ModelId | "">("");
  const [configuredProviders, setConfiguredProviders] = useState<string[]>([]);
  const [openRouterModels, setOpenRouterModels] = useState<OpenRouterModel[]>([]);
  const [opencodeGoModels, setOpencodeGoModels] = useState<string[]>([]);
  const sourceRef = useRef<HTMLInputElement>(null);

  // Reset every time the dialog (re)opens.
  useEffect(() => {
    if (!open) return;
    setResolvedParent(defaultTargetParent);
    setSource(initialSourcePath ?? "");
    setTarget(
      initialSourcePath
        ? suggestTarget(initialSourcePath, defaultTargetParent)
        : "",
    );
    setBusy(false);
    setPicking(false);
    setError(null);
    setOverwriteAck(false);
    // Focus the source field if it's empty so the user can paste / pick.
    if (!initialSourcePath) sourceRef.current?.focus();
  }, [open, initialSourcePath, defaultTargetParent]);

  // Fetch the global default goal-mode model + the set of providers the
  // user has configured, so the picker only offers models that will
  // actually work. Failures stay silent — we just hide the picker.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void Promise.all([
      api.listDefaultModeModelSettings().catch(() => null),
      api.listConfiguredModelProviders().catch(() => [] as string[]),
      api.listOpenRouterModels().catch(() => [] as OpenRouterModel[]),
    ]).then(async ([modeSettings, providers, openRouter]) => {
      if (cancelled) return;
      const goal = modeSettings?.goal ?? modeSettings?.act ?? null;
      setDefaultGoalModel(goal);
      setConfiguredProviders(providers);
      setOpenRouterModels(openRouter);
      if (providers.includes("opencode-go")) {
        const goModels = await api.listOpencodeGoModels().catch(() => [] as string[]);
        if (!cancelled) setOpencodeGoModels(goModels);
      }
      setSelectedModelId(goal ? modelIdFromRef(goal) : "");
    });
    return () => {
      cancelled = true;
    };
  }, [open]);

  // Resolve the real WSL projects parent (per-user) when the dialog opens,
  // then re-suggest the target from it. Silent fallback to the neutral
  // default keeps the field usable even without WSL.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    void api
      .defaultWslProjectsParent()
      .then((parent) => {
        if (cancelled || !parent) return;
        setResolvedParent(parent);
        setTarget((current) => {
          const src = source || initialSourcePath || "";
          return src ? suggestTarget(src, parent) : current;
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [open, initialSourcePath]);

  const availableModels: ModelEntry[] = useMemo(
    () =>
      availableModelsForProviders(
        configuredProviders,
        openRouterModels,
        opencodeGoModels,
      ),
    [configuredProviders, openRouterModels, opencodeGoModels],
  );

  const selectedEntry = useMemo(
    () => availableModels.find((m) => m.value === selectedModelId) ?? null,
    [availableModels, selectedModelId],
  );

  const defaultGoalLabel = useMemo(
    () => labelForModelRef(defaultGoalModel),
    [defaultGoalModel],
  );

  // Auto-update the target when the user picks a different source, but
  // only if the user hasn't manually edited the target yet (compare it
  // against the suggestion for the previous source).
  const lastSuggestedTargetRef = useRef<string>("");
  useEffect(() => {
    if (!source) return;
    const suggested = suggestTarget(source, resolvedParent);
    if (target === "" || target === lastSuggestedTargetRef.current) {
      setTarget(suggested);
    }
    lastSuggestedTargetRef.current = suggested;
  }, [source, resolvedParent, target]);

  const pickSource = async () => {
    setPicking(true);
    setError(null);
    try {
      const selected = await openDialog({
        directory: true,
        multiple: false,
        title: "Pick the Windows folder to migrate",
      });
      if (typeof selected === "string") setSource(selected);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPicking(false);
    }
  };

  const isWslPath = useMemo(
    () => /^\\\\wsl[$\.][^\\]*\\/i.test(target),
    [target],
  );

  const submit = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await api.prepareMigrationTarget({
        sourcePath: source,
        targetPath: target,
      });
      if (result.targetExisted && result.targetWasNonEmpty && !overwriteAck) {
        // Force the user to acknowledge they're targeting a non-empty
        // folder before we actually start the agent (otherwise files
        // would mingle with the existing contents).
        setOverwriteAck(true);
        setError(
          "The target folder is not empty. Tick the box below to migrate into it anyway.",
        );
        setBusy(false);
        return;
      }
      // Build the ModelRef the parent will use to override the goal-mode
      // model on the freshly-created conversation. We only pass it when
      // the user picked something other than the current default.
      let model: ModelRef | undefined;
      let thinking: ThinkingLevel | undefined;
      if (selectedEntry) {
        const baseRef = modelRefFromId(selectedEntry.value);
        thinking = selectedEntry.defaultThinking;
        model = modelRefWithThinking(baseRef, thinking);
      }
      onConfirm({
        sourceWindows: result.sourceWindows,
        targetWindows: result.targetWindows,
        prompt: result.prompt,
        model,
        thinking,
      });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  if (!open) return null;

  return (
    <div
      className="migrate__backdrop"
      role="dialog"
      aria-modal="true"
      aria-label="Migrate a Windows project to WSL"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy) onCancel();
      }}
    >
      <div className="migrate">
        <header className="migrate__head">
          <div className="migrate__head-text">
            <h2>Migrate to WSL</h2>
            <p>
              Sinew will copy the project into WSL and run an agent that picks
              the safest strategy (git re-clone vs file copy), surfaces
              environment risks, and gives you a checklist.
            </p>
          </div>
          <button
            type="button"
            className="migrate__close"
            title="Cancel"
            onClick={onCancel}
            disabled={busy}
          >
            <Icon icon="solar:close-square-linear" width={14} height={14} />
          </button>
        </header>

        <div className="migrate__field">
          <label htmlFor="migrate-source">Source (Windows folder)</label>
          <div className="migrate__input-row">
            <input
              id="migrate-source"
              ref={sourceRef}
              className="migrate__input"
              value={source}
              onChange={(event) => setSource(event.target.value)}
              placeholder="C:\Users\you\Documents\my-project"
              spellCheck={false}
              autoComplete="off"
            />
            <button
              type="button"
              className="migrate__btn"
              onClick={() => void pickSource()}
              disabled={picking}
            >
              <Icon icon="solar:folder-open-linear" width={13} height={13} />
              <span>{picking ? "Picking…" : "Browse"}</span>
            </button>
          </div>
        </div>

        <div className="migrate__field">
          <label htmlFor="migrate-target">Target (WSL folder)</label>
          <input
            id="migrate-target"
            className="migrate__input"
            value={target}
            onChange={(event) => setTarget(event.target.value)}
            placeholder="\\wsl$\Ubuntu\home\you\projects\my-project"
            spellCheck={false}
            autoComplete="off"
          />
          {!isWslPath && target.length > 0 && (
            <p className="migrate__hint migrate__hint--warn">
              This doesn't look like a WSL path. Migration without a WSL
              target won't get you the benefits of Linux.
            </p>
          )}
        </div>

        <div className="migrate__field">
          <label htmlFor="migrate-model">
            <Icon
              icon="solar:cpu-bolt-linear"
              width={13}
              height={13}
              aria-hidden="true"
            />{" "}
            Migration agent model
          </label>
          {availableModels.length > 0 ? (
            <select
              id="migrate-model"
              className="migrate__input migrate__select"
              value={selectedModelId}
              onChange={(event) => setSelectedModelId(event.target.value)}
              disabled={busy}
            >
              {availableModels.map((entry) => (
                <option key={entry.value} value={entry.value}>
                  {entry.label}
                </option>
              ))}
            </select>
          ) : (
            <p className="migrate__hint migrate__hint--warn">
              No model providers configured. Open Settings and add an API
              key before starting a migration.
            </p>
          )}
          <p className="migrate__hint">
            Runs in Goal mode.
            {defaultGoalModel &&
            defaultGoalLabel &&
            selectedEntry?.value !== modelIdFromRef(defaultGoalModel)
              ? ` Workspace default: ${defaultGoalLabel}.`
              : ""}
          </p>
        </div>


        {overwriteAck && (
          <label className="migrate__overwrite">
            <input
              type="checkbox"
              onChange={(event) => {
                if (event.target.checked) setError(null);
                else setOverwriteAck(false);
              }}
            />
            <span>I understand the target folder already has files in it.</span>
          </label>
        )}

        {error && <div className="migrate__error">{error}</div>}

        <div className="migrate__actions">
          <button
            type="button"
            className="migrate__btn"
            onClick={onCancel}
            disabled={busy}
          >
            Cancel
          </button>
          <button
            type="button"
            className="migrate__btn migrate__btn--primary"
            onClick={() => void submit()}
            disabled={busy || !source || !target || availableModels.length === 0}
          >
            {busy ? "Preparing…" : "Start migration"}
          </button>
        </div>
      </div>
    </div>
  );
}
