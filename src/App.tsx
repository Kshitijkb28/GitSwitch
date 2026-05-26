import { Routes, Route, NavLink } from "react-router-dom";
import { LayoutDashboard, Key, Settings as SettingsIcon } from "lucide-react";
import { GitHubIcon } from "./components/GitHubIcon";
import { Dashboard } from "./pages/Dashboard";
import { ProfileForm } from "./pages/ProfileForm";
import { SSHWizard } from "./pages/SSHWizard";
import { Settings } from "./pages/Settings";
import { GitHubAuth } from "./pages/GitHubAuth";

function App() {
  return (
    <div className="flex h-screen bg-zinc-900 text-zinc-100">
      <nav className="w-56 border-r border-zinc-800 flex flex-col">
        <div className="p-4 border-b border-zinc-800">
          <h1 className="text-lg font-bold tracking-tight">
            <span className="text-emerald-400">Git</span>Switch
          </h1>
        </div>
        <div className="flex-1 p-3 space-y-1">
          <SidebarLink to="/" icon={<LayoutDashboard size={18} />}>
            Profiles
          </SidebarLink>
          <SidebarLink to="/ssh" icon={<Key size={18} />}>
            SSH Keys
          </SidebarLink>
          <SidebarLink to="/github" icon={<GitHubIcon size={18} />}>
            GitHub Auth
          </SidebarLink>
          <SidebarLink to="/settings" icon={<SettingsIcon size={18} />}>
            Settings
          </SidebarLink>
        </div>
        <div className="p-3 border-t border-zinc-800">
          <p className="text-xs text-zinc-600">v0.1.0</p>
        </div>
      </nav>

      <main className="flex-1 overflow-y-auto p-6">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/profile/new" element={<ProfileForm />} />
          <Route path="/profile/:id/edit" element={<ProfileForm />} />
          <Route path="/ssh" element={<SSHWizard />} />
          <Route path="/github" element={<GitHubAuth />} />
          <Route path="/settings" element={<Settings />} />
        </Routes>
      </main>
    </div>
  );
}

function SidebarLink({
  to,
  icon,
  children,
}: {
  to: string;
  icon: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <NavLink
      to={to}
      end
      className={({ isActive }) =>
        `flex items-center gap-3 px-3 py-2 rounded-lg text-sm font-medium transition-colors ${
          isActive
            ? "bg-zinc-800 text-emerald-400"
            : "text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/50"
        }`
      }
    >
      {icon}
      {children}
    </NavLink>
  );
}

export default App;
