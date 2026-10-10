//! Builds the sidecars — the lock helper and the terminal's git guard — and
//! places them where Tauri's bundler, and its build script (which refuses to
//! compile the app without them), expect them:
//! `src-tauri/binaries/<name>-<target-triple>[.exe]`.
//!
//! The same program on every OS and from every shell (cmd, PowerShell, sh, the
//! Docker builder): Tauri's before-commands run it with cargo, so a Windows
//! machine needs no bash. Usage: `cargo run -p gitswitch-sidecars -- [release|debug]`.
//! Tauri sets `TAURI_ENV_TARGET_TRIPLE` for before-commands;
//! `universal-apple-darwin` is built for both Apple architectures and joined
//! with lipo. A cross-build that needs another linker (the Windows installer
//! from a Linux or macOS Docker host, through cargo-xwin) names the cargo to
//! build with in `GITSWITCH_CARGO_RUNNER`: `cargo run` itself always sets
//! `CARGO` to the real toolchain binary, so that cannot carry it.

use std::path::{Path, PathBuf};
use std::process::Command;

const SIDECARS: [&str; 2] = ["gitswitch-lock-helper", "gitswitch-git-shim"];

fn main() {
    if let Err(e) = run() {
        eprintln!("sidecars: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let profile = std::env::args().nth(1).unwrap_or_else(|| "release".into());
    if profile != "release" && profile != "debug" {
        return Err(format!("unknown profile `{}` (release or debug)", profile));
    }
    // Where the checkout is now: `cargo run` says so at run time. The path
    // compiled in stays the old one when a checkout is moved, as cargo does
    // not rebuild for that.
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let src = manifest.parent().ok_or("this crate is not inside src-tauri")?.to_path_buf();
    let cargo = std::env::var("GITSWITCH_CARGO_RUNNER")
        .ok()
        .filter(|c| !c.trim().is_empty())
        .or_else(|| std::env::var("CARGO").ok())
        .unwrap_or_else(|| "cargo".into());
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let host = host_triple(&rustc)?;
    let triple = std::env::var("TAURI_ENV_TARGET_TRIPLE").ok().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| host.clone());
    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let bins = src.join("binaries");
    std::fs::create_dir_all(&bins).map_err(|e| format!("{}: {}", bins.display(), e))?;
    for pkg in SIDECARS {
        let out = bins.join(format!("{}-{}{}", pkg, triple, ext));
        if triple == "universal-apple-darwin" {
            let a = build(&cargo, &src, pkg, "aarch64-apple-darwin", &host, &profile, ext)?;
            let x = build(&cargo, &src, pkg, "x86_64-apple-darwin", &host, &profile, ext)?;
            let st = Command::new("lipo").arg("-create").arg(&a).arg(&x).arg("-output").arg(&out).status().map_err(|e| format!("lipo: {}", e))?;
            if !st.success() {
                return Err(format!("lipo could not join {} and {}", a.display(), x.display()));
            }
        } else {
            let built = build(&cargo, &src, pkg, &triple, &host, &profile, ext)?;
            std::fs::copy(&built, &out).map_err(|e| format!("copying {} to {}: {}", built.display(), out.display(), e))?;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {}", out.display(), e))?;
        }
        println!("sidecar: {}", out.display());
    }
    Ok(())
}

/// The triple rustc builds for when no target is named.
fn host_triple(rustc: &str) -> Result<String, String> {
    let out = Command::new(rustc).arg("-vV").output().map_err(|e| format!("{} -vV: {}", rustc, e))?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("host: ").map(|h| h.trim().to_string()))
        .ok_or_else(|| format!("{} -vV named no host", rustc))
}

/// `cargo build -p <pkg>` for one triple; the path of what it built.
fn build(cargo: &str, src: &Path, pkg: &str, triple: &str, host: &str, profile: &str, ext: &str) -> Result<PathBuf, String> {
    let mut cmd = Command::new(cargo);
    cmd.current_dir(src).args(["build", "-p", pkg]);
    if profile == "release" {
        cmd.arg("--release");
    }
    let dir = if triple == host {
        src.join("target").join(profile)
    } else {
        cmd.args(["--target", triple]);
        src.join("target").join(triple).join(profile)
    };
    let st = cmd.status().map_err(|e| format!("{}: {}", cargo, e))?;
    if !st.success() {
        return Err(format!("cargo build -p {} failed", pkg));
    }
    Ok(dir.join(format!("{}{}", pkg, ext)))
}
