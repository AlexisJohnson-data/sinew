/**
 * Hand off a migration prompt between the Welcome screen and the
 * Workspace it triggers. The Welcome's `MigrationDialog` resolves the
 * paths and prompt via `prepare_migration_target`, then asks App to
 * open the target as a workspace. The freshly-mounted Workspace pulls
 * the prompt out of here, creates a fresh conversation, and pre-fills
 * the chat composer in Goal mode so the user just has to press Send.
 *
 * Pure module singleton — no React state, no localStorage. The handoff
 * lives in memory only and gets consumed exactly once.
 */

import type { ModelRef, ThinkingLevel } from "../types";

type Pending = {
  prompt: string;
  /** Friendly description used to remind the user what's being migrated. */
  sourceWindows: string;
  /** Goal-mode model the user picked in the MigrationDialog. The
   *  receiving ChatPane applies it before the user submits, so the
   *  migration runs with the chosen LLM regardless of the workspace's
   *  previous default. */
  model?: ModelRef;
  thinking?: ThinkingLevel;
};

let pending: Pending | null = null;

export function setPendingMigration(value: Pending): void {
  pending = value;
}

export function consumePendingMigration(): Pending | null {
  const value = pending;
  pending = null;
  return value;
}

/** Window event dispatched by Workspace once it's ready to receive the
 *  prompt. ChatPane listens for it and pre-fills its composer + mode. */
export const MIGRATION_PREFILL_EVENT = "sinew:migration-prefill";

export type MigrationPrefillDetail = {
  text: string;
  /** Optional override for the goal-mode model — when present the
   *  composer applies it to the new conversation before the user sends. */
  model?: ModelRef;
  thinking?: ThinkingLevel;
};
