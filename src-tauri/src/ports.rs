//! The Ports page: every listening port on this machine, the process that
//! holds it, the repository folder it runs from, and a way to stop it.
//!
//! Two copies of one project collide through the machine they share — a dev
//! server, a database, a Compose project started from copy 1 is what the
//! agent in copy 2 finds "already running". This module names who holds a
//! port and where it was started, so the person can stop the right one.
//!
//! Every fact comes from the OS's own tools, parsed from captured output:
//! - macOS: `lsof` (own processes, full names, working directories),
//!   `netstat` (every process, root's included — lsof shows none of root's
//!   without an administrator), `ps` (owner, start time, full path, the
//!   environment of own non-Apple processes).
//! - Linux: `ss -tulpnH` and `/proc`.
//! - Windows: one PowerShell script (Get-NetTCPConnection, Get-NetUDPEndpoint,
//!   Get-Process) as JSON, with `netstat -ano` + `tasklist` as the fallback.
//! - Docker, on every OS: `docker ps`, so a port published by a container is
//!   stopped through Docker — never by killing Docker Desktop's own process,
//!   which is what holds the socket on macOS.
//!
//! Nothing here elevates: a root-owned listener is named, with what to run.

use crate::error::AppError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const LIST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg_attr(not(windows), allow(dead_code))]
const POWERSHELL_TIMEOUT: Duration = Duration::from_secs(15);
const DOCKER_TIMEOUT: Duration = Duration::from_secs(3);
const DOCKER_BACKOFF: Duration = Duration::from_secs(60);
const DOCKER_STOP_TIMEOUT: Duration = Duration::from_secs(25);
const KILL_GRACE: Duration = Duration::from_secs(3);
const KILL_HARD_GRACE: Duration = Duration::from_secs(1);

/// Docker Desktop's own processes: they hold every published port on macOS
/// and Windows, and ending one takes Docker down with it.
const DOCKER_PROCESS_NAMES: &[&str] = &[
    "com.docker.backend",
    "com.docker.vpnkit",
    "vpnkit",
    "com.docker.supervisor",
    "docker-proxy",
    "Docker Desktop",
    "Docker",
];

// ---------------------------------------------------------------------------
// What the page gets
// ---------------------------------------------------------------------------

/// One listening port, with everything the OS would say about its holder.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Listener {
    /// "tcp:18789:725" — stable across refreshes, for the page's rows.
    pub key: String,
    pub proto: String,
    pub port: u16,
    /// Every address it is bound on, IPv4 and IPv6 merged: ["127.0.0.1", "::1"], ["*"].
    pub addresses: Vec<String>,
    /// loopback | all | lan
    pub scope: String,
    /// None: another user's socket the OS shows no owner for, or a Docker port with no socket.
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub exe: Option<String>,
    /// The folder it runs from; None on Windows and for other users' processes.
    pub cwd: Option<String>,
    /// RFC 3339.
    pub started_at: Option<String>,
    pub user: Option<String>,
    /// Started from a GitSwitch terminal (GITSWITCH_TERMINAL=1 in its environment).
    pub ours: bool,
    /// The GITSWITCH_WORKSPACE id in its environment, when one is.
    pub workspace: Option<String>,
    pub clone: Option<CloneRef>,
    pub container: Option<ContainerRef>,
    pub killable: bool,
    /// When not killable: why, in the person's words.
    pub why_not: Option<String>,
}

/// The repository in a profile folder that a process runs inside.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct CloneRef {
    pub path: String,
    pub name: String,
    pub profile_name: String,
}

/// The container a port is published by, and its Compose project when it has one.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ContainerRef {
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub working_dir: Option<String>,
    pub container_port: Option<u16>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PortsReport {
    /// macos | linux | windows
    pub os: String,
    pub rows: Vec<Listener>,
    /// What this listing could not see, one sentence each.
    pub notes: Vec<String>,
    /// ok | missing | unavailable | timeout | skipped
    pub docker: String,
    pub took_ms: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct KillOutcome {
    pub pid: u32,
    pub ended: bool,
    /// term | kill | taskkill | taskkill-force
    pub how: String,
    /// The whole process group was signalled (the target led it).
    pub group: bool,
    pub message: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ContainerStopOutcome {
    pub stopped: Vec<String>,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Raw facts, before they are joined
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// One socket as a tool reported it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawSock {
    pub pid: Option<u32>,
    pub name: Option<String>,
    pub proto: Proto,
    pub addr: String,
    pub port: u16,
}

/// What `ps` says about a process, any owner.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct ProcInfo {
    pub uid: u32,
    pub user: String,
    pub pgid: u32,
    pub started_at: Option<String>,
    pub comm: String,
}

/// The GitSwitch marks in a process's environment.
#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) struct EnvMarks {
    pub terminal: bool,
    pub workspace: Option<String>,
}

/// A port a container publishes on the host.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Published {
    pub host_ip: String,
    pub host_port: u16,
    pub container_port: u16,
    pub proto: Proto,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Container {
    pub id: String,
    pub name: String,
    pub project: Option<String>,
    pub service: Option<String>,
    pub working_dir: Option<String>,
    pub published: Vec<Published>,
}

// ---------------------------------------------------------------------------
// Parsers — pure, unit-tested on captured output
// ---------------------------------------------------------------------------

/// `host:port` as lsof and ss print it: `127.0.0.1:27017`, `[::1]:27017`,
/// `*:7000`, `0.0.0.0:22`, `127.0.0.53%lo:53`. Wildcards become `*`.
fn split_host_port(s: &str) -> Option<(String, u16)> {
    let (host, port) = s.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let host = match host {
        "*" | "0.0.0.0" | "::" | "" => "*".to_string(),
        h => h.to_string(),
    };
    Some((host, port))
}

/// `lsof -nP -iTCP -sTCP:LISTEN -iUDP -F pcnPT`: one field per line, the
/// first byte says which. A process starts with `p`, each socket with `f`;
/// `T` lines carry the TCP state. Only LISTEN sockets and bound UDP sockets
/// count; `*:*` (unbound) and `->` (connected) UDP sockets do not.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_lsof_sockets(text: &str) -> Vec<RawSock> {
    let mut out = Vec::new();
    let mut pid: Option<u32> = None;
    let mut name: Option<String> = None;
    let mut proto: Option<Proto> = None;
    let mut addr: Option<(String, u16)> = None;
    let mut listen = false;
    let flush = |pid: Option<u32>, name: &Option<String>, proto: Option<Proto>, addr: &mut Option<(String, u16)>, listen: &mut bool, out: &mut Vec<RawSock>| {
        if let (Some(pid), Some(proto), Some((host, port))) = (pid, proto, addr.take()) {
            let keep = match proto {
                Proto::Tcp => *listen,
                Proto::Udp => true,
            };
            if keep {
                out.push(RawSock { pid: Some(pid), name: name.clone(), proto, addr: host, port });
            }
        }
        *listen = false;
    };
    for line in text.lines() {
        let Some(tag) = line.chars().next() else { continue };
        let rest = &line[1..];
        match tag {
            'p' => {
                flush(pid, &name, proto, &mut addr, &mut listen, &mut out);
                pid = rest.trim().parse().ok();
                name = None;
                proto = None;
            }
            'c' => name = Some(rest.to_string()),
            'f' => {
                flush(pid, &name, proto, &mut addr, &mut listen, &mut out);
                proto = None;
            }
            'P' => {
                proto = match rest.trim() {
                    "TCP" => Some(Proto::Tcp),
                    "UDP" => Some(Proto::Udp),
                    _ => None,
                }
            }
            'n' => {
                let n = rest.trim();
                addr = if n == "*:*" || n.contains("->") { None } else { split_host_port(n) };
            }
            'T' if rest.trim() == "ST=LISTEN" => listen = true,
            _ => {}
        }
    }
    flush(pid, &name, proto, &mut addr, &mut listen, &mut out);
    out
}

/// `lsof -a -p <pids> -d cwd -Fn`: `p<pid>`, `fcwd`, `n<path>`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_lsof_cwd(text: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let mut pid: Option<u32> = None;
    let mut in_cwd = false;
    for line in text.lines() {
        let Some(tag) = line.chars().next() else { continue };
        let rest = &line[1..];
        match tag {
            'p' => {
                pid = rest.trim().parse().ok();
                in_cwd = false;
            }
            'f' => in_cwd = rest.trim() == "cwd",
            'n' if in_cwd => {
                if let Some(p) = pid {
                    out.insert(p, rest.to_string());
                }
            }
            _ => {}
        }
    }
    out
}

/// macOS `netstat -anv -p tcp|udp`: every socket of every user, with a
/// `process:pid` column (names cut to 16 characters). From the right, eight
/// bookkeeping columns follow `process:pid`; four numeric ones precede it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_netstat_macos(text: &str, proto: Proto) -> Vec<RawSock> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("Active") || t.starts_with("Proto") || t.is_empty() {
            continue;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        if toks.len() < 14 {
            continue;
        }
        let family = toks[0];
        let is_tcp = family.starts_with("tcp");
        let is_udp = family.starts_with("udp");
        if (proto == Proto::Tcp && !is_tcp) || (proto == Proto::Udp && !is_udp) {
            continue;
        }
        let local = toks[3];
        let foreign = toks[4];
        // After the foreign address: tcp has a state token; udp has none.
        let after = if is_tcp {
            if toks.get(5) != Some(&"LISTEN") {
                continue;
            }
            6
        } else {
            if foreign != "*.*" {
                continue;
            }
            5
        };
        // Four numeric columns, then process:pid, then eight columns.
        let procpid_idx = after + 4;
        if toks.len() < procpid_idx + 1 + 8 {
            continue;
        }
        // The process name may hold spaces: everything between the four
        // numbers and the last eight columns is the name.
        let name_tokens = &toks[procpid_idx..toks.len() - 8];
        let joined = name_tokens.join(" ");
        let Some((name, pid)) = joined.rsplit_once(':') else { continue };
        let Some(pid) = pid.parse::<u32>().ok() else { continue };
        // Local address: the port follows the last dot.
        let Some((host, port)) = local.rsplit_once('.') else { continue };
        let Some(port) = port.parse::<u16>().ok() else { continue };
        let host = match host {
            "*" => "*".to_string(),
            h => h.to_string(),
        };
        out.push(RawSock { pid: Some(pid), name: Some(name.trim().to_string()), proto, addr: host, port });
    }
    out
}

