import { useEffect, useRef, useState } from "react";
import { CheckCircle2, Copy, ExternalLink, Loader2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { copyText } from "../../lib/clipboard";
import { Button } from "../Button";

/**
 * A code GitHub gave, to enter on github.com: shown large, with Copy and
 * Open GitHub, and a line saying GitSwitch is waiting. The same look as the
 * GitHub sign-in's code on the GitHub Auth page.
 */
export function DeviceCodePanel({
  code,
  url,
  login,
  waiting = "Waiting for you to approve it on GitHub…",
}: {
  code: string;
  url: string;
  /** The account to be signed in as on github.com, when it matters. */
  login?: string;
  waiting?: string;
}) {
  const [copied, setCopied] = useState<boolean | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);

  async function copy() {
    const done = await copyText(code);
    setCopied(done);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setCopied(null), 2000);
  }

  return (
    <div className="px-4 py-5 rounded-lg bg-zinc-950/60 border border-zinc-700 text-center space-y-3" data-device-code={code}>
      <p className="text-xs text-zinc-500 uppercase tracking-wider">Your code</p>
      <div className="inline-flex items-center gap-3 bg-zinc-950 border border-zinc-700 rounded-xl px-5 py-3 max-w-full">
        <span className="text-2xl font-mono font-bold tracking-[0.3em] text-emerald-400 break-all" aria-label={`The code is ${code}`}>
          {code}
        </span>
        <button
          type="button"
          onClick={() => void copy()}
          aria-label="Copy the code"
          title="Copy the code"
          className="p-2 rounded-lg hover:bg-zinc-700 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer"
        >
          {copied ? <CheckCircle2 size={18} className="text-emerald-400" /> : <Copy size={18} />}
        </button>
      </div>
      <p className="text-xs h-4" role="status">
        {copied === true && <span className="text-emerald-300/90">Copied.</span>}
        {copied === false && <span className="text-amber-300/90">Couldn't copy it: select the code and copy it by hand.</span>}
      </p>
      {/* The next step is GitHub's: its button has the focus when the code appears. */}
      <div className="flex justify-center">
        <Button
          type="button"
          autoFocus
          onClick={() => {
            openUrl(url).catch(() => window.open(url, "_blank"));
          }}
        >
          <ExternalLink size={16} />
          Open GitHub
        </Button>
      </div>
      <p className="text-xs text-zinc-500 break-words">
        Enter the code at <span className="font-mono text-zinc-300">{url}</span>
        {login ? (
          <>
            , signed in to GitHub as <span className="text-zinc-300">@{login}</span>.
          </>
        ) : (
          "."
        )}
      </p>
      <p className="flex items-center justify-center gap-2 text-sm text-zinc-400">
        <Loader2 size={16} className="animate-spin" />
        {waiting}
      </p>
    </div>
  );
}
