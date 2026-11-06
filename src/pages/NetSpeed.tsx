import { useEffect, useState } from "react";
import { Wifi, Cable, Smartphone, Network, Gauge, Loader2, MapPin, Settings2 } from "lucide-react";
import { Card } from "../components/Card";
import { Button } from "../components/Button";
import { Checkbox } from "../components/Checkbox";
import { Segmented, cleanError } from "../components/github/shared";
import { SpeedGraph, DOWN_COLOUR, UP_COLOUR } from "../components/netspeed/SpeedGraph";
import { speed, amount, link, mbps } from "../components/netspeed/format";
import * as api from "../lib/api";

const RUNNING = ["latency", "download", "upload"];

function KindIcon({ kind, size = 16 }: { kind: api.NetKind; size?: number }) {
  if (kind === "wifi") return <Wifi size={size} />;
  if (kind === "ethernet") return <Cable size={size} />;
  if (kind === "cellular") return <Smartphone size={size} />;
  return <Network size={size} />;
}

const KIND: Record<api.NetKind, string> = { wifi: "Wi-Fi", ethernet: "Ethernet", cellular: "Mobile", other: "Other", virtual: "Virtual" };

/** Where the speed can be shown outside the window, in this system's words. */
function where(platform: api.NetSnapshot["platform"]): string {
  return platform === "windows" ? "the taskbar" : platform === "macos" ? "the menu bar" : "the top bar";
}

/**
 * The Net speed tab: how fast the connection is moving data now, the
 * connection itself, the last two minutes, what was used, a speed test, and
 * showing the speed in the taskbar / menu bar / top bar.
 */