/// `ps -o pid=,uid=,user=,pgid=,lstart=,comm= -p …`: four columns, then the
/// five words of `lstart`, then the command (spaces allowed).
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_ps_table(text: &str) -> HashMap<u32, ProcInfo> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 10 {
            continue;
        }
        let Some(pid) = toks[0].parse::<u32>().ok() else { continue };
        let Some(uid) = toks[1].parse::<u32>().ok() else { continue };
        let user = toks[2].to_string();
        let Some(pgid) = toks[3].parse::<u32>().ok() else { continue };
        let lstart = toks[4..9].join(" ");
        let comm = toks[9..].join(" ");
        out.insert(pid, ProcInfo { uid, user, pgid, started_at: parse_lstart(&lstart), comm });
    }
    out
}

/// `Sun Oct 11 00:09:02 2026` → RFC 3339 in the machine's local time.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn parse_lstart(s: &str) -> Option<String> {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let naive = NaiveDateTime::parse_from_str(s.trim(), "%a %b %e %H:%M:%S %Y").ok()?;
    Local.from_local_datetime(&naive).single().map(|d| d.to_rfc3339())
}

/// `ps -Eww -o pid=,command= -p …`: the pid, then the command and its
/// environment as `KEY=value` words. Only the two GitSwitch marks are read.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_ps_env(text: &str) -> HashMap<u32, EnvMarks> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let t = line.trim_start();
        let Some((pid, rest)) = t.split_once(' ') else { continue };
        let Some(pid) = pid.parse::<u32>().ok() else { continue };
        let mut marks = EnvMarks::default();
        for w in rest.split_whitespace() {
            if w == "GITSWITCH_TERMINAL=1" {
                marks.terminal = true;
            } else if let Some(v) = w.strip_prefix("GITSWITCH_WORKSPACE=") {
                if !v.is_empty() {
                    marks.workspace = Some(v.to_string());
                }
            }
        }
        out.insert(pid, marks);
    }
    out
}

/// Linux `ss -tulpnH`: netid, state, recv-q, send-q, local, peer, and for
/// sockets whose process the caller may read, `users:(("name",pid=N,fd=M),…)`.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn parse_ss(text: &str) -> Vec<RawSock> {
    let mut out = Vec::new();
    for line in text.lines() {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 6 {
            continue;
        }
        let proto = match toks[0] {
            "tcp" => Proto::Tcp,
            "udp" => Proto::Udp,
            _ => continue,
        };
        match proto {
            Proto::Tcp if toks[1] != "LISTEN" => continue,
            Proto::Udp if toks[1] != "UNCONN" || !matches!(toks[5], "0.0.0.0:*" | "[::]:*" | "*:*") => continue,
            _ => {}
        }
        let Some((host, port)) = split_host_port(toks[4]) else { continue };
        let mut pid: Option<u32> = None;
        let mut name: Option<String> = None;
        if let Some(users) = toks.get(6).filter(|u| u.starts_with("users:")) {
            // The smallest pid is the master of a pre-forked server.
            for part in users.split("),(") {
                let n = part.split('"').nth(1).map(str::to_string);
                let p = part.split("pid=").nth(1).and_then(|s| s.split(',').next()).and_then(|s| s.parse::<u32>().ok());
                if let Some(p) = p {
                    if pid.is_none_or(|cur| p < cur) {
                        pid = Some(p);
                        name = n;
                    }
                }
            }
        }
        out.push(RawSock { pid, name, proto, addr: host, port });
    }
    out
}

/// A PowerShell list of one element arrives as an object, not an array.
#[cfg_attr(not(windows), allow(dead_code))]
fn one_or_many<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        Many(Vec<T>),
        One(T),
        None(()),
    }
    Ok(match OneOrMany::<T>::deserialize(d)? {
        OneOrMany::Many(v) => v,
        OneOrMany::One(x) => vec![x],
        OneOrMany::None(()) => Vec::new(),
    })
}

#[derive(Deserialize, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct PsSock {
    pub a: String,
    pub p: u16,
    pub pid: u32,
}

#[derive(Deserialize, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct PsProc {
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub started: Option<String>,
}

#[derive(Deserialize, Debug)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct PsReport {
    #[serde(default, deserialize_with = "one_or_many")]
    pub tcp: Vec<PsSock>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub udp: Vec<PsSock>,
    #[serde(default, deserialize_with = "one_or_many")]
    pub procs: Vec<PsProc>,
}

/// The one PowerShell line that lists Windows' listeners as JSON. No double
/// quote in it, so it travels as a single argument untouched.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) const POWERSHELL_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; $tcp=@(Get-NetTCPConnection -State Listen | ForEach-Object { [pscustomobject]@{a=[string]$_.LocalAddress;p=[int]$_.LocalPort;pid=[int]$_.OwningProcess} }); $udp=@(Get-NetUDPEndpoint | ForEach-Object { [pscustomobject]@{a=[string]$_.LocalAddress;p=[int]$_.LocalPort;pid=[int]$_.OwningProcess} }); $ids=@($tcp.pid + $udp.pid | Sort-Object -Unique); $procs=@(foreach ($i in $ids) { $x=Get-Process -Id $i; if ($x) { $st=$null; try { $st=$x.StartTime.ToUniversalTime().ToString('o') } catch {}; [pscustomobject]@{pid=[int]$i;name=[string]$x.Name;path=[string]$x.Path;started=$st} } }); [pscustomobject]@{tcp=$tcp;udp=$udp;procs=$procs} | ConvertTo-Json -Compress -Depth 4";

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_powershell(text: &str) -> Option<PsReport> {
    let start = text.find('{')?;
    serde_json::from_str(&text[start..]).ok()
}

/// Windows `netstat -ano`: TCP rows need LISTENING; UDP rows have no state.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_netstat_windows(text: &str) -> Vec<RawSock> {
    let mut out = Vec::new();
    let mut started = false;
    for line in text.lines() {
        let t = line.trim();
        if !started {
            started = t.starts_with("Proto");
            continue;
        }
        let toks: Vec<&str> = t.split_whitespace().collect();
        let (proto, local, pid) = match toks.first().copied() {
            Some("TCP") if toks.len() >= 5 && toks[3] == "LISTENING" => (Proto::Tcp, toks[1], toks[4]),
            Some("UDP") if toks.len() >= 4 => (Proto::Udp, toks[1], toks[3]),
            _ => continue,
        };
        let Some((host, port)) = split_host_port(local) else { continue };
        out.push(RawSock { pid: pid.parse().ok(), name: None, proto, addr: host, port });
    }
    out
}

/// `tasklist /FO CSV /NH`: `"name.exe","pid","session","#","mem"`.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_tasklist_csv(text: &str) -> HashMap<u32, String> {
    let mut out = HashMap::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split("\",\"").collect();
        if fields.len() < 2 {
            continue;
        }
        let name = fields[0].trim_start_matches('"');
        let Some(pid) = fields[1].trim_end_matches('"').parse::<u32>().ok() else { continue };
        let name = name.strip_suffix(".exe").unwrap_or(name).to_string();
        out.insert(pid, name);
    }
    out
}

#[derive(Deserialize, Debug)]
struct DockerPs {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Names")]
    names: String,
    #[serde(rename = "Ports", default)]
    ports: String,
    #[serde(rename = "Labels", default)]
    labels: String,
}

/// `0.0.0.0:8080->80/tcp, [::]:8080->80/tcp, 3000/tcp, 3000-3005->3000-3005/tcp`.
/// Only published ports (with `->`) count; ranges are expanded pairwise.
pub(crate) fn parse_docker_ports(s: &str) -> Vec<Published> {
    let mut out = Vec::new();
    for item in s.split(',').map(str::trim).filter(|i| !i.is_empty()) {
        let Some((host, rest)) = item.split_once("->") else { continue };
        let Some((cport, proto)) = rest.rsplit_once('/') else { continue };
        let proto = match proto {
            "tcp" => Proto::Tcp,
            "udp" => Proto::Udp,
            _ => continue,
        };
        let Some((hip, hport)) = host.rsplit_once(':') else { continue };
        let hip = hip.trim_start_matches('[').trim_end_matches(']');
        let hip = match hip {
            "0.0.0.0" | "::" | "" => "*".to_string(),
            h => h.to_string(),
        };
        let range = |s: &str| -> Option<(u16, u16)> {
            match s.split_once('-') {
                Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
                None => {
                    let p: u16 = s.parse().ok()?;
                    Some((p, p))
                }
            }
        };
        let (Some((h0, h1)), Some((c0, c1))) = (range(hport), range(cport)) else { continue };
        for (i, hp) in (h0..=h1).enumerate() {
            let cp = c0.saturating_add(i as u16).min(c1);
            out.push(Published { host_ip: hip.clone(), host_port: hp, container_port: cp, proto });
        }
    }
    out
}

