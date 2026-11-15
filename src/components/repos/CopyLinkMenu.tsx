import { useEffect, useRef, useState } from "react";
import { Copy } from "lucide-react";
import { copyText } from "../../lib/clipboard";
import { useToast } from "../Toast";

export type CloneLink = { key: "https" | "ssh" | "cli"; label: string; text: string };

/**
 * The three ways GitHub offers a repository's address on its own Code button:
 * HTTPS, SSH and the GitHub CLI command. `sshUrl` is the SSH address the list
 * already clones with (git@github.com:…, or an organization's org-123@… for
 * SSH certificates); HTTPS and gh use the host it names.
 */
export function cloneLinks(fullName: string, sshUrl: string): CloneLink[] {
  const host = sshUrl.match(/@([^:/]+):/)?.[1] ?? "github.com";
  return [
    { key: "https", label: "HTTPS", text: `https://${host}/${fullName}.git` },
    { key: "ssh", label: "SSH", text: sshUrl },
    { key: "cli", label: "GitHub CLI", text: host === "github.com" ? `gh repo clone ${fullName}` : `gh repo clone ${host}/${fullName}` },
  ];
}

type Props = { fullName: string; sshUrl: string };

/** Copy a repository's address: a small menu of HTTPS, SSH and GitHub CLI. */
export function CopyLinkMenu({ fullName, sshUrl }: Props) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const toast = useToast();
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    const esc = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", away);
    document.addEventListener("keydown", esc);
    return () => {
      document.removeEventListener("mousedown", away);
      document.removeEventListener("keydown", esc);
    };
  }, [open]);
  const links = cloneLinks(fullName, sshUrl);

  async function copy(l: CloneLink) {
    setOpen(false);
    if (await copyText(l.text)) toast.success(l.key === "cli" ? "GitHub CLI command copied" : `${l.label} link copied`);
    else toast.error(`Couldn't copy it. The ${l.label} address is ${l.text}`);
  }

  return (
    <div ref={box} className="relative">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="Copy link"
        title="Copy the repository's address: HTTPS, SSH or GitHub CLI"
        className="p-2 rounded-lg text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800 transition-colors cursor-pointer"
      >
        <Copy size={14} />
      </button>
      {open && (
        <div
          role="menu"
          aria-label="Copy link"
          className="absolute right-0 top-full mt-1 z-30 w-80 max-w-[80vw] rounded-lg border border-zinc-700/70 bg-zinc-900 p-1 shadow-xl"
        >
          {links.map((l) => (
            <button
              key={l.key}
              type="button"
              role="menuitem"
              onClick={() => void copy(l)}
              className="w-full text-left px-3 py-2 rounded-md hover:bg-zinc-800 cursor-pointer"
            >
              <span className="block text-xs font-medium text-zinc-200">{l.label}</span>
              <span className="block font-mono text-[11px] text-zinc-400 truncate" title={l.text}>
                {l.text}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
