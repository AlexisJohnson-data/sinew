/// Release notes shown in Welcome's "What's new" card, newest first. Add
/// an entry when a release ships something users will notice; an entry
/// newer than the running app stays hidden until that version is
/// installed, so notes can be written before the release.

export type ChangelogItem = {
  icon: string;
  title: string;
  body: string;
};

export type ChangelogEntry = {
  version: string;
  /// ISO date of the release.
  date: string;
  items: ChangelogItem[];
};

export const CHANGELOG: ChangelogEntry[] = [
  {
    version: "0.2.45",
    date: "2026-10-31",
    items: [
      {
        icon: "solar:shield-check-bold-duotone",
        title: "Claude through your own Claude Code",
        body: "Claude models now run through the official claude command, signed in with your own account. Sinew no longer handles Claude credentials, and you stay signed in.",
      },
      {
        icon: "solar:check-square-bold-duotone",
        title: "Same chat, Claude Code's tools",
        body: "Edits, commands and searches show as usual cards with diffs; approvals, questions and the browser go through Sinew.",
      },
    ],
  },
  {
    version: "0.2.44",
    date: "2026-10-29",
    items: [
      {
        icon: "solar:global-bold-duotone",
        title: "A much better browser agent",
        body: "The agent reads pages as an accessibility tree and acts on element references instead of guessing CSS selectors, clicks with the real mouse, and waits for the page to settle.",
      },
      {
        icon: "solar:bug-bold-duotone",
        title: "Debug your app in the browser",
        body: "Console errors with stack traces pointing at your files, failed network requests with their bodies, and Web Vitals (LCP, CLS, INP).",
      },
      {
        icon: "solar:shield-check-bold-duotone",
        title: "Safer browsing",
        body: "Running JavaScript in pages and uploading files are off by default (Settings → Tools → Browser).",
      },
    ],
  },
  {
    version: "0.2.43",
    date: "2026-10-29",
    items: [
      {
        icon: "solar:widget-bold-duotone",
        title: "Snaps like any Windows app",
        body: "Windows + arrow now docks Sinew to half the screen.",
      },
      {
        icon: "solar:settings-bold-duotone",
        title: "Settings inside the app",
        body: "Settings open as a window within Sinew instead of a separate one.",
      },
      {
        icon: "solar:speedometer-middle-bold-duotone",
        title: "OpenCode Go limits",
        body: "Rolling, weekly and monthly usage of your OpenCode Go plan, next to Claude and ChatGPT.",
      },
    ],
  },
  {
    version: "0.2.42",
    date: "2026-10-28",
    items: [
      {
        icon: "solar:wallet-money-bold-duotone",
        title: "Usage and plan limits",
        body: "Tokens and cost per conversation, live Claude and ChatGPT limits in a pill next to the composer, and Settings → Usage.",
      },
      {
        icon: "solar:branching-paths-up-bold-duotone",
        title: "Fork a conversation",
        body: "Hover one of your messages to continue from there in a new conversation; the original stays intact.",
      },
      {
        icon: "solar:notebook-bold-duotone",
        title: "Persistent memory",
        body: "The agent remembers your preferences and project decisions across conversations. Edit your rules for every project from this screen.",
      },
      {
        icon: "solar:shield-warning-bold-duotone",
        title: "Permissions",
        body: "Machine-wiping commands are blocked, and the agent can ask before risky ones.",
      },
      {
        icon: "solar:command-bold-duotone",
        title: "Slash commands and diagnostics",
        body: "Type / for commands, @ for MCP servers; language-server errors are checked after every edit.",
      },
    ],
  },
  {
    version: "0.2.41",
    date: "2026-10-27",
    items: [
      {
        icon: "solar:cpu-bolt-bold-duotone",
        title: "Your models, discovered",
        body: "Each connected account lists the models it can use, so new ones show up without an update.",
      },
    ],
  },
  {
    version: "0.2.39",
    date: "2026-09-22",
    items: [
      {
        icon: "solar:stars-bold-duotone",
        title: "Claude Opus 5.5, GPT-6 Sol and Luna",
        body: "The latest Claude and OpenAI models.",
      },
    ],
  },
  {
    version: "0.2.34",
    date: "2026-09-16",
    items: [
      {
        icon: "solar:server-bold-duotone",
        title: "Windows MCP servers in WSL",
        body: "Native Windows .exe MCP servers run as is in WSL workspaces.",
      },
    ],
  },
  {
    version: "0.2.31",
    date: "2026-09-12",
    items: [
      {
        icon: "solar:gallery-add-bold-duotone",
        title: "Paste images into files",
        body: "Ctrl+V an image in the editor saves it next to the file and inserts the link.",
      },
    ],
  },
];

/// `0.2.43` < `0.2.44`.
export function compareVersions(a: string, b: string): number {
  const pa = a.split(".").map((n) => Number.parseInt(n, 10) || 0);
  const pb = b.split(".").map((n) => Number.parseInt(n, 10) || 0);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const diff = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/// The entries up to the running version, newest first.
export function releasedEntries(appVersion: string | null): ChangelogEntry[] {
  if (!appVersion) return CHANGELOG;
  return CHANGELOG.filter((entry) => compareVersions(entry.version, appVersion) <= 0);
}

/// "Oct 28".
export function releaseDate(iso: string): string {
  const date = new Date(`${iso}T12:00:00`);
  return Number.isNaN(date.getTime())
    ? iso
    : date.toLocaleDateString([], { month: "short", day: "numeric" });
}
