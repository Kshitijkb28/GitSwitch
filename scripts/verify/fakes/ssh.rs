// The fake ssh of sandbox.sh's "Who would push" section, as a program Windows
// can start (it starts programs by their .exe and cannot run the suite's shell
// script as ssh). Compiled there by the suite with `rustc`; it behaves exactly
// like the script:
//
// - `-G` and `-F` go to the real ssh (FAKE_REAL_SSH), so alias resolution is
//   the real thing;
// - otherwise it greets as github.com does, naming the key it was handed with
//   -i, else the IdentityFile of a Host block that says IdentitiesOnly, else
//   "port-<n>" for an explicit port, else "agent-user"; and exits 1 like ssh -T.
use std::process::{exit, Command};

fn main() {
    // The real ssh is named by an absolute path, never looked up as "ssh":
    // Windows looks in this program's own folder first, where "ssh" is this
    // fake, which would then start itself without end. FAKE_SSH_INSIDE is set
    // for the real ssh's run, so a misnamed one stops here instead.
    if std::env::var_os("FAKE_SSH_INSIDE").is_some() {
        eprintln!("fake ssh: FAKE_REAL_SSH names this fake, not the real ssh");
        exit(2);
    }
    let Some(real) = std::env::var_os("FAKE_REAL_SSH").filter(|p| std::path::Path::new(p).is_absolute()) else {
        eprintln!("fake ssh: FAKE_REAL_SSH must name the real ssh by an absolute path");
        exit(2);
    };
    let real_ssh = |args: &[&str]| {
        let mut cmd = Command::new(&real);
        cmd.args(args).env("FAKE_SSH_INSIDE", "1");
        cmd
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-G") | Some("-F")) {
        let all: Vec<&str> = args.iter().map(String::as_str).collect();
        let code = real_ssh(&all).status().ok().and_then(|s| s.code()).unwrap_or(1);
        exit(code);
    }
    let (mut key, mut port, mut prev, mut dest) = (String::new(), String::new(), String::new(), String::new());
    for a in &args {
        if prev == "-i" {
            key = a.clone();
        }
        if prev == "-p" {
            port = a.clone();
        }
        prev = a.clone();
        dest = a.clone();
    }
    let host = dest.split_once('@').map(|(_, h)| h.to_string()).unwrap_or_else(|| dest.clone());
    if key.is_empty() {
        let config = std::env::var("GITSWITCH_SSH_CONFIG").unwrap_or_default();
        if let Ok(out) = real_ssh(&["-F", &config, "-G", &host]).output() {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.lines().any(|l| l.starts_with("identitiesonly yes")) {
                if let Some(line) = text.lines().find(|l| l.starts_with("identityfile ")) {
                    key = line.split_whitespace().nth(1).unwrap_or_default().to_string();
                }
            }
        }
    }
    let who = if !key.is_empty() {
        std::path::Path::new(&key).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or(key)
    } else if !port.is_empty() {
        format!("port-{}", port)
    } else {
        "agent-user".to_string()
    };
    eprintln!("Hi {}! You've successfully authenticated, but GitHub does not provide shell access.", who);
    exit(1);
}