/// Docker's label string is comma-joined, but a value may hold commas
/// (`config_files=a.yml,b.yml`): a new pair starts only where the text up to
/// the next `=` looks like a label key.
pub(crate) fn parse_docker_labels(s: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut last: Option<String> = None;
    for piece in s.split(',') {
        let looks_like_pair = piece
            .split_once('=')
            .map(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/')))
            .unwrap_or(false);
        if looks_like_pair {
            let (k, v) = piece.split_once('=').unwrap();
            out.insert(k.to_string(), v.to_string());
            last = Some(k.to_string());
        } else if let Some(k) = &last {
            if let Some(v) = out.get_mut(k) {
                v.push(',');
                v.push_str(piece);
            }
        }
    }
    out
}

/// One `docker ps --format '{{json .}}'` line per container.
pub(crate) fn parse_docker_ps(text: &str) -> Vec<Container> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Ok(c) = serde_json::from_str::<DockerPs>(line) else { continue };
        let labels = parse_docker_labels(&c.labels);
        out.push(Container {
            id: c.id,
            name: c.names.split(',').next().unwrap_or("").to_string(),
            project: labels.get("com.docker.compose.project").cloned(),
            service: labels.get("com.docker.compose.service").cloned(),
            working_dir: labels.get("com.docker.compose.project.working_dir").cloned(),
            published: parse_docker_ports(&c.ports),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Joining the facts into rows
// ---------------------------------------------------------------------------

/// Everything a platform's listing gathered, before the join.
#[derive(Default, Debug)]
pub(crate) struct Gathered {
    pub socks: Vec<RawSock>,
    pub procs: HashMap<u32, ProcInfo>,
    pub cwds: HashMap<u32, String>,
    pub exes: HashMap<u32, String>,
    pub names: HashMap<u32, String>,
    pub marks: HashMap<u32, EnvMarks>,
    pub containers: Vec<Container>,
    /// The caller's own uid (unix); None on Windows.
    pub own_uid: Option<u32>,
    pub own_pid: u32,
    pub shell_pids: Vec<u32>,
    pub os: String,
}

/// The repository a folder is inside, when a profile folder owns it: walk up
/// to the nearest `.git` (a directory, or a file for a worktree or submodule).
pub(crate) fn clone_of(dir: &str, profs: &[crate::profiles::Profile]) -> Option<CloneRef> {
    let mut cur = PathBuf::from(dir);
    loop {
        if cur.join(".git").exists() {
            let root = crate::paths::norm(&cur.to_string_lossy());
            let owner = crate::commit_audit::owning_profile(&root, profs)?;
            return Some(CloneRef { name: crate::paths::base_name(&root), path: root, profile_name: owner.name.clone() });
        }
        if !cur.pop() {
            return None;
        }
    }
}

fn os_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "Linux"
    }
}

fn is_system_exe(exe: &str, os: &str) -> bool {
    let e = exe.replace('\\', "/");
    match os {
        "macos" => ["/System/", "/usr/libexec/", "/usr/sbin/", "/sbin/"].iter().any(|p| e.starts_with(p)),
        "linux" => ["/usr/lib/systemd/", "/lib/systemd/"].iter().any(|p| e.starts_with(p)),
        _ => {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()).replace('\\', "/").to_ascii_lowercase();
            e.to_ascii_lowercase().starts_with(&format!("{}/", root.trim_end_matches('/')))
        }
    }
}

const WINDOWS_SYSTEM_NAMES: &[&str] = &["System", "svchost", "services", "lsass", "wininit", "csrss", "smss", "winlogon", "spoolsv"];

/// `killable` and `why_not`, first reason wins — the texts the page shows.
pub(crate) fn why_not_killable(row: &Listener, g: &Gathered) -> Option<String> {
    if row.container.is_some() {
        return Some("Published by Docker — stop the container instead.".into());
    }
    let Some(pid) = row.pid else {
        return Some("Another user's process — the system shows no process id without an administrator.".into());
    };
    if pid < 100 {
        return Some(format!("A system process (pid {}) — not stoppable from here.", pid));
    }
    if pid == g.own_pid {
        return Some("This is GitSwitch itself.".into());
    }
    if g.shell_pids.contains(&pid) {
        return Some("The terminal drawer's own shell — close the drawer instead.".into());
    }
    if let (Some(own), Some(p)) = (g.own_uid, g.procs.get(&pid)) {
        if p.uid != own {
            return Some(format!("Owned by {} — needs an administrator: stop it from Terminal with sudo kill {}.", p.user, pid));
        }
    }
    if row.name.as_deref().is_some_and(|n| DOCKER_PROCESS_NAMES.contains(&n)) {
        return Some("Docker Desktop's own process — stop the container instead (Docker did not answer, so the container is unknown).".into());
    }
    if row.exe.as_deref().is_some_and(|e| is_system_exe(e, &g.os)) {
        return Some(format!("Part of {} — not stoppable from here.", os_name()));
    }
    if g.os == "windows" {
        if row.name.as_deref().is_some_and(|n| WINDOWS_SYSTEM_NAMES.contains(&n)) {
            return Some("Part of Windows — not stoppable from here.".into());
        }
        if row.exe.as_deref().is_none_or(str::is_empty) {
            return Some("Windows did not say what this process is (access denied) — likely a service; stop it from an administrator terminal.".into());
        }
    }
    None
}

/// Group the sockets by (proto, port, pid), merge their addresses, attach
/// what is known about the process and the container, and decide killability.
pub(crate) fn assemble(g: &Gathered, profs: &[crate::profiles::Profile]) -> Vec<Listener> {
    // (port, proto, pid) → (addresses, the name a tool gave).
    type GroupKey = (u16, &'static str, Option<u32>);
    let mut groups: BTreeMap<GroupKey, (Vec<String>, Option<String>)> = BTreeMap::new();
    for s in &g.socks {
        let e = groups.entry((s.port, s.proto.as_str(), s.pid)).or_insert_with(|| (Vec::new(), None));
        if !e.0.contains(&s.addr) {
            e.0.push(s.addr.clone());
        }
        if e.1.is_none() {
            e.1 = s.name.clone();
        }
    }
    let mut clone_memo: HashMap<String, Option<CloneRef>> = HashMap::new();
    let mut rows = Vec::new();
    let mut seen_published: HashSet<(u16, &'static str)> = HashSet::new();
    for ((port, proto, pid), (mut addresses, sock_name)) in groups {
        addresses.sort();
        let scope = if addresses.iter().all(|a| a == "*") {
            "all"
        } else if addresses.iter().all(|a| a == "::1" || a.starts_with("127.")) {
            "loopback"
        } else {
            "lan"
        };
        let info = pid.and_then(|p| g.procs.get(&p));
        let exe = pid.and_then(|p| g.exes.get(&p).cloned()).or_else(|| info.map(|i| i.comm.clone()).filter(|c| c.contains('/') || c.contains('\\')));
        let name = sock_name
            .or_else(|| pid.and_then(|p| g.names.get(&p).cloned()))
            .or_else(|| info.map(|i| crate::paths::base_name(&i.comm)));
        let cwd = pid.and_then(|p| g.cwds.get(&p).cloned());
        let marks = pid.and_then(|p| g.marks.get(&p)).cloned().unwrap_or_default();
        let container = g.containers.iter().find_map(|c| {
            c.published
                .iter()
                .find(|pb| pb.host_port == port && pb.proto.as_str() == proto)
                .filter(|_| pid.is_none() || name.as_deref().is_some_and(|n| DOCKER_PROCESS_NAMES.contains(&n)))
                .map(|pb| ContainerRef {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    project: c.project.clone(),
                    service: c.service.clone(),
                    working_dir: c.working_dir.clone(),
                    container_port: Some(pb.container_port),
                })
        });
        if container.is_some() {
            seen_published.insert((port, proto));
        }
        // A port published by a container belongs to the container's project
        // folder, not to Docker Desktop's own working directory, which is what
        // holds the socket on macOS.
        let clone_dir = container.as_ref().and_then(|c| c.working_dir.clone()).or_else(|| cwd.clone());
        let clone = clone_dir.and_then(|d| clone_memo.entry(d.clone()).or_insert_with(|| clone_of(&d, profs)).clone());
        let mut row = Listener {
            key: format!("{}:{}:{}", proto, port, pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into())),
            proto: proto.into(),
            port,
            addresses,
            scope: scope.into(),
            pid,
            name,
            exe,
            cwd,
            started_at: info.and_then(|i| i.started_at.clone()),
            user: info.map(|i| i.user.clone()),
            ours: marks.terminal,
            workspace: marks.workspace,
            clone,
            container,
            killable: false,
            why_not: None,
        };
        row.why_not = why_not_killable(&row, g);
        row.killable = row.why_not.is_none();
        rows.push(row);
    }
    // A container whose published port has no listening socket (Linux without
    // the userland proxy) still deserves a row, so the port is accounted for.
    for c in &g.containers {
        for pb in &c.published {
            let key = (pb.host_port, pb.proto.as_str());
            if seen_published.contains(&key) {
                continue;
            }
            seen_published.insert(key);
            let clone = c.working_dir.as_ref().and_then(|d| clone_memo.entry(d.clone()).or_insert_with(|| clone_of(d, profs)).clone());
            rows.push(Listener {
                key: format!("{}:{}:docker-{}", pb.proto.as_str(), pb.host_port, c.id),
                proto: pb.proto.as_str().into(),
                port: pb.host_port,
                addresses: vec![pb.host_ip.clone()],
                scope: if pb.host_ip == "*" { "all".into() } else if pb.host_ip.starts_with("127.") || pb.host_ip == "::1" { "loopback".into() } else { "lan".into() },
                pid: None,
                name: Some("docker".into()),
                exe: None,
                cwd: None,
                started_at: None,
                user: None,
                ours: false,
                workspace: None,
                clone,
                container: Some(ContainerRef {
                    id: c.id.clone(),
                    name: c.name.clone(),
                    project: c.project.clone(),
                    service: c.service.clone(),
                    working_dir: c.working_dir.clone(),
                    container_port: Some(pb.container_port),
                }),
                killable: false,
                why_not: Some("Published by Docker — stop the container instead.".into()),
            });
        }
    }
    rows.sort_by(|a, b| a.port.cmp(&b.port).then(a.proto.cmp(&b.proto)).then(a.pid.cmp(&b.pid)));
    rows
}

// ---------------------------------------------------------------------------
// Running the tools
// ---------------------------------------------------------------------------

/// A tool on PATH, or at one of the places the OS keeps it (`fix_path_env`
/// adds no `/usr/sbin`, where macOS keeps lsof and netstat).
fn tool(name: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let c = dir.join(if cfg!(windows) { format!("{}.exe", name) } else { name.to_string() });
            if c.is_file() {
                return Some(c);
            }
        }
    }
    fallbacks.iter().map(PathBuf::from).find(|p| p.is_file())
}