export function NetSpeed() {
  const [snap, setSnap] = useState<api.NetSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [test, setTest] = useState<api.SpeedTest | null>(null);
  const [showVirtual, setShowVirtual] = useState(false);
  const interval = snap?.settings.interval_ms ?? 1000;

  useEffect(() => {
    let alive = true;
    const read = () =>
      api.netspeedSnapshot().then(
        (s) => {
          if (!alive) return;
          setSnap(s);
          setError(null);
        },
        (e) => alive && setError(cleanError(e))
      );
    read();
    const t = window.setInterval(read, interval);
    return () => {
      alive = false;
      window.clearInterval(t);
    };
  }, [interval]);

  useEffect(() => {
    api.speedtestStatus().then(setTest, () => {});
  }, []);
  const testing = !!test && RUNNING.includes(test.phase);
  useEffect(() => {
    if (!testing) return;
    const t = window.setInterval(() => api.speedtestStatus().then(setTest, () => {}), 300);
    return () => window.clearInterval(t);
  }, [testing]);

  async function save(change: Partial<api.NetSettings>) {
    if (!snap) return;
    const next = { ...snap.settings, ...change };
    setSnap({ ...snap, settings: next });
    try {
      const s = await api.netspeedSetSettings(next);
      setSnap((p) => (p ? { ...p, settings: s } : p));
    } catch (e) {
      setError(cleanError(e));
    }
  }

  async function startTest() {
    setError(null);
    try {
      await api.speedtestStart();
      setTest({ phase: "latency", progress: 0, latency_ms: null, down_bps: null, up_bps: null, used_bytes: 0, error: null, at: null });
    } catch (e) {
      setError(cleanError(e));
    }
  }

  if (!snap) {
    return (
      <div className="space-y-5">
        <h1 className="text-2xl font-bold text-zinc-100">Net speed</h1>
        {error ? <p className="text-sm text-red-400">{error}</p> : <p className="flex items-center gap-2 text-sm text-zinc-400"><Loader2 size={14} className="animate-spin" /> Measuring…</p>}
      </div>
    );
  }

  const s = snap.settings;
  const bits = s.unit === "bits";
  const conn = snap.connection;
  const physical = snap.interfaces.filter((i) => i.kind !== "virtual");
  const virtual = snap.interfaces.filter((i) => i.kind === "virtual");
  const place = where(snap.platform);
  const showsOutside = s.tray || (snap.platform !== "windows" && s.bar);

  return (
    <div className="space-y-5" data-netspeed>
      <div>
        <h1 className="text-2xl font-bold text-zinc-100">Net speed</h1>
        <p className="text-sm text-zinc-400 mt-1">How fast your connection is moving data right now, and how much it used.</p>
      </div>
      {error && <p className="text-sm text-red-400 whitespace-pre-wrap" data-netspeed-error>{error}</p>}

      <div className="grid gap-4 md:grid-cols-3">
        {(
          [
            ["Download", snap.down, DOWN_COLOUR, "down"],
            ["Upload", snap.up, UP_COLOUR, "up"],
          ] as const
        ).map(([label, v, colour, key]) => (
          <Card key={key}>
            <div className="space-y-1" data-speed={key}>
              <p className="flex items-center gap-1.5 text-xs text-zinc-400">
                <span className="inline-block w-2 h-2 rounded-full" style={{ background: colour }} /> {label}
              </p>
              <p className="text-3xl font-semibold text-zinc-100 tabular-nums">{speed(v, bits)}</p>
              <p className="text-xs text-zinc-500 truncate">{snap.source_name}</p>
            </div>
          </Card>
        ))}
        <Card>
          <div className="space-y-1.5 text-xs" data-connection>
            <p className="text-zinc-400">Connection in use</p>
            {conn ? (
              <>
                <p className="flex items-center gap-2 text-sm text-zinc-100">
                  <KindIcon kind={conn.kind} /> {conn.name}
                  {link(conn.link_bps) && <span className="text-xs text-zinc-500">{link(conn.link_bps)} link</span>}
                </p>
                {conn.detail && <p className="text-zinc-500 truncate" title={conn.detail}>{conn.detail}</p>}
                {snap.wifi && (
                  <div className="space-y-0.5 text-zinc-400" data-wifi>
                    {snap.wifi.ssid && <p>Network: <span className="text-zinc-200">{snap.wifi.ssid}</span></p>}
                    {snap.wifi.signal != null && (
                      <p>
                        Signal: <span className="text-zinc-200">{snap.wifi.signal}%</span>
                        {snap.wifi.rssi_dbm != null && <span className="text-zinc-500"> ({snap.wifi.rssi_dbm} dBm)</span>}
                      </p>
                    )}
                    {snap.wifi.rx_mbps != null && (
                      <p>
                        Wi-Fi rate: <span className="text-zinc-200">{Math.round(snap.wifi.rx_mbps)} Mbps</span>
                        {snap.wifi.tx_mbps != null && <span className="text-zinc-500"> down, {Math.round(snap.wifi.tx_mbps)} Mbps up</span>}
                      </p>
                    )}
                    {snap.wifi.note && (
                      <p className="text-amber-300/90" data-wifi-note>
                        {snap.wifi.note}{" "}
                        {snap.wifi.fix === "location" && snap.platform !== "linux" && (
                          <button type="button" className="text-emerald-400 hover:underline cursor-pointer inline-flex items-center gap-1" onClick={() => api.netspeedOpenSettings("location").catch((e) => setError(cleanError(e)))}>
                            <MapPin size={11} /> Open the location settings
                          </button>
                        )}
                      </p>
                    )}
                  </div>
                )}
              </>
            ) : (
              <p className="text-sm text-zinc-300">Not connected</p>
            )}
          </div>
        </Card>
      </div>

      <Card>
        <SpeedGraph history={snap.history} bits={bits} />
      </Card>

      <div className="grid gap-4 md:grid-cols-2">
        <Card>
          <div className="space-y-2 text-xs" data-usage>
            <p className="text-sm text-zinc-200 font-medium">Data used</p>
            <table className="w-full text-left tabular-nums">
              <thead className="text-zinc-500">
                <tr>
                  <th className="font-normal py-0.5" />
                  <th className="font-normal">Downloaded</th>
                  <th className="font-normal">Uploaded</th>
                </tr>
              </thead>
              <tbody className="text-zinc-200">
                {(
                  [
                    ["Today", snap.usage.today],
                    ["This month", snap.usage.month],
                    ["Since GitSwitch started", snap.usage.session],
                  ] as const
                ).map(([label, v]) => (
                  <tr key={label} data-usage-row={label}>
                    <td className="py-0.5 text-zinc-400">{label}</td>
                    <td>{amount(v[0])}</td>
                    <td>{amount(v[1])}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className="text-zinc-500">Your Wi-Fi, Ethernet and mobile connections — not VPNs or virtual machines, which carry the same data again. What moves while GitSwitch is closed is added when it starts again, unless the PC restarted in between.</p>
          </div>
        </Card>

        <Card>
          <div className="space-y-2 text-xs" data-speedtest>
            <p className="text-sm text-zinc-200 font-medium flex items-center gap-2">
              <Gauge size={15} /> Speed test
            </p>
            <p className="text-zinc-500">How fast your internet can go right now, measured with Cloudflare's speed test (speed.cloudflare.com). It uses up to about 100 MB.</p>
            {test && RUNNING.includes(test.phase) && (
              <div className="space-y-1" data-speedtest-running>
                <p className="flex items-center gap-2 text-zinc-300">
                  <Loader2 size={13} className="animate-spin" /> {test.phase === "latency" ? "Measuring the delay…" : test.phase === "download" ? "Measuring the download…" : "Measuring the upload…"}
                </p>
                <div className="h-1.5 rounded bg-zinc-700 overflow-hidden">
                  <div className="h-full bg-emerald-500 transition-all" style={{ width: `${Math.round(test.progress * 100)}%` }} />
                </div>
              </div>
            )}
            {test && (test.phase === "done" || test.down_bps != null) && (
              <div className="grid grid-cols-3 gap-2" data-speedtest-result>
                <div>
                  <p className="text-zinc-500">Download</p>
                  <p className="text-lg text-zinc-100 tabular-nums">{mbps(test.down_bps)}</p>
                </div>
                <div>
                  <p className="text-zinc-500">Upload</p>
                  <p className="text-lg text-zinc-100 tabular-nums">{mbps(test.up_bps)}</p>
                </div>
                <div>
                  <p className="text-zinc-500">Delay</p>
                  <p className="text-lg text-zinc-100 tabular-nums">{test.latency_ms != null ? `${Math.round(test.latency_ms)} ms` : "—"}</p>
                </div>
              </div>
            )}
            {test && (test.phase === "failed" || test.phase === "cancelled") && (
              <p className={test.phase === "failed" ? "text-red-400" : "text-zinc-400"} data-speedtest-error>
                {test.phase === "cancelled" ? "Stopped." : test.error}
              </p>
            )}
            {test?.phase === "done" && test.used_bytes > 0 && <p className="text-zinc-500">It used {amount(test.used_bytes)}.</p>}
            <div className="flex items-center gap-2">
              {testing ? (
                <Button variant="secondary" size="sm" onClick={() => api.speedtestCancel().catch(() => {})} data-speedtest-cancel>
                  Stop
                </Button>
              ) : (
                <Button size="sm" onClick={startTest} data-speedtest-start>
                  <Gauge size={14} /> {test?.phase === "done" ? "Test again" : "Test my internet speed"}
                </Button>
              )}
            </div>
          </div>
        </Card>
      </div>

      <Card>
        <div className="space-y-2 text-xs" data-adapters>
          <p className="text-sm text-zinc-200 font-medium">Adapters</p>
          <table className="w-full text-left tabular-nums">
            <thead className="text-zinc-500">
              <tr>
                <th className="font-normal py-0.5">Adapter</th>
                <th className="font-normal">Link</th>
                <th className="font-normal">Download</th>
                <th className="font-normal">Upload</th>
              </tr>
            </thead>
            <tbody>
              {[...physical, ...(showVirtual ? virtual : [])].map((i) => (
                <tr key={i.id} className={i.up ? "text-zinc-200" : "text-zinc-500"} data-adapter={i.name}>
                  <td className="py-1">
                    <span className="flex items-center gap-2 min-w-0">
                      <KindIcon kind={i.kind} size={13} />
                      <span className="truncate" title={i.detail ?? ""}>{i.name}</span>
                      <span className="text-[10px] px-1.5 rounded border border-zinc-700 text-zinc-400 shrink-0">{KIND[i.kind]}</span>
                      {i.in_use && <span className="text-[10px] px-1.5 rounded border border-emerald-500/40 text-emerald-300 shrink-0">in use</span>}
                      {!i.up && <span className="text-zinc-500 shrink-0">not connected</span>}
                    </span>
                  </td>
                  <td>{link(i.link_bps) ?? "—"}</td>
                  <td>{speed(i.down, bits)}</td>
                  <td>{speed(i.upload, bits)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {virtual.length > 0 && (
            <button type="button" className="text-emerald-400 hover:underline cursor-pointer" onClick={() => setShowVirtual((v) => !v)} data-show-virtual>
              {showVirtual ? "Hide virtual adapters" : `Show virtual adapters (${virtual.length}) — VPNs, virtual machines`}
            </button>
          )}
        </div>
      </Card>

      <Card>
        <div className="space-y-3 text-xs" data-netspeed-settings>
          <p className="text-sm text-zinc-200 font-medium flex items-center gap-2">
            <Settings2 size={15} /> Show the speed outside GitSwitch
          </p>
          <label className="flex items-center gap-2 cursor-pointer text-zinc-200 text-sm">
            <Checkbox checked={s.tray} onChange={(v) => save({ tray: v })} aria-label={`Show the speed in ${place}`} />
            Show the speed in {place}
          </label>
          {snap.platform === "windows" && (
            <div className="flex items-center gap-3 flex-wrap pl-6">
              <span className="text-zinc-400">How</span>
              <Segmented
                label="How the speed is shown"
                value={s.style}
                disabled={!s.tray}
                onChange={(v) => save({ style: v })}
                options={[
                  { value: "icon", label: "Numbers in the icon" },
                  { value: "bar", label: "Speed bar on the taskbar" },
                  { value: "both", label: "Both" },
                ]}
              />
            </div>
          )}
          {snap.platform === "windows" && s.style !== "icon" && (
            <div className="flex items-center gap-3 flex-wrap pl-6">
              <span className="text-zinc-400">Where</span>
              <Segmented
                label="Where the speed bar goes"
                value={s.bar_place ?? "taskbar"}
                disabled={!s.tray}
                onChange={(v) => save({ bar_place: v })}
                options={[
                  { value: "taskbar", label: "On the taskbar" },
                  { value: "anywhere", label: "Anywhere on the screen" },
                ]}
              />
            </div>
          )}
          {snap.platform === "windows" && s.tray && s.style !== "icon" && (
            <p className="text-zinc-500 pl-6" data-bar-tip={s.bar_place ?? "taskbar"}>
              {(s.bar_place ?? "taskbar") === "taskbar"
                ? "The speed bar sits on the taskbar and keeps clear of the app icons and the clock: when they need its place, it moves to the nearest free spot. Drag it along the taskbar to choose where it goes; double-click it to come back here."
                : "The speed bar floats over other windows: drag it anywhere on the screen and it stays where you leave it. Double-click it to come back here."}{" "}
              <button type="button" data-bar-reset className="text-emerald-400 hover:underline cursor-pointer" onClick={() => api.netspeedBarReset().catch((e) => setError(cleanError(e)))}>
                Put it back beside the clock
              </button>
            </p>
          )}
          {snap.platform !== "windows" && (
            <label className="flex items-center gap-2 cursor-pointer text-zinc-300">
              <Checkbox checked={s.bar} onChange={(v) => save({ bar: v })} aria-label="Also show a floating speed bar" />
              Also show a floating speed bar (drag it anywhere)
            </label>
          )}
          <div className="grid gap-3 sm:grid-cols-2">
            <div className="flex items-center gap-3 flex-wrap">
              <span className="text-zinc-400 w-20">Show</span>
              <Segmented
                label="Show"
                value={s.show}
                onChange={(v) => save({ show: v })}
                options={[
                  { value: "both", label: "Both" },
                  { value: "down", label: "Download" },
                  { value: "up", label: "Upload" },
                ]}
              />
            </div>
            <div className="flex items-center gap-3 flex-wrap">
              <span className="text-zinc-400 w-20">Units</span>
              <Segmented
                label="Units"
                value={s.unit}
                onChange={(v) => save({ unit: v })}
                options={[
                  { value: "bytes", label: "MB/s" },
                  { value: "bits", label: "Mbps" },
                ]}
              />
            </div>
            <div className="flex items-center gap-3 flex-wrap">
              <span className="text-zinc-400 w-20">Update every</span>
              <Segmented
                label="Update every"
                value={String(s.interval_ms) as "1000" | "2000" | "5000"}
                onChange={(v) => save({ interval_ms: Number(v) })}
                options={[
                  { value: "1000", label: "1 s" },
                  { value: "2000", label: "2 s" },
                  { value: "5000", label: "5 s" },
                ]}
              />
            </div>
            <label className="flex items-center gap-3 flex-wrap">
              <span className="text-zinc-400 w-20">Connection</span>
              <select
                value={s.source}
                onChange={(e) => save({ source: e.target.value })}
                aria-label="Connection"
                className="rounded-md bg-zinc-800 border border-zinc-700 px-2 py-1 text-zinc-200"
              >
                <option value="auto">The one in use</option>
                <option value="all">All my connections</option>
                {snap.interfaces.map((i) => (
                  <option key={i.id} value={i.id}>
                    {i.name}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {snap.platform === "windows" && s.tray && s.style !== "bar" && (
            <p className="text-zinc-500" data-taskbar-tip>
              Windows may hide a new tray icon behind the ^ by the clock. To keep it in sight, turn GitSwitch on under Other system tray icons.{" "}
              <button type="button" className="text-emerald-400 hover:underline cursor-pointer" onClick={() => api.netspeedOpenSettings("taskbar").catch((e) => setError(cleanError(e)))}>
                Open the taskbar settings
              </button>
            </p>
          )}
          {snap.platform === "linux" && s.tray && <p className="text-zinc-500">The top bar shows it next to GitSwitch's icon with the AppIndicator extension (on by default in Ubuntu).</p>}
          {showsOutside && <p className="text-zinc-500" data-keeps-running>While the speed is shown, closing the window keeps GitSwitch running; quit it from its icon's menu.</p>}
        </div>
      </Card>
    </div>
  );
}
