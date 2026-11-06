import { useEffect, useRef, useState } from "react";
import { Routes, Route, NavLink, Navigate, useLocation, useNavigate } from "react-router-dom";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { readPersisted, writePersisted } from "./lib/persist";
import { deepLinkTake } from "./lib/api";
import { startUpdateChecks, useUpdates } from "./lib/updates";
import { LayoutDashboard, Key, Settings as SettingsIcon, GitBranch, ScanSearch, Stethoscope, History as HistoryIcon, BookMarked, GitPullRequest, GitPullRequestArrow, SquareTerminal, Network, Trash2, CircleArrowUp } from "lucide-react";
import { GitHubIcon } from "./components/GitHubIcon";
import { Dashboard } from "./pages/Dashboard";
import { ProfileForm } from "./pages/ProfileForm";
import { SSHWizard } from "./pages/SSHWizard";
import { Settings } from "./pages/Settings";
import { GitHubAuth } from "./pages/GitHubAuth";
import { SparseClone } from "./pages/SparseClone";
import { Repositories } from "./pages/Repositories";
import { AutoAssign } from "./pages/AutoAssign";
import { Doctor } from "./pages/Doctor";
import { CommitAudit } from "./pages/CommitAudit";
import { Repo } from "./pages/Repo";
import { Ports } from "./pages/Ports";
import { Pulls } from "./pages/Pulls";
import { Uninstall } from "./pages/Uninstall";
import { ConsoleDrawer } from "./components/console/ConsoleDrawer";
import { toggleConsole, useConsole } from "./lib/console";

const REPO_TABS = ["changes", "history", "submodules", "github"];

/** The Repository page's tab the person was last on. */
function rememberedRepoTab(): string {
  const t = readPersisted<string>("repo.tab", "changes");
  return REPO_TABS.includes(t) ? t : "changes";
}