fn docker_tool() -> Option<PathBuf> {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    let win = format!("{}\\Docker\\Docker\\resources\\bin\\docker.exe", pf);
    tool("docker", &["/usr/local/bin/docker", "/opt/homebrew/bin/docker", &win])
}

/// Run a tool with a time limit; `None` when it is missing or over time.
async fn run_tool(program: &Path, args: &[&str], timeout: Duration) -> Option<(String, String, i32)> {
    let mut cmd = crate::spawn::async_command(program);
    cmd.args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    cmd.env("LC_ALL", "C");
    let out = tokio::time::timeout(timeout, cmd.output()).await.ok()?.ok()?;
    Some((String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned(), out.status.code().unwrap_or(-1)))
}

fn docker_retry_after() -> &'static Mutex<Option<Instant>> {
    static R: OnceLock<Mutex<Option<Instant>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

/// `docker ps`, never for longer than 3 s, and not again for a minute after
/// a timeout: a Docker Desktop that is stopped or starting must not make the
/// page slow on every tick.
async fn docker_containers() -> (Vec<Container>, &'static str, Option<String>) {
    let Some(docker) = docker_tool() else {
        return (Vec::new(), "missing", Some("Docker is not installed — containers are not shown.".into()));
    };
    if let Ok(g) = docker_retry_after().lock() {
        if g.is_some_and(|t| t > Instant::now()) {
            return (Vec::new(), "skipped", Some("Docker did not answer within 3 s — containers are not shown; it is asked again in 60 s.".into()));
        }
    }
    match run_tool(&docker, &["ps", "--format", "{{json .}}"], DOCKER_TIMEOUT).await {
        Some((stdout, _, 0)) => (parse_docker_ps(&stdout), "ok", None),
        Some(_) => (Vec::new(), "unavailable", Some("Docker is not running — containers are not shown.".into())),
        None => {
            if let Ok(mut g) = docker_retry_after().lock() {
                *g = Some(Instant::now() + DOCKER_BACKOFF);
            }
            (Vec::new(), "timeout", Some("Docker did not answer within 3 s — containers are not shown; it is asked again in 60 s.".into()))
        }
    }
}

#[cfg(unix)]
fn own_uid() -> u32 {
    // SAFETY: getuid has no preconditions.
    unsafe { libc::getuid() }
}

fn pid_list(pids: &[u32]) -> String {
    pids.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(",")
}

#[cfg(target_os = "macos")]
async fn gather(notes: &mut Vec<String>) -> Gathered {
    let mut g = Gathered { os: "macos".into(), own_uid: Some(own_uid()), own_pid: std::process::id(), shell_pids: crate::terminal::shell_pids(), ..Default::default() };
    let lsof = tool("lsof", &["/usr/sbin/lsof"]);
    let netstat = tool("netstat", &["/usr/sbin/netstat"]);
    let ps = tool("ps", &["/bin/ps"]).unwrap_or_else(|| PathBuf::from("/bin/ps"));
    // Own processes, with full names.
    if let Some(lsof) = &lsof {
        if let Some((out, _, _)) = run_tool(lsof, &["-nP", "-iTCP", "-sTCP:LISTEN", "-iUDP", "-F", "pcnPT"], LIST_TIMEOUT).await {
            g.socks = parse_lsof_sockets(&out);
        }
    } else {
        notes.push("lsof is not installed — only other users' processes could be listed.".into());
    }
    // Everyone else's, root's included.
    let own: HashSet<u32> = g.socks.iter().filter_map(|s| s.pid).collect();
    let mut netstat_rows = 0;
    if let Some(netstat) = &netstat {
        for (proto, flag) in [(Proto::Tcp, "tcp"), (Proto::Udp, "udp")] {
            if let Some((out, _, _)) = run_tool(netstat, &["-anv", "-p", flag], LIST_TIMEOUT).await {
                for s in parse_netstat_macos(&out, proto) {
                    if s.pid.is_some_and(|p| !own.contains(&p)) {
                        g.socks.push(s);
                        netstat_rows += 1;
                    }
                }
            }
        }
        if netstat_rows > 0 {
            notes.push("Processes of other users (including root) are listed from netstat; GitSwitch cannot inspect their folder or stop them.".into());
        }
    } else {
        notes.push("Only your own processes are listed — lsof shows no other user's without an administrator.".into());
    }
    let mut pids: Vec<u32> = g.socks.iter().filter_map(|s| s.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        return g;
    }
    if let Some((out, _, _)) = run_tool(&ps, &["-o", "pid=,uid=,user=,pgid=,lstart=,comm=", "-p", &pid_list(&pids)], LIST_TIMEOUT).await {
        g.procs = parse_ps_table(&out);
    }
    let mine: Vec<u32> = pids.iter().copied().filter(|p| g.procs.get(p).is_some_and(|i| Some(i.uid) == g.own_uid)).collect();
    if !mine.is_empty() {
        if let Some(lsof) = &lsof {
            if let Some((out, _, _)) = run_tool(lsof, &["-a", "-p", &pid_list(&mine), "-d", "cwd", "-Fn"], LIST_TIMEOUT).await {
                g.cwds = parse_lsof_cwd(&out);
            }
        }
        if let Some((out, _, _)) = run_tool(&ps, &["-Eww", "-o", "pid=,command=", "-p", &pid_list(&mine)], LIST_TIMEOUT).await {
            g.marks = parse_ps_env(&out);
        }
    }
    for (pid, info) in &g.procs {
        if info.comm.starts_with('/') {
            g.exes.insert(*pid, info.comm.clone());
        }
    }
    g
}

