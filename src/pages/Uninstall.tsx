import { useEffect, useMemo, useRef, useState } from "react";
import { Trash2, Search, Loader2 } from "lucide-react";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { RefreshButton } from "../components/RefreshButton";
import { Segmented, cleanError } from "../components/github/shared";
import { formatBytes } from "../components/changes/LfsFilesModal";
import { UninstallFlow } from "../components/apps/UninstallFlow";
import { UninstallDialog } from "../components/UninstallDialog";
import { usePersistedState } from "../lib/persist";
import * as api from "../lib/api";

type Sort = "name" | "size" | "date";

const SOURCE: Record<string, string> = {
  app: "App",
  "app-store": "App Store",
  homebrew: "Homebrew",
  msi: "Windows Installer",
  exe: "Program",
  store: "Store app",
  deb: "Package",
  snap: "Snap",
  flatpak: "Flatpak",
  appimage: "AppImage",
  package: "Package",
};

/**
 * The Uninstall tab: the apps on this computer. Uninstalling one goes step
 * by step (UninstallFlow), and afterwards only its leftovers are offered.
 */
export function Uninstall() {
  const [kind, setKind] = usePersistedState<"apps" | "packages">("uninstall.kind", "apps");
  const [list, setList] = useState<api.AppList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = usePersistedState<Sort>("uninstall.sort", "name");
  const [sizes, setSizes] = useState<Record<string, [number, boolean]>>({});
  const [picked, setPicked] = useState<api.InstalledApp | null>(null);
  const [selfOpen, setSelfOpen] = useState(false);
  const [resume, setResume] = useState<{ session: string; name: string } | null>(null);
  const [unfinished, setUnfinished] = useState<api.UninstallSessionSummary[]>([]);
  const ask = useRef(0);
  const shownKind = kind === "packages" && list?.platform !== "linux" ? "apps" : kind;

  async function load(k = shownKind) {
    const id = ++ask.current;
    setLoading(true);
    setError(null);
    try {
      const [l, s] = await Promise.all([api.appsList(k), api.appsSessions().catch(() => [])]);
      if (ask.current !== id) return;
      setList(l);
      setUnfinished(s);
      setSizes({});
    } catch (e) {
      if (ask.current === id) setError(cleanError(e));
    } finally {
      if (ask.current === id) setLoading(false);
    }
  }

  useEffect(() => {
    load(shownKind);
    return () => {
      ask.current++;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shownKind]);

  // Sizes the listing could not give (macOS bundles), two at a time.
  useEffect(() => {
    if (!list) return;
    const run = ask.current;
    const todo = (list.apps ?? []).filter((a) => a.size == null && !a.blocked).map((a) => a.id);
    let next = 0;
    let stopped = false;
    const worker = async () => {
      while (!stopped && next < todo.length) {
        const id = todo[next++];
        try {
          const s = await api.appsSize(id);
          if (stopped || ask.current !== run) return;
          setSizes((m) => ({ ...m, [id]: s }));
        } catch {
          // a size is a nicety
        }
      }
    };
    worker();
    worker();
    return () => {
      stopped = true;
    };
  }, [list]);

  const sizeOf = (a: api.InstalledApp) => a.size ?? sizes[a.id]?.[0] ?? null;

  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase();
    const apps = (list?.apps ?? []).filter((a) => !f || a.name.toLowerCase().includes(f) || (a.publisher ?? "").toLowerCase().includes(f));
    const by = [...apps];
    if (sort === "size") by.sort((a, b) => (sizeOf(b) ?? -1) - (sizeOf(a) ?? -1));
    else if (sort === "date") by.sort((a, b) => (b.date ?? 0) - (a.date ?? 0));
    else by.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
    return by;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [list, filter, sort, sizes]);

  function open(a: api.InstalledApp) {
    if (a.is_self) setSelfOpen(true);
    else setPicked(a);
  }

  return (
    <div className="space-y-5">
      <div className="flex items-start justify-between gap-4 flex-wrap">
        <div className="min-w-0">
          <h1 className="text-2xl font-bold text-zinc-100">Uninstall</h1>
          <p className="text-sm text-zinc-400 mt-1">
            The apps on this computer. Uninstall one, then move only what it left behind to the {list?.platform === "windows" ? "Recycle Bin" : "Trash"}.
          </p>
        </div>
        <RefreshButton onClick={() => load()} busy={loading} title="List the apps again" />
      </div>

      {unfinished.length > 0 && (
        <Card>
          <div className="space-y-2 text-xs" data-unfinished>
            <p className="text-zinc-300 font-medium">Not finished</p>
            {unfinished.map((u) => (
              <div key={u.session} className="flex items-center gap-2 flex-wrap">
                <span className="text-zinc-300">{u.name}</span>
                <span className="text-zinc-500">{u.state === "gone" ? "uninstalled — its leftovers were not looked at" : u.state === "uninstalling" ? "was being uninstalled" : "still installed"}</span>
                <Button variant="secondary" size="sm" onClick={() => setResume({ session: u.session, name: u.name })}>
                  Continue
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => api.appsForget(u.session).then(() => setUnfinished((x) => x.filter((y) => y.session !== u.session)), () => {})}
                >
                  Forget
                </Button>
              </div>
            ))}
          </div>
        </Card>
      )}

      <div className="flex items-center gap-3 flex-wrap">
        {list?.platform === "linux" && (
          <Segmented
            label="What to list"
            value={kind}
            onChange={setKind}
            options={[
              { value: "apps", label: "Apps" },
              { value: "packages", label: "All packages" },
            ]}
          />
        )}
        <div className="relative flex-1 min-w-[12rem] max-w-sm">
          <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-zinc-500" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={kind === "packages" ? "Find a package" : "Find an app"}
            aria-label="Find an app"
            className="w-full pl-8 pr-3 py-1.5 rounded-lg bg-zinc-800 border border-zinc-700 text-sm text-zinc-200 placeholder:text-zinc-500 focus:outline-none focus:border-emerald-500/50"
          />
        </div>
        <Segmented
          label="Sort"
          value={sort}
          onChange={setSort}
          options={[
            { value: "name", label: "Name" },
            { value: "size", label: "Size" },
            { value: "date", label: "Date" },
          ]}
        />
      </div>

      {kind === "packages" && list?.platform === "linux" && (
        <p className="text-xs text-amber-300/90">
          Every package, libraries included. Before anything is removed you see what else apt would remove with it; the system's own packages can't be removed here.
        </p>
      )}
      {error && <p className="text-sm text-red-400 whitespace-pre-wrap">{error}</p>}
      {(list?.notes ?? []).map((n) => (
        <p key={n} className="text-xs text-zinc-500">
          {n}
        </p>
      ))}

      <Card>
        {!list && loading ? (
          <p className="flex items-center gap-2 text-sm text-zinc-400">
            <Loader2 size={14} className="animate-spin" /> Listing the apps…
          </p>
        ) : shown.length === 0 ? (
          <p className="text-sm text-zinc-500">{filter ? "Nothing matches." : "No apps found."}</p>
        ) : (
          <ul className="divide-y divide-zinc-800" data-app-count={shown.length}>
            {shown.map((a) => {
              const size = sizeOf(a);
              return (
                <li key={a.id} className="py-2.5 flex items-center gap-3 min-w-0" data-app-row={a.name}>
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2 min-w-0 flex-wrap">
                      <span className="text-sm text-zinc-200 truncate">{a.name}</span>
                      {a.version && <span className="text-xs text-zinc-500 truncate max-w-[10rem]">{a.version}</span>}
                      <span className="text-[10px] px-1.5 rounded border border-zinc-700 text-zinc-400 shrink-0">{SOURCE[a.source] ?? a.source}</span>
                      {a.is_self && <span className="text-[10px] px-1.5 rounded border border-emerald-500/40 text-emerald-300 shrink-0">this app</span>}
                    </div>
                    <p className="text-xs text-zinc-500 truncate" title={a.location ?? ""}>
                      {[a.publisher, a.location].filter(Boolean).join(" · ")}
                    </p>
                    {a.notes.map((n) => (
                      <p key={n} className="text-[11px] text-zinc-500 break-words">
                        {n}
                      </p>
                    ))}
                    {a.blocked && <p className="text-[11px] text-amber-300/90" data-app-blocked>Not removable here: {a.blocked}</p>}
                  </div>
                  <span className="text-xs text-zinc-500 shrink-0 w-16 text-right">{size != null ? `${formatBytes(size)}${a.size == null && sizes[a.id] && !sizes[a.id][1] ? "+" : ""}` : ""}</span>
                  <Button variant="secondary" size="sm" onClick={() => open(a)} disabled={!!a.blocked} data-app-uninstall>
                    <Trash2 size={13} /> Uninstall…
                  </Button>
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <UninstallFlow
        app={picked}
        resume={resume}
        platform={list?.platform ?? "macos"}
        onClose={() => {
          setPicked(null);
          setResume(null);
          api.appsSessions().then(setUnfinished, () => {});
        }}
        onChanged={() => load()}
      />
      <UninstallDialog open={selfOpen} onClose={() => setSelfOpen(false)} />
    </div>
  );
}
