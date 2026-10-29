import { useEffect, useState } from "react";
import { Icon } from "@iconify/react";
import { getVersion } from "@tauri-apps/api/app";
import { releaseDate, releasedEntries, type ChangelogEntry } from "../lib/changelog";

/// Welcome's "What's new": the running version's notes up front, earlier
/// releases one click away. Open every time Welcome shows; closing it
/// leaves a small link to bring it back.
export function WhatsNew() {
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [open, setOpen] = useState(true);
  const [showEarlier, setShowEarlier] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void getVersion()
      .catch(() => null)
      .then((version) => {
        if (!cancelled) setAppVersion(version);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const entries = releasedEntries(appVersion);
  const [latest, ...earlier] = entries;
  if (!latest) return null;

  const close = () => {
    setOpen(false);
    setShowEarlier(false);
  };

  if (!open) {
    return (
      <button type="button" className="welcome__whatsnew-reopen" onClick={() => setOpen(true)}>
        <Icon icon="solar:magic-stick-3-linear" width={13} height={13} />
        What's new in {latest.version}
      </button>
    );
  }

  return (
    <section className="welcome__whatsnew" aria-label="What's new">
      <div className="welcome__whatsnew-head">
        <span className="welcome__whatsnew-badge">
          <Icon icon="solar:magic-stick-3-bold" width={12} height={12} />
          New
        </span>
        <span className="welcome__whatsnew-title">What's new in {latest.version}</span>
        <span className="welcome__whatsnew-date">{releaseDate(latest.date)}</span>
        <button
          type="button"
          className="welcome__whatsnew-dismiss"
          onClick={close}
          title="Close"
          aria-label="Close what's new"
        >
          <Icon icon="solar:close-circle-linear" width={15} height={15} />
        </button>
      </div>
      <ul className="welcome__whatsnew-list">
        {latest.items.map((item) => (
          <li key={item.title} className="welcome__whatsnew-item">
            <span className="welcome__whatsnew-icon">
              <Icon icon={item.icon} width={18} height={18} />
            </span>
            <span className="welcome__whatsnew-body">
              <span className="welcome__whatsnew-item-title">{item.title}</span>
              <span className="welcome__whatsnew-item-sub">{item.body}</span>
            </span>
          </li>
        ))}
      </ul>
      {earlier.length > 0 && (
        <>
          <button
            type="button"
            className="welcome__whatsnew-earlier-toggle"
            onClick={() => setShowEarlier((value) => !value)}
            aria-expanded={showEarlier}
          >
            <Icon
              icon="solar:alt-arrow-down-linear"
              width={12}
              height={12}
              className="welcome__whatsnew-chevron"
              data-open={showEarlier ? "true" : "false"}
            />
            Earlier versions
          </button>
          {showEarlier && (
            <div className="welcome__whatsnew-earlier">
              {earlier.map((entry) => (
                <EarlierRelease key={entry.version} entry={entry} />
              ))}
            </div>
          )}
        </>
      )}
    </section>
  );
}

function EarlierRelease({ entry }: { entry: ChangelogEntry }) {
  return (
    <div className="welcome__whatsnew-release">
      <div className="welcome__whatsnew-release-head">
        <span className="welcome__whatsnew-release-version">{entry.version}</span>
        <span className="welcome__whatsnew-date">{releaseDate(entry.date)}</span>
      </div>
      <ul className="welcome__whatsnew-release-list">
        {entry.items.map((item) => (
          <li key={item.title}>
            <span className="welcome__whatsnew-item-title">{item.title}</span>
            <span className="welcome__whatsnew-item-sub"> — {item.body}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