#[cfg(target_os = "linux")]
async fn gather(notes: &mut Vec<String>) -> Gathered {
    let mut g = Gathered { os: "linux".into(), own_uid: Some(own_uid()), own_pid: std::process::id(), shell_pids: crate::terminal::shell_pids(), ..Default::default() };
    let ss = tool("ss", &["/usr/bin/ss", "/usr/sbin/ss", "/bin/ss", "/sbin/ss"]);
    let lsof = tool("lsof", &["/usr/bin/lsof"]);
    if let Some(ss) = &ss {
        if let Some((out, _, _)) = run_tool(ss, &["-tulpnH"], LIST_TIMEOUT).await {
            g.socks = parse_ss(&out);
        }
    } else if let Some(lsof) = &lsof {
        if let Some((out, _, _)) = run_tool(lsof, &["-nP", "-iTCP", "-sTCP:LISTEN", "-iUDP", "-F", "pcnPT"], LIST_TIMEOUT).await {
            g.socks = parse_lsof_sockets(&out);
        }
    } else {
        notes.push("Neither ss nor lsof is installed — install iproute2 to list ports.".into());
    }
    if g.socks.iter().any(|s| s.pid.is_none()) {
        notes.push("Rows without a process belong to another user — ss shows no pid without root.".into());
    }
    let mut pids: Vec<u32> = g.socks.iter().filter_map(|s| s.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    if pids.is_empty() {
        return g;
    }
    let ps = tool("ps", &["/usr/bin/ps", "/bin/ps"]).unwrap_or_else(|| PathBuf::from("/usr/bin/ps"));
    if let Some((out, _, _)) = run_tool(&ps, &["-o", "pid=,uid=,user=,pgid=,lstart=,comm=", "-p", &pid_list(&pids)], LIST_TIMEOUT).await {
        g.procs = parse_ps_table(&out);
    }
    for pid in &pids {
        let proc_dir = PathBuf::from(format!("/proc/{}", pid));
        if let Ok(cwd) = std::fs::read_link(proc_dir.join("cwd")) {
            g.cwds.insert(*pid, cwd.to_string_lossy().trim_end_matches(" (deleted)").to_string());
        }
        if let Ok(exe) = std::fs::read_link(proc_dir.join("exe")) {
            g.exes.insert(*pid, exe.to_string_lossy().trim_end_matches(" (deleted)").to_string());
        }
        if let Ok(comm) = std::fs::read_to_string(proc_dir.join("comm")) {
            g.names.insert(*pid, comm.trim().to_string());
        }
        if let Ok(env) = std::fs::read(proc_dir.join("environ")) {
            let mut marks = EnvMarks::default();
            for w in env.split(|b| *b == 0) {
                let w = String::from_utf8_lossy(w);
                if w == "GITSWITCH_TERMINAL=1" {
                    marks.terminal = true;
                } else if let Some(v) = w.strip_prefix("GITSWITCH_WORKSPACE=") {
                    if !v.is_empty() {
                        marks.workspace = Some(v.to_string());
                    }
                }
            }
            g.marks.insert(*pid, marks);
        }
    }
    g
}

#[cfg(windows)]
async fn gather(notes: &mut Vec<String>) -> Gathered {
    let mut g = Gathered { os: "windows".into(), own_uid: None, own_pid: std::process::id(), shell_pids: crate::terminal::shell_pids(), ..Default::default() };
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let ps1 = tool("powershell", &[&format!("{}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe", root)]);
    let mut listed = false;
    if let Some(ps1) = &ps1 {
        if let Some((out, _, 0)) = run_tool(ps1, &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", POWERSHELL_SCRIPT], POWERSHELL_TIMEOUT).await {
            if let Some(rep) = parse_powershell(&out) {
                for (list, proto) in [(&rep.tcp, Proto::Tcp), (&rep.udp, Proto::Udp)] {
                    for s in list {
                        let (host, _) = split_host_port(&format!("{}:{}", s.a, s.p)).unwrap_or((s.a.clone(), s.p));
                        g.socks.push(RawSock { pid: Some(s.pid), name: None, proto, addr: host, port: s.p });
                    }
                }
                for p in rep.procs {
                    g.names.insert(p.pid, p.name.clone());
                    if !p.path.is_empty() {
                        g.exes.insert(p.pid, p.path.clone());
                    }
                    g.procs.insert(p.pid, ProcInfo { uid: 0, user: String::new(), pgid: p.pid, started_at: p.started, comm: p.path });
                }
                listed = true;
            }
        }
    }
    if !listed {
        let netstat = tool("netstat", &[&format!("{}\\System32\\netstat.exe", root)]);
        let tasklist = tool("tasklist", &[&format!("{}\\System32\\tasklist.exe", root)]);
        if let Some(netstat) = &netstat {
            if let Some((out, _, _)) = run_tool(netstat, &["-ano"], LIST_TIMEOUT).await {
                g.socks = parse_netstat_windows(&out);
            }
        }
        if let Some(tasklist) = &tasklist {
            if let Some((out, _, _)) = run_tool(tasklist, &["/FO", "CSV", "/NH"], LIST_TIMEOUT).await {
                g.names = parse_tasklist_csv(&out);
            }
        }
        if netstat.is_none() {
            notes.push("Neither PowerShell nor netstat answered — no ports could be listed.".into());
        }
    }
    notes.push("Folders are not readable on Windows, so rows cannot be attributed to a repository yet.".into());
    g
}

/// Every listener on this machine, with its holder, folder and container.
pub async fn list() -> Result<PortsReport, AppError> {
    let started = Instant::now();
    let mut notes = Vec::new();
    let (mut g, (containers, docker, docker_note)) = tokio::join!(gather(&mut notes), docker_containers());
    g.containers = containers;
    if let Some(n) = docker_note {
        notes.push(n);
    }
    let profs = crate::profiles::load_profiles().map(|s| s.profiles).unwrap_or_default();
    let rows = tokio::task::spawn_blocking(move || assemble(&g, &profs))
        .await
        .map_err(|e| AppError::Command(format!("Background task failed: {}", e)))?;
    Ok(PortsReport { os: g_os(), rows, notes, docker: docker.into(), took_ms: started.elapsed().as_millis() as u64 })
}

fn g_os() -> String {
    if cfg!(target_os = "macos") {
        "macos".into()
    } else if cfg!(windows) {
        "windows".into()
    } else {
        "linux".into()
    }
}

// ---------------------------------------------------------------------------
// Stopping things
// ---------------------------------------------------------------------------

/// Two spellings of one process, as the tools print them: equal ignoring
/// case and `.exe`, or one the start of the other — `python3` for a bundle
/// `ps` calls `Python`, `com.docker.backe` for a name netstat cut at 16, a
/// `node` for `node.exe`. The pid was re-read a moment ago, so the match
/// guards against a pid reused by an unrelated program, not a subtle one.
pub(crate) fn names_match(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        let t = s.trim();
        let t = t.strip_suffix(".exe").unwrap_or(t).to_ascii_lowercase();
        // `python3.12` → `python`, `node22` → `node`: the version is not the name.
        t.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.').to_string()
    };
    let (a, b) = (norm(a), norm(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

/// What `ps` (or tasklist) says a pid is right now — the kill trusts this,
/// never the row the page sent.
#[cfg(unix)]
async fn current_process(pid: u32) -> Result<ProcInfo, AppError> {
    let ps = tool("ps", &["/bin/ps", "/usr/bin/ps"]).unwrap_or_else(|| PathBuf::from("/bin/ps"));
    let out = run_tool(&ps, &["-o", "pid=,uid=,user=,pgid=,lstart=,comm=", "-p", &pid.to_string()], LIST_TIMEOUT).await;
    let table = out.map(|(o, _, _)| parse_ps_table(&o)).unwrap_or_default();
    table.get(&pid).cloned().ok_or_else(|| AppError::NotFound(format!("pid {} is no longer running", pid)))
}

#[cfg(windows)]
async fn current_process(pid: u32) -> Result<ProcInfo, AppError> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let tasklist = tool("tasklist", &[&format!("{}\\System32\\tasklist.exe", root)]).ok_or_else(|| AppError::Command("tasklist is not available".into()))?;
    let out = run_tool(&tasklist, &["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"], LIST_TIMEOUT).await;
    let names = out.map(|(o, _, _)| parse_tasklist_csv(&o)).unwrap_or_default();
    names.get(&pid).map(|n| ProcInfo { uid: 0, user: String::new(), pgid: pid, started_at: None, comm: n.clone() }).ok_or_else(|| AppError::NotFound(format!("pid {} is no longer running", pid)))
}

fn refusal(text: impl Into<String>) -> AppError {
    AppError::Command(format!("refused: {}", text.into()))
}

/// Stop a process: a polite request first, the hard way only on `force`.
/// `name` is what the page listed; a pid that now belongs to something else
/// is refused, so a stale list never ends the wrong program.
pub async fn kill(pid: u32, force: bool, name: &str) -> Result<KillOutcome, AppError> {
    if pid < 100 {
        return Err(refusal(format!("a system process (pid {}) — not stoppable from here", pid)));
    }
    if pid == std::process::id() {
        return Err(refusal("this is GitSwitch itself"));
    }
    if crate::terminal::shell_pids().contains(&pid) {
        return Err(refusal("the terminal drawer's own shell — close the drawer instead"));
    }
    let info = current_process(pid).await?;
    #[cfg(unix)]
    if info.uid != own_uid() {
        return Err(refusal(format!("owned by {} — needs an administrator", info.user)));
    }
    let shown = crate::paths::base_name(&info.comm);
    if !name.trim().is_empty() && !names_match(&shown, name) && !names_match(&info.comm, name) {
        return Err(refusal(format!("pid {} is now {}, not {} — the list is stale, refresh it", pid, shown, name)));
    }
    let label = if shown.is_empty() { name.to_string() } else { shown.clone() };
    let started = Instant::now();
    let outcome = kill_platform(pid, force, &info).await?;
    let argv: Vec<String> = match outcome.how.as_str() {
        "term" => vec!["kill".into(), "-TERM".into(), pid.to_string()],
        "kill" => vec!["kill".into(), "-KILL".into(), pid.to_string()],
        "taskkill" => vec!["taskkill".into(), "/PID".into(), pid.to_string(), "/T".into()],
        _ => vec!["taskkill".into(), "/PID".into(), pid.to_string(), "/T".into(), "/F".into()],
    };
    crate::cmdlog::record(Path::new("/"), argv, if outcome.ended { 0 } else { 1 }, started.elapsed(), "", &outcome.message);
    Ok(KillOutcome { message: outcome.message.replace("{name}", &label), ..outcome })
}

#[cfg(unix)]
async fn kill_platform(pid: u32, force: bool, info: &ProcInfo) -> Result<KillOutcome, AppError> {
    // SAFETY: plain libc calls on a pid the caller owns; no memory is shared.
    let (pgid, own_group) = unsafe { (libc::getpgid(pid as i32), libc::getpgrp()) };
    // The whole group only when the target leads it and it is not ours: a dev
    // server started as a shell job takes its children with it; a process
    // inside the drawer shell's own group is signalled alone.
    let group = pgid == pid as i32 && pgid != own_group;
    let target = if group { -(pid as i32) } else { pid as i32 };
    let send = |sig: i32| -> Result<bool, AppError> {
        // SAFETY: kill(2) with a pid/pgid this user owns.
        let r = unsafe { libc::kill(target, sig) };
        if r == 0 {
            return Ok(true);
        }
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::ESRCH) => Ok(false),
            Some(libc::EPERM) => Err(AppError::Command("the operating system refused (EPERM): another user's process".into())),
            _ => Err(AppError::Command(format!("could not signal pid {}: {}", pid, std::io::Error::last_os_error()))),
        }
    };
    let alive = |pid: u32| -> bool {
        // SAFETY: signal 0 checks existence only.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    };
    let wait_gone = |grace: Duration| async move {
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // A zombie has let go of its sockets; its parent just has not reaped it.
        let ps = tool("ps", &["/bin/ps", "/usr/bin/ps"]).unwrap_or_else(|| PathBuf::from("/bin/ps"));
        matches!(run_tool(&ps, &["-o", "stat=", "-p", &pid.to_string()], LIST_TIMEOUT).await, Some((s, _, _)) if s.trim().starts_with('Z'))
    };
    let _ = info;
    let mut how = "term";
    let mut ended = if send(libc::SIGTERM)? { wait_gone(KILL_GRACE).await } else { true };
    if !ended && force {
        how = "kill";
        ended = if send(libc::SIGKILL)? { wait_gone(KILL_HARD_GRACE).await } else { true };
    }
    let message = match (ended, how, group) {
        (true, "term", _) => format!("Stopped {{name}} (pid {}).", pid),
        (true, _, true) => format!("Force-stopped {{name}} (pid {}) and its process group.", pid),
        (true, _, false) => format!("Force-stopped {{name}} (pid {}).", pid),
        (false, _, _) => format!("{{name}} (pid {}) ignored the stop request for {} s — Force stop sends SIGKILL.", pid, KILL_GRACE.as_secs()),
    };
    Ok(KillOutcome { pid, ended, how: how.into(), group, message })
}

#[cfg(windows)]
async fn kill_platform(pid: u32, force: bool, _info: &ProcInfo) -> Result<KillOutcome, AppError> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
    let taskkill = tool("taskkill", &[&format!("{}\\System32\\taskkill.exe", root)]).ok_or_else(|| AppError::Command("taskkill is not available".into()))?;
    let pid_s = pid.to_string();
    let mut args = vec!["/PID", pid_s.as_str(), "/T"];
    if force {
        args.push("/F");
    }
    let Some((stdout, stderr, _)) = run_tool(&taskkill, &args, LIST_TIMEOUT).await else {
        return Err(AppError::Command("taskkill did not answer".into()));
    };
    if stderr.contains("Access is denied") {
        return Err(AppError::Command("Windows refused: this process belongs to another user or the system — stop it from an administrator terminal".into()));
    }
    let already_gone = stderr.contains("not found") || stdout.contains("not found");
    let mut ended = already_gone;
    if !ended {
        let deadline = Instant::now() + KILL_GRACE;
        while Instant::now() < deadline {
            if current_process(pid).await.is_err() {
                ended = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let how = if force { "taskkill-force" } else { "taskkill" };
    let message = if ended {
        format!("Stopped {{name}} (pid {}).", pid)
    } else {
        format!("{{name}} (pid {}) is still running after {} s — Force stop ends it with taskkill /F.", pid, KILL_GRACE.as_secs())
    };
    Ok(KillOutcome { pid, ended, how: how.into(), group: true, message })
}

fn valid_container_id(id: &str) -> bool {
    (12..=64).contains(&id.len()) && id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn valid_project(p: &str) -> bool {
    let mut chars = p.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit()) && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Stop one container, or every container of a Compose project (by label —
/// the compose plugin is not needed, and `up`-started projects behave the same).
pub async fn container_stop(id: &str, project: Option<&str>) -> Result<ContainerStopOutcome, AppError> {
    let docker = docker_tool().ok_or_else(|| AppError::Command("Docker is not installed.".into()))?;
    let ids: Vec<String> = match project {
        Some(p) => {
            if !valid_project(p) {
                return Err(AppError::Config(format!("`{}` is not a Compose project name.", p)));
            }
            let filter = format!("label=com.docker.compose.project={}", p);
            let Some((out, _, 0)) = run_tool(&docker, &["ps", "-q", "--filter", &filter], DOCKER_TIMEOUT).await else {
                return Err(AppError::Command("Docker did not answer.".into()));
            };
            out.lines().map(|l| l.trim().to_string()).filter(|l| valid_container_id(l)).collect()
        }
        None => {
            if !valid_container_id(id) {
                return Err(AppError::Config(format!("`{}` is not a container id.", id)));
            }
            vec![id.to_string()]
        }
    };
    if ids.is_empty() {
        return Ok(ContainerStopOutcome { stopped: Vec::new(), message: "Nothing of that project is running.".into() });
    }
    let mut args: Vec<&str> = vec!["stop", "-t", "10"];
    args.extend(ids.iter().map(String::as_str));
    let timeout = DOCKER_STOP_TIMEOUT + Duration::from_secs(2 * ids.len() as u64);
    match run_tool(&docker, &args, timeout).await {
        Some((_, _, 0)) => {}
        Some((_, err, _)) => return Err(AppError::Command(format!("Docker could not stop it: {}", err.trim()))),
        None => return Err(AppError::Command("Docker did not finish stopping it in time.".into())),
    }
    let message = match project {
        Some(p) => format!("Stopped {} container{} of project {}.", ids.len(), if ids.len() == 1 { "" } else { "s" }, p),
        None => format!("Stopped {}.", &ids[0][..12.min(ids[0].len())]),
    };
    Ok(ContainerStopOutcome { stopped: ids, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSOF: &str = "p647\ncControlCenter\nf9\nPTCP\nn*:7000\nTST=LISTEN\nTQR=0\nTQS=0\nf10\nPTCP\nn*:7000\nTST=LISTEN\nTQR=0\nTQS=0\nf11\nPTCP\nn*:5000\nTST=LISTEN\nTQR=0\nTQS=0\np694\ncidentityservicesd\nf7\nPUDP\nn*:*\np720\ncmongod\nf9\nPTCP\nn127.0.0.1:27017\nTST=LISTEN\nTQR=0\nTQS=0\nf10\nPTCP\nn[::1]:27017\nTST=LISTEN\nTQR=0\nTQS=0\np725\ncnode\nf18\nPTCP\nn127.0.0.1:18789\nTST=LISTEN\nTQR=0\nTQS=0\nf19\nPTCP\nn[::1]:18789\nTST=LISTEN\nTQR=0\nTQS=0\nf20\nPTCP\nn127.0.0.1:18791\nTST=LISTEN\nTQR=0\nTQS=0\nf22\nPUDP\nn*:5353\np752\ncreplicatord\nf8\nPUDP\nn*:59549\np889\ncCode Helper (Plugin)\nf54\nPTCP\nn127.0.0.1:57225\nTST=LISTEN\nTQR=0\nTQS=0\np2842\nccom.docker.backend\nf51\nPTCP\nn*:8080\nTST=LISTEN\nTQR=0\nTQS=0\nf54\nPTCP\nn127.0.0.1:5433\nTST=LISTEN\nTQR=0\nTQS=0\np5030\ncBrave Browser Helper\nf40\nPUDP\nn192.168.1.8:64598->160.79.104.10:443\nf74\nPUDP\nn*:5353\n";

    #[test]
    fn lsof_sockets_captured_on_macos() {
        let s = parse_lsof_sockets(LSOF);
        let find = |pid: u32, port: u16| s.iter().filter(|x| x.pid == Some(pid) && x.port == port).collect::<Vec<_>>();
        assert_eq!(find(647, 7000).len(), 2, "both binds of 7000, merged later");
        assert_eq!(find(647, 5000).len(), 1);
        assert!(s.iter().all(|x| x.pid != Some(694)), "an unbound UDP socket is not a listener");
        let m: Vec<&str> = find(720, 27017).iter().map(|x| x.addr.as_str()).collect();
        assert_eq!(m, vec!["127.0.0.1", "::1"]);
        assert_eq!(find(725, 18791).len(), 1);
        assert_eq!(find(725, 5353)[0].proto, Proto::Udp);
        assert_eq!(find(889, 57225)[0].name.as_deref(), Some("Code Helper (Plugin)"));
        assert_eq!(find(2842, 8080)[0].addr, "*");
        let brave: Vec<_> = s.iter().filter(|x| x.pid == Some(5030)).collect();
        assert_eq!(brave.len(), 1, "a connected UDP socket is skipped");
        assert_eq!(brave[0].port, 5353);
    }

    #[test]
    fn lsof_cwd_captured_on_macos() {
        let m = parse_lsof_cwd("p647\nfcwd\nn/\np720\nfcwd\nn/opt/homebrew\np725\nfcwd\nn/Users/macbookpro/.openclaw\n");
        assert_eq!(m.get(&725).map(String::as_str), Some("/Users/macbookpro/.openclaw"));
        assert_eq!(m.get(&647).map(String::as_str), Some("/"));
        assert_eq!(m.len(), 3);
    }

    const NETSTAT: &str = "Active Internet connections (including servers)
Proto Recv-Q Send-Q  Local Address                                 Foreign Address                               (state)          rxbytes      txbytes  rhiwat  shiwat          process:pid    state  options           gencnt    flags   flags1 usecnt rtncnt fltrs
tcp4       0      0  192.168.1.8.51429      20.207.73.85.443       ESTABLISHED        25440        29113  131072  131228      Code Helper:871    00102 00000008 0000000000030694 00000080 04000900      2      0 000000
tcp4       0      0  127.0.0.1.5433         *.*                    LISTEN                 0            0  131072  131072 com.docker.backe:2842   00100 00000006 000000000001bbd3 00000000 00000800      1      0 000000
tcp46      0      0  *.8080                 *.*                    LISTEN                 0            0  131072  131072 com.docker.backe:2842   00100 00000006 000000000001bbd2 00000000 00000800      1      0 000000
tcp6       0      0  ::1.18789              *.*                    LISTEN                 0            0  131072  131072             node:725    00100 00000106 00000000000033d1 00000001 00000800      1      0 000000
tcp4       0      0  127.0.0.1.57225        *.*                    LISTEN                 0            0  131072  131072 Code Helper (Plu:889    00100 00000106 0000000000001be7 00000001 00000800      1      0 000000
tcp4       0      0  127.0.0.1.8021         *.*                    LISTEN                 0            0  131072  131072          launchd:1      00180 00000006 0000000000000c62 00000000 00000800      1      0 000000
tcp6       0      0  ::1.8021               *.*                    LISTEN                 0            0  131072  131072          launchd:1      00180 00000006 0000000000000c61 00000000 00000800      1      0 000000
udp4       0      0  192.168.1.8.58100      13.126.138.201.443                        15814         8402 1048576   29040     Slack Helper:52815  00102 00000000 00000000000308b7 00000000 04200900      1      0 000002
udp46      0      0  *.5353                 *.*                                           0            0  786896    9216 Brave Browser He:5030   00100 00000204 0000000000004d54 00000000 00000800      1      0 000000
";

    #[test]
    fn netstat_captured_on_macos() {
        let tcp = parse_netstat_macos(NETSTAT, Proto::Tcp);
        assert_eq!(tcp.len(), 6, "ESTABLISHED is skipped; the rest are LISTEN");
        let launchd: Vec<_> = tcp.iter().filter(|s| s.pid == Some(1)).collect();
        assert_eq!(launchd.len(), 2);
        assert_eq!(launchd[0].name.as_deref(), Some("launchd"));
        assert_eq!(launchd[0].addr, "127.0.0.1");
        assert_eq!(launchd[1].addr, "::1");
        let plu = tcp.iter().find(|s| s.pid == Some(889)).unwrap();
        assert_eq!(plu.name.as_deref(), Some("Code Helper (Plu"), "a truncated name with spaces survives");
        assert_eq!(tcp.iter().find(|s| s.port == 8080).unwrap().addr, "*");
        let udp = parse_netstat_macos(NETSTAT, Proto::Udp);
        assert_eq!(udp.len(), 1, "a connected UDP client socket is skipped");
        assert_eq!(udp[0].pid, Some(5030));
        assert_eq!(udp[0].port, 5353);
    }

    #[test]
    fn ps_table_captured_on_macos() {
        let t = parse_ps_table("    1     0 root     1 Sun Oct 11 00:09:02 2026     /sbin/launchd\n  720   501 macbookpro   720 Sun Oct 11 00:13:40 2026     /opt/homebrew/opt/mongodb-community/bin/mongod\n  885   501 macbookpro   863 Sun Oct 11 00:13:47 2026     /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper\n");
        assert_eq!(t[&1].user, "root");
        assert_eq!(t[&1].uid, 0);
        assert_eq!(t[&720].pgid, 720);
        assert_eq!(t[&885].comm, "/Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper.app/Contents/MacOS/Code Helper");
        assert!(t[&885].started_at.as_deref().unwrap().starts_with("2026-10-11T00:13:47"));
    }

    #[test]
    fn ps_env_marks() {
        let m = parse_ps_env("725 /opt/homebrew/opt/node/bin/node /opt/homebrew/bin/openclaw gateway SHELL=/bin/zsh USER=macbookpro GITSWITCH_TERMINAL=1 GITSWITCH_WORKSPACE=abc12345 HOME=/Users/macbookpro\n647 /System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter\n");
        assert!(m[&725].terminal);
        assert_eq!(m[&725].workspace.as_deref(), Some("abc12345"));
        assert!(!m[&647].terminal);
        assert_eq!(m[&647].workspace, None);
    }

    // Constructed from ss(8)'s documented layout as printed on Ubuntu 22.04 — not captured.
    const SS: &str = "udp   UNCONN 0      0                                  0.0.0.0:5353       0.0.0.0:*    users:((\"avahi-daemon\",pid=812,fd=12))
udp   UNCONN 0      0                                     [::]:5353          [::]:*    users:((\"avahi-daemon\",pid=812,fd=14))
udp   UNCONN 0      0                            127.0.0.53%lo:53         0.0.0.0:*
tcp   LISTEN 0      4096                             127.0.0.1:27017      0.0.0.0:*    users:((\"mongod\",pid=1203,fd=11))
tcp   LISTEN 0      511                                      *:3000             *:*    users:((\"node\",pid=725,fd=18))
tcp   LISTEN 0      128                                0.0.0.0:22         0.0.0.0:*
tcp   LISTEN 0      4096                                  [::]:8080          [::]:*    users:((\"docker-proxy\",pid=2201,fd=4))
tcp   LISTEN 0      511                                  [::1]:631           [::]:*    users:((\"cupsd\",pid=640,fd=7))
tcp   LISTEN 0      4096                               0.0.0.0:80         0.0.0.0:*    users:((\"nginx\",pid=901,fd=6),(\"nginx\",pid=900,fd=6))
";

    #[test]
    fn ss_documented_layout() {
        let s = parse_ss(SS);
        assert_eq!(s.iter().filter(|x| x.port == 5353).count(), 2);
        assert_eq!(s.iter().find(|x| x.port == 53).unwrap().pid, None, "no users: → another user's socket");
        assert_eq!(s.iter().find(|x| x.port == 22).unwrap().pid, None);
        let nginx = s.iter().find(|x| x.port == 80).unwrap();
        assert_eq!(nginx.pid, Some(900), "the master is the smallest pid");
        assert_eq!(nginx.name.as_deref(), Some("nginx"));
        assert_eq!(s.iter().find(|x| x.port == 8080).unwrap().name.as_deref(), Some("docker-proxy"));
        assert_eq!(s.iter().find(|x| x.port == 3000).unwrap().addr, "*");
        assert_eq!(s.iter().find(|x| x.port == 631).unwrap().addr, "::1");
    }

    #[test]
    fn powershell_json_array_and_single_object() {
        let many = r#"{"tcp":[{"a":"0.0.0.0","p":135,"pid":1032},{"a":"::","p":135,"pid":1032},{"a":"127.0.0.1","p":3000,"pid":7256}],"udp":[{"a":"0.0.0.0","p":5353,"pid":2456}],"procs":[{"pid":4,"name":"System","path":"","started":null},{"pid":1032,"name":"svchost","path":"C:\\Windows\\System32\\svchost.exe","started":"2026-10-10T18:43:12.0000000Z"},{"pid":7256,"name":"node","path":"C:\\Program Files\\nodejs\\node.exe","started":"2026-10-11T07:12:30.1234567Z"}]}"#;
        let r = parse_powershell(many).unwrap();
        assert_eq!(r.tcp.len(), 3);
        assert_eq!(r.procs.iter().find(|p| p.pid == 4).unwrap().path, "");
        let one = r#"{"tcp":{"a":"127.0.0.1","p":3000,"pid":7256},"udp":{"a":"0.0.0.0","p":5353,"pid":2456},"procs":{"pid":7256,"name":"node","path":"C:\\node.exe","started":null}}"#;
        let r = parse_powershell(one).unwrap();
        assert_eq!(r.tcp.len(), 1);
        assert_eq!(r.udp.len(), 1);
        assert_eq!(r.procs.len(), 1);
        assert!(!POWERSHELL_SCRIPT.contains('"'), "the script travels as one argument, so it holds no double quote");
    }

    #[test]
    fn netstat_windows_and_tasklist_documented_layout() {
        let n = parse_netstat_windows("\nActive Connections\n\n  Proto  Local Address          Foreign Address        State           PID\n  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1032\n  TCP    127.0.0.1:3000         0.0.0.0:0              LISTENING       7256\n  TCP    127.0.0.1:49669        127.0.0.1:49670        ESTABLISHED     7256\n  TCP    [::]:135               [::]:0                 LISTENING       1032\n  UDP    0.0.0.0:5353           *:*                                    2456\n  UDP    [::1]:1900             *:*                                    3892\n");
        assert_eq!(n.len(), 5, "ESTABLISHED is skipped");
        assert_eq!(n.iter().filter(|s| s.port == 135).count(), 2);
        assert_eq!(n.iter().find(|s| s.port == 1900).unwrap().addr, "::1");
        let t = parse_tasklist_csv("\"svchost.exe\",\"1032\",\"Services\",\"0\",\"12,345 K\"\n\"node.exe\",\"7256\",\"Console\",\"1\",\"80,000 K\"\n");
        assert_eq!(t[&1032], "svchost");
        assert_eq!(t[&7256], "node");
    }

    const DOCKER: &str = r#"{"Command":"\"/docker-entrypoint.…\"","CreatedAt":"2026-09-25 05:44:29 +0530 IST","HealthStatus":"healthy","ID":"f50b929f94ba","Image":"carpool-proxy:local","Labels":"com.docker.compose.config-hash=15cee35bda2655230d5aeb5943c4d69536bc589bb39d4a0426b95d143cea1d0e,com.docker.compose.container-number=1,com.docker.compose.depends_on=frontend:service_healthy:false,backend:service_healthy:false,com.docker.compose.image=sha256:ddc1218a73de43f524067f50249d10d4bbe44740c1b494d8bbdeb57e1c78addc,com.docker.compose.oneoff=False,com.docker.compose.project.config_files=/Users/macbookpro/Documents/Ethara/CarPoolMain/docker-compose.yml,com.docker.compose.project.working_dir=/Users/macbookpro/Documents/Ethara/CarPoolMain,com.docker.compose.project=carpool,com.docker.compose.service=proxy,com.docker.compose.version=5.5.1,desktop.docker.io/ports.scheme=v2,desktop.docker.io/ports/80/tcp=:8080,maintainer=NGINX Docker Maintainers \u003cdocker-maint@nginx.com\u003e","LocalVolumes":"0","Mounts":"","Names":"carpool-proxy-1","Networks":"carpool_default","Platform":{"architecture":"arm64","os":"linux"},"Ports":"0.0.0.0:8080-\u003e80/tcp, [::]:8080-\u003e80/tcp","RunningFor":"2 weeks ago","Size":"81.9kB (virtual 54.2MB)","State":"running","Status":"Up 30 minutes (healthy)"}
{"Command":"\"docker-entrypoint.s…\"","CreatedAt":"2026-09-25 05:44:29 +0530 IST","HealthStatus":"healthy","ID":"88633eb1207c","Image":"postgres:15-alpine","Labels":"com.docker.compose.config-hash=fac6522858425b929a3c7572a96be484516ca2d345e61f9a15d2a2ad11d22b61,com.docker.compose.container-number=1,com.docker.compose.depends_on=,com.docker.compose.image=sha256:91a22c143801df42c2e4a8f9926eab5e48f39f40beb5220b7c29d4478263c341,com.docker.compose.oneoff=False,com.docker.compose.project.config_files=/Users/macbookpro/Documents/Ethara/CarPoolMain/docker-compose.yml,com.docker.compose.project.working_dir=/Users/macbookpro/Documents/Ethara/CarPoolMain,com.docker.compose.project=carpool,com.docker.compose.service=postgres,com.docker.compose.version=5.5.1,desktop.docker.io/ports.scheme=v2,desktop.docker.io/ports/5432/tcp=127.0.0.1:5433","LocalVolumes":"1","Mounts":"carpool_postgr…","Names":"carpool-postgres-1","Networks":"carpool_default","Platform":{"architecture":"arm64","os":"linux","variant":"v8"},"Ports":"127.0.0.1:5433-\u003e5432/tcp","RunningFor":"2 weeks ago","Size":"20.5kB (virtual 295MB)","State":"running","Status":"Up 30 minutes (healthy)"}
{"ID":"0000aaaa1111","Names":"carpool-frontend-1","Ports":"3000/tcp","Labels":"com.docker.compose.project=carpool,com.docker.compose.service=frontend"}
"#;

    #[test]
    fn docker_ps_captured() {
        let c = parse_docker_ps(DOCKER);
        assert_eq!(c.len(), 3);
        let proxy = &c[0];
        assert_eq!(proxy.name, "carpool-proxy-1");
        assert_eq!(proxy.project.as_deref(), Some("carpool"));
        assert_eq!(proxy.service.as_deref(), Some("proxy"));
        assert_eq!(proxy.working_dir.as_deref(), Some("/Users/macbookpro/Documents/Ethara/CarPoolMain"));
        assert_eq!(proxy.published.len(), 2);
        assert_eq!(proxy.published[0].host_port, 8080);
        assert_eq!(proxy.published[0].container_port, 80);
        assert_eq!(proxy.published[0].host_ip, "*");
        let labels = parse_docker_labels("a=1,com.docker.compose.depends_on=frontend:service_healthy:false,backend:service_healthy:false,com.docker.compose.project.config_files=/x/a.yml,/x/b.yml,z=9");
        assert_eq!(labels["com.docker.compose.depends_on"], "frontend:service_healthy:false,backend:service_healthy:false");
        assert_eq!(labels["com.docker.compose.project.config_files"], "/x/a.yml,/x/b.yml");
        assert_eq!(labels["z"], "9");
        let pg = &c[1];
        assert_eq!(pg.published[0].host_ip, "127.0.0.1");
        assert_eq!(pg.published[0].host_port, 5433);
        assert_eq!(pg.published[0].container_port, 5432);
        assert!(c[2].published.is_empty(), "an unpublished port is not a listener");
    }

    #[test]
    fn docker_port_ranges() {
        let p = parse_docker_ports("0.0.0.0:3000-3002->3000-3002/tcp, 127.0.0.1:9000->9000/udp");
        assert_eq!(p.len(), 4);
        assert_eq!((p[1].host_port, p[1].container_port), (3001, 3001));
        assert_eq!(p[3].proto, Proto::Udp);
    }

    fn gathered() -> Gathered {
        let mut g = Gathered { os: "macos".into(), own_uid: Some(501), own_pid: 99999, shell_pids: vec![4242], ..Default::default() };
        g.socks = parse_lsof_sockets(LSOF);
        g.socks.extend(parse_netstat_macos(NETSTAT, Proto::Tcp).into_iter().filter(|s| s.pid == Some(1)));
        g.procs = parse_ps_table("    1     0 root     1 Sun Oct 11 00:09:02 2026     /sbin/launchd\n  647   501 macbookpro   647 Sun Oct 11 00:09:02 2026     /System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter\n  720   501 macbookpro   720 Sun Oct 11 00:13:40 2026     /opt/homebrew/opt/mongodb-community/bin/mongod\n  725   501 macbookpro   725 Sun Oct 11 00:13:40 2026     /opt/homebrew/opt/node/bin/node\n  889   501 macbookpro   863 Sun Oct 11 00:13:47 2026     /Applications/Visual Studio Code.app/Contents/Frameworks/Code Helper (Plugin).app/Contents/MacOS/Code Helper (Plugin)\n 2842   501 macbookpro  2791 Sun Oct 11 00:10:00 2026     /Applications/Docker.app/Contents/MacOS/com.docker.backend\n");
        for (pid, info) in &g.procs {
            g.exes.insert(*pid, info.comm.clone());
        }
        g.cwds = parse_lsof_cwd("p725\nfcwd\nn/Users/macbookpro/.openclaw\n");
        g.marks = parse_ps_env("725 node GITSWITCH_TERMINAL=1 GITSWITCH_WORKSPACE=abc12345\n");
        g.containers = parse_docker_ps(DOCKER);
        g
    }

    #[test]
    fn rows_merge_binds_and_decide_who_can_be_stopped() {
        let g = gathered();
        let rows = assemble(&g, &[]);
        let row = |port: u16| rows.iter().find(|r| r.port == port && r.proto == "tcp").unwrap();
        assert_eq!(row(27017).addresses, vec!["127.0.0.1", "::1"], "IPv4 and IPv6 binds are one row");
        assert_eq!(row(27017).scope, "loopback");
        assert_eq!(row(7000).scope, "all");
        assert_eq!(rows.iter().filter(|r| r.port == 7000).count(), 1);
        let node = row(18789);
        assert!(node.killable && node.ours);
        assert_eq!(node.workspace.as_deref(), Some("abc12345"));
        assert_eq!(node.cwd.as_deref(), Some("/Users/macbookpro/.openclaw"));
        assert!(node.started_at.is_some());
        let launchd = row(8021);
        assert_eq!(launchd.why_not.as_deref(), Some("A system process (pid 1) — not stoppable from here."));
        let cc = row(5000);
        assert_eq!(cc.why_not.as_deref(), Some("Part of macOS — not stoppable from here."));
        let docker = row(8080);
        assert_eq!(docker.container.as_ref().map(|c| c.name.as_str()), Some("carpool-proxy-1"));
        // Attribution follows the container's project folder, not Docker's own cwd.
        let dir = std::env::temp_dir().join(format!("gitswitch-ports-compose-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proj").join("carpool").join(".git")).unwrap();
        let prof = crate::profiles::Profile {
            id: "p".into(), name: "Work".into(), git_name: "n".into(), git_email: "e".into(), ssh_key_path: None, is_default: false, allow_push: true, signing_enabled: false,
            directories: vec![crate::paths::norm(&dir.join("proj").to_string_lossy())], remote_patterns: vec![], github_login: None, push_over_ssh: None, created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let mut g2 = gathered();
        g2.cwds.insert(2842, "/Users/macbookpro/Library/Containers/com.docker.docker/Data".into());
        for c in g2.containers.iter_mut() {
            c.working_dir = Some(crate::paths::norm(&dir.join("proj").join("carpool").to_string_lossy()));
        }
        let rows2 = assemble(&g2, std::slice::from_ref(&prof));
        let d2 = rows2.iter().find(|r| r.port == 8080 && r.proto == "tcp").unwrap();
        assert_eq!(d2.clone.as_ref().map(|c| c.name.as_str()), Some("carpool"), "the Compose project's folder names the clone");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(docker.why_not.as_deref(), Some("Published by Docker — stop the container instead."));
        assert_eq!(docker.container.as_ref().and_then(|c| c.container_port), Some(80));
    }

    #[test]
    fn killable_matrix_texts() {
        let mut g = gathered();
        let base = |pid: Option<u32>, name: &str, exe: Option<&str>| Listener {
            key: "k".into(), proto: "tcp".into(), port: 1, addresses: vec![], scope: "all".into(), pid, name: Some(name.into()), exe: exe.map(String::from), cwd: None, started_at: None, user: None, ours: false, workspace: None, clone: None, container: None, killable: false, why_not: None,
        };
        assert_eq!(why_not_killable(&base(None, "x", None), &g).unwrap(), "Another user's process — the system shows no process id without an administrator.");
        assert_eq!(why_not_killable(&base(Some(99999), "x", None), &g).unwrap(), "This is GitSwitch itself.");
        assert_eq!(why_not_killable(&base(Some(4242), "zsh", None), &g).unwrap(), "The terminal drawer's own shell — close the drawer instead.");
        assert_eq!(why_not_killable(&base(Some(1), "launchd", None), &g).unwrap(), "A system process (pid 1) — not stoppable from here.");
        g.procs.insert(7777, ProcInfo { uid: 0, user: "root".into(), pgid: 7777, started_at: None, comm: "/usr/local/bin/x".into() });
        assert_eq!(why_not_killable(&base(Some(7777), "x", None), &g).unwrap(), "Owned by root — needs an administrator: stop it from Terminal with sudo kill 7777.");
        g.containers.clear();
        assert!(why_not_killable(&base(Some(2842), "com.docker.backend", Some("/Applications/Docker.app/Contents/MacOS/com.docker.backend")), &g).unwrap().starts_with("Docker Desktop's own process"));
        assert_eq!(why_not_killable(&base(Some(725), "node", Some("/opt/homebrew/bin/node")), &g), None);
        g.os = "windows".into();
        g.own_uid = None;
        assert_eq!(why_not_killable(&base(Some(1032), "svchost", Some("C:\\Other\\svchost.exe")), &g).unwrap(), "Part of Windows — not stoppable from here.");
        assert!(why_not_killable(&base(Some(5555), "mystery", Some("")), &g).unwrap().contains("access denied"));
        assert_eq!(why_not_killable(&base(Some(7256), "node", Some("C:\\Program Files\\nodejs\\node.exe")), &g), None);
    }

    #[test]
    fn ids_projects_and_names() {
        assert!(valid_container_id("f50b929f94ba"));
        assert!(!valid_container_id("F50B929F94BA"));
        assert!(!valid_container_id("f50b929f94ba; rm -rf /"));
        assert!(valid_project("carpool"));
        assert!(valid_project("argos_08-10-26-2f9c1a7b"));
        assert!(!valid_project("-x"));
        assert!(!valid_project("Argos"));
        assert!(names_match("node", "node"));
        assert!(names_match("node.exe", "Node"));
        assert!(names_match("com.docker.backe", "com.docker.backend"), "a 16-char truncation matches");
        assert!(names_match("Python", "python3"), "macOS calls the bundle Python, lsof says python3");
        assert!(names_match("python3", "python3.12"));
        assert!(!names_match("node", "mongod"));
        assert!(!names_match("", "node"));
    }

    #[test]
    fn clone_of_finds_the_deepest_repo_in_a_profile_folder() {
        let dir = std::env::temp_dir().join(format!("gitswitch-ports-clone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let outer = dir.join("profiles").join("outer");
        let inner = outer.join("inner");
        std::fs::create_dir_all(inner.join("src")).unwrap();
        std::fs::create_dir_all(outer.join(".git")).unwrap();
        std::fs::write(inner.join(".git"), "gitdir: ../.git/modules/inner").unwrap();
        let prof = crate::profiles::Profile {
            id: "p".into(), name: "Work".into(), git_name: "n".into(), git_email: "e".into(), ssh_key_path: None, is_default: false, allow_push: true, signing_enabled: false,
            directories: vec![crate::paths::norm(&dir.join("profiles").to_string_lossy())], remote_patterns: vec![], github_login: None, push_over_ssh: None, created_at: chrono::Utc::now(), updated_at: chrono::Utc::now(),
        };
        let c = clone_of(&inner.join("src").to_string_lossy(), std::slice::from_ref(&prof)).unwrap();
        assert_eq!(c.name, "inner", "the nearest .git — a file for a submodule — wins");
        assert_eq!(c.profile_name, "Work");
        assert!(clone_of(&std::env::temp_dir().to_string_lossy(), std::slice::from_ref(&prof)).is_none(), "outside every profile folder: no clone");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