function App() {
  const location = useLocation();
  const navigate = useNavigate();
  // A gitswitch:// link (the VS Code extension's "Open GitSwitch") names a
  // page. Listened for once: `navigate` changes with every page, the listener
  // need not. The link that started the app came before anyone listened: it
  // is collected once the listener is in place.
  const navigateRef = useRef(navigate);
  useEffect(() => {
    navigateRef.current = navigate;
  }, [navigate]);
  useEffect(() => {
    let stop: (() => void) | undefined;
    let gone = false;
    listen<string>("navigate", (e) => navigateRef.current(e.payload))
      .then((f) => {
        if (gone) return f();
        stop = f;
        return deepLinkTake().then((page) => {
          if (!gone && typeof page === "string" && page.startsWith("/")) navigateRef.current(page);
        });
      })
      .catch(() => {});
    return () => {
      gone = true;
      stop?.();
    };
  }, []);
  // The tab on screen is remembered, so the sidebar's Repository and /repo
  // bring the person back to it after another page.
  const repoTabHere = /^\/repo\/(changes|history|submodules|github)\/?$/.exec(location.pathname)?.[1] ?? null;
  useEffect(() => {
    if (repoTabHere) writePersisted("repo.tab", repoTabHere);
  }, [repoTabHere]);
  const { open: consoleOpen, height: consoleHeight } = useConsole();

  // The version this copy was built as, and a newer release once one is out.
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then((v) => setVersion(typeof v === "string" ? v : null), () => {});
    return startUpdateChecks();
  }, []);
  const { update } = useUpdates();

  // Ctrl/Cmd + ` is the shortcut every other editor uses for its terminal.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "`") {
        e.preventDefault();
        toggleConsole();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Only the page scrolls, and the sidebar's list in a window too short for it —
  // never the window as a whole, which put a second scrollbar beside the
  // page's on Windows. The sidebar fits the default window (650 high).
  return (
    <div className="flex h-screen overflow-hidden bg-zinc-900 text-zinc-100">
      <nav className="w-56 shrink-0 border-r border-zinc-800 flex flex-col">
        <div className="p-4 border-b border-zinc-800">
          <h1 className="text-lg font-bold tracking-tight">
            <span className="text-emerald-400">Git</span>Switch
          </h1>
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto p-3 space-y-0.5 [scrollbar-width:thin]">
          <SidebarLink
            to="/"
            icon={<LayoutDashboard size={18} />}
            active={location.pathname === "/" || location.pathname.startsWith("/profile")}
          >
            Profiles
          </SidebarLink>
          <SidebarLink to="/ssh" icon={<Key size={18} />}>
            SSH Keys
          </SidebarLink>
          <SidebarLink to="/github" icon={<GitHubIcon size={18} />}>
            GitHub Auth
          </SidebarLink>
          <SidebarLink to="/repos" icon={<BookMarked size={18} />}>
            Repositories
          </SidebarLink>
          <SidebarLink to="/pulls" icon={<GitPullRequest size={18} />}>
            Pull requests
          </SidebarLink>
          <SidebarLink to="/sparse" icon={<GitBranch size={18} />}>
            Clone
          </SidebarLink>
          <SidebarLink
            to={`/repo/${repoTabHere ?? rememberedRepoTab()}`}
            icon={<GitPullRequestArrow size={18} />}
            active={/^\/repo(\/|$)/.test(location.pathname)}
          >
            Repository
          </SidebarLink>
          <SidebarLink to="/ports" icon={<Network size={18} />}>
            Ports
          </SidebarLink>
          <SidebarLink to="/auto-assign" icon={<ScanSearch size={18} />}>
            Auto Assign
          </SidebarLink>
          <SidebarLink to="/doctor" icon={<Stethoscope size={18} />}>
            Doctor
          </SidebarLink>
          <SidebarLink to="/commits" icon={<HistoryIcon size={18} />}>
            Commit Audit
          </SidebarLink>
          <SidebarLink to="/uninstall" icon={<Trash2 size={18} />}>
            Uninstall
          </SidebarLink>
          <SidebarLink to="/settings" icon={<SettingsIcon size={18} />}>
            Settings
          </SidebarLink>
          <button
            type="button"
            onClick={toggleConsole}
            title="A shell in the selected repository (Ctrl/Cmd + `)"
            className={`w-full flex items-center gap-3 px-3 py-1.5 rounded-lg text-sm transition-colors cursor-pointer ${
              consoleOpen
                ? "bg-emerald-500/10 text-emerald-400"
                : "text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
            }`}
          >
            <SquareTerminal size={18} />
            Terminal
          </button>
        </div>
        <div className="p-3 border-t border-zinc-800 space-y-2">
          {update && (
            <NavLink
              to="/settings"
              title="Install it from Settings"
              className="flex items-center gap-2 px-2 py-1.5 rounded-md text-xs font-medium bg-emerald-500/10 text-emerald-400 hover:bg-emerald-500/20 transition-colors"
            >
              <CircleArrowUp size={14} />
              Update available: v{update.version}
            </NavLink>
          )}
          {version && <p className="text-xs text-zinc-600">v{version}</p>}
        </div>
      </nav>

      <main
        className="flex-1 min-w-0 overflow-y-auto p-6"
        style={consoleOpen ? { paddingBottom: consoleHeight + 24 } : undefined}
      >
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/profile/new" element={<ProfileForm />} />
          <Route path="/profile/:id/edit" element={<ProfileForm />} />
          <Route path="/ssh" element={<SSHWizard />} />
          <Route path="/github" element={<GitHubAuth />} />
          <Route path="/repos" element={<Repositories />} />
          <Route path="/pulls" element={<Pulls />} />
          <Route path="/sparse" element={<SparseClone />} />
          <Route path="/repo/:tab" element={<Repo />} />
          <Route path="/repo" element={<Navigate to={`/repo/${rememberedRepoTab()}`} replace />} />
          <Route path="/changes" element={<Navigate to="/repo/changes" replace />} />
          <Route path="/auto-assign" element={<AutoAssign />} />
          <Route path="/ports" element={<Ports />} />
          <Route path="/doctor" element={<Doctor />} />
          <Route path="/history" element={<Navigate to="/repo/history" replace />} />
          <Route path="/commits" element={<CommitAudit />} />
          <Route path="/uninstall" element={<Uninstall />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </main>
      <ConsoleDrawer />
    </div>
  );
}

function SidebarLink({
  to,
  icon,
  children,
  active,
}: {
  to: string;
  icon: React.ReactNode;
  children: React.ReactNode;
  active?: boolean;
}) {
  return (
    <NavLink
      to={to}
      end
      className={({ isActive }) => {
        const highlighted = active !== undefined ? active : isActive;
        return `flex items-center gap-3 px-3 py-1.5 rounded-lg text-sm font-medium transition-colors ${
          highlighted
            ? "bg-zinc-800 text-emerald-400"
            : "text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/50"
        }`;
      }}
    >
      {icon}
      {children}
    </NavLink>
  );
}

export default App;
