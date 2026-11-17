import { KeyboardEvent as ReactKeyboardEvent, ReactNode, useEffect, useRef, useState } from "react";
import { ExternalLink, Globe, Lock, MoreHorizontal, Trash2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { RemoteRepo } from "../../lib/api";
import { DeleteRepoModal } from "../github/DeleteRepoModal";
import { VisibilityModal } from "../github/VisibilityModal";
import { deleteBlock, repoUrl, settingsUrl, visibilityBlock, type Want } from "../github/repoAdmin";

type Props = {
  repo: RemoteRepo;
  /** The account that listed it ("gh:<login>" or "signin:<login>"): the one to act as. */
  account: string;
  /** That account's login, for the reasons a step can't be taken. */
  login: string;
  /** Its folder on this computer, when it is cloned: the delete dialog checks it for commits GitHub has and it lacks. */
  localPath: string | null;
};

type Item = { key: string; label: string; icon: ReactNode; why: string | null; danger?: boolean; run: () => void };

/**
 * A repository's other actions on the Repositories page: make it private or
 * public, open its settings on GitHub (its page, for one this account doesn't
 * admin), delete it. What the listing already knows decides what is offered
 * (no question to GitHub until one is picked): a step this account can't take
 * is greyed, with the reason.
 */
export function RepoActionsMenu({ repo, account, login, localPath }: Props) {
  const [open, setOpen] = useState(false);
  /** The dialog open, with the step picked as the menu offered it: a row corrected meanwhile doesn't turn it around. */
  const [dialog, setDialog] = useState<{ kind: "visibility"; want: Want } | { kind: "delete" } | null>(null);
  const box = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);

  const admin = repo.permission === "admin";
  // The listing has no "internal": an internal repository is listed as
  // private, and the dialog says what GitHub really has.
  const want: Want = repo.private ? "public" : "private";
  const visWhy = visibilityBlock({ admin, archived: repo.archived, fork: repo.fork }, login);
  const delWhy = deleteBlock(admin, login);
  const items: (Item | "separator")[] = [
    {
      key: "visibility",
      label: want === "public" ? "Make public…" : "Make private…",
      icon: want === "public" ? <Globe size={14} /> : <Lock size={14} />,
      why: visWhy,
      run: () => setDialog({ kind: "visibility", want }),
    },
    // GitHub shows a repository's settings to its admins only: the others get its page.
    admin
      ? {
          key: "settings",
          label: "Open settings on GitHub",
          icon: <ExternalLink size={14} />,
          why: null,
          run: () => void openUrl(settingsUrl(repo.owner, repo.name)).catch(() => undefined),
        }
      : {
          key: "open",
          label: "Open on GitHub",
          icon: <ExternalLink size={14} />,
          why: null,
          run: () => void openUrl(repoUrl(repo.owner, repo.name)).catch(() => undefined),
        },
    "separator",
    { key: "delete", label: "Delete on GitHub…", icon: <Trash2 size={14} />, why: delWhy, danger: true, run: () => setDialog({ kind: "delete" }) },
  ];

  // Another account listed the row: a dialog opened as the one before is closed.
  useEffect(() => {
    setDialog(null);
  }, [account]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        setOpen(false);
        trigger.current?.focus();
      }
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    // The first item that can be used gets the focus, so the arrows and Enter work at once.
    menu.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not([aria-disabled="true"])')?.focus();
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);

  /** Up and down move between the items, Home and End to the ends. */
  function onMenuKey(e: ReactKeyboardEvent<HTMLDivElement>) {
    const all = [...(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])];
    if (!all.length) return;
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    let next = -1;
    if (e.key === "ArrowDown") next = (at + 1) % all.length;
    else if (e.key === "ArrowUp") next = (at - 1 + all.length) % all.length;
    else if (e.key === "Home") next = 0;
    else if (e.key === "End") next = all.length - 1;
    else if (e.key === "Tab") setOpen(false);
    if (next >= 0) {
      e.preventDefault();
      all[next].focus();
    }
  }

  function pick(item: Item) {
    if (item.why) return;
    setOpen(false);
    item.run();
  }

  /**
   * A dialog closed: the focus goes back to the menu's button — or, when the
   * row went with a delete, to the next row's ⋯ button (else the one before),
   * so the keyboard carries on from where it was. The neighbours are found
   * now, while the row is still there.
   */
  function closeDialog() {
    const row = box.current?.closest("[data-repo-row]");
    const near = [row?.nextElementSibling, row?.previousElementSibling]
      .map((r) => r?.querySelector<HTMLButtonElement>('button[aria-label^="More actions for"]') ?? null)
      .find((b): b is HTMLButtonElement => !!b);
    setDialog(null);
    window.setTimeout(() => {
      const own = trigger.current;
      if (own?.isConnected) own.focus();
      else if (near?.isConnected) near.focus();
    }, 0);
  }

  return (
    <div ref={box} className="relative">
      <button
        ref={trigger}
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={`More actions for ${repo.owner}/${repo.name}`}
        title={admin ? "Make it private or public, open its settings on GitHub, or delete it" : "Open it on GitHub. Changing who can see it, or deleting it, needs an admin."}
        className="p-2 rounded-lg text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer"
      >
        <MoreHorizontal size={14} />
      </button>
      {open && (
        <div
          ref={menu}
          role="menu"
          aria-label={`More actions for ${repo.owner}/${repo.name}`}
          onKeyDown={onMenuKey}
          className="absolute right-0 top-full mt-1 z-30 w-72 max-w-[80vw] rounded-lg border border-zinc-700/70 bg-zinc-900 p-1 shadow-xl"
        >
          {items.map((item, i) =>
            item === "separator" ? (
              <div key={`sep-${i}`} role="separator" className="my-1 border-t border-zinc-800" />
            ) : (
              <button
                key={item.key}
                type="button"
                role="menuitem"
                data-repo-action={item.key}
                aria-disabled={item.why ? "true" : undefined}
                title={item.why ?? undefined}
                onClick={() => pick(item)}
                className={`w-full text-left px-3 py-2 rounded-md flex items-start gap-2.5 focus:outline-none focus:bg-zinc-800 ${
                  item.why ? "cursor-not-allowed" : "cursor-pointer hover:bg-zinc-800"
                }`}
              >
                <span className={`mt-0.5 shrink-0 ${item.why ? "text-zinc-600" : item.danger ? "text-red-400" : "text-zinc-400"}`}>{item.icon}</span>
                <span className="min-w-0">
                  <span className={`block text-xs font-medium ${item.why ? "text-zinc-500" : item.danger ? "text-red-400" : "text-zinc-200"}`}>{item.label}</span>
                  {item.why && <span className="block text-[11px] text-zinc-500 leading-snug mt-0.5">{item.why}</span>}
                </span>
              </button>
            )
          )}
        </div>
      )}
      {/* Mounted only while open: a page of rows keeps no idle dialogs. */}
      {dialog?.kind === "visibility" && <VisibilityModal open account={account} owner={repo.owner} name={repo.name} want={dialog.want} onClose={closeDialog} />}
      {dialog?.kind === "delete" && <DeleteRepoModal open account={account} owner={repo.owner} name={repo.name} repoPath={localPath} onClose={closeDialog} />}
    </div>
  );
}
