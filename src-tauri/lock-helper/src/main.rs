//! GitSwitch's lock helper: the only GitSwitch code that ever runs with
//! administrator rights, and — under the name `git-remote-gitswitch-push-blocked`
//! — the remote helper that explains a refused push.
//!
//! Rules it never breaks: it never spawns a shell, never runs git while
//! elevated, never writes to a path a job supplied, never follows a symlink on
//! a managed path, never echoes file contents into an error, and exits before
//! touching the filesystem unless it is elevated and the command line is
//! exactly one of the shapes it knows.

use gitswitch_lock_core as lc;
use lc::{exit, Change, Job, JobResult, Layout, Op, Platform, Registry};
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_BACKUPS: usize = 5;

/// A failure with the exit code it maps to. Messages name paths and error
/// kinds, never file contents.
#[derive(Debug)]
struct Fail {
    code: i32,
    msg: String,
}

impl Fail {
    fn new(code: i32, msg: impl Into<String>) -> Fail {
        Fail { code, msg: msg.into() }
    }
    fn usage(msg: impl Into<String>) -> Fail {
        Fail::new(exit::USAGE, msg)
    }
    fn refused(msg: impl Into<String>) -> Fail {
        Fail::new(exit::REFUSED, msg)
    }
    fn io(what: &str, e: std::io::Error) -> Fail {
        Fail::new(exit::IO, format!("{}: {}", what, e))
    }
}

type R<T> = Result<T, Fail>;

fn now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    lc::rfc3339_utc(secs)
}

/// Debug builds only: prefix every managed path so the verify suites can run
/// the helper unprivileged. Refused outright when actually running as root,
/// so a release-like environment can never be redirected.
fn test_root() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("GITSWITCH_LOCK_ROOT").map(PathBuf::from)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

// ---------------------------------------------------------------------------
// Platform primitives
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod sys {
    use std::fs::{File, Metadata, OpenOptions};
    use std::io;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    pub fn is_root() -> bool {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() == 0 }
    }
    pub fn is_elevated() -> bool {
        is_root()
    }
    pub fn current_uid() -> u32 {
        // SAFETY: as above.
        unsafe { libc::geteuid() }
    }
    pub fn open_read_nofollow(p: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(p)
    }
    pub fn create_new_nofollow(p: &Path, mode: u32) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(p)
    }
    pub fn open_append_nofollow(p: &Path, mode: u32) -> io::Result<File> {
        OpenOptions::new()
            .append(true)
            .create(true)
            .mode(mode)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(p)
    }
    pub fn owner_uid(m: &Metadata) -> u32 {
        m.uid()
    }
    pub fn owner_gid(m: &Metadata) -> u32 {
        m.gid()
    }
    pub fn admin_owned(_p: &Path, m: &Metadata) -> bool {
        m.uid() == 0
    }
    pub fn owned_by_current_user(_p: &Path, m: &Metadata) -> bool {
        m.uid() == current_uid()
    }
    /// Group- or world-writable?
    pub fn loosely_writable(_p: &Path, m: &Metadata) -> bool {
        m.permissions().mode() & 0o022 != 0
    }
    pub fn chown(f: &File, uid: u32, gid: u32) -> io::Result<()> {
        use std::os::unix::io::AsRawFd;
        // SAFETY: fchown on an open descriptor we own.
        if unsafe { libc::fchown(f.as_raw_fd(), uid, gid) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    /// Make `p` the administrator's: root-owned (the mode is set by the caller).
    pub fn lock_down(p: &Path) -> io::Result<()> {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(p.as_os_str().as_bytes()).map_err(|_| io::Error::other("path has a NUL byte"))?;
        // SAFETY: lchown with a valid NUL-terminated path.
        if unsafe { libc::lchown(c.as_ptr(), 0, 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    pub fn set_mode(f: &File, mode: u32) -> io::Result<()> {
        f.set_permissions(std::fs::Permissions::from_mode(mode))
    }
    pub fn set_dir_mode(p: &Path, mode: u32) -> io::Result<()> {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
    }
    pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }
    /// Replace this process with the installed helper; only returns on failure.
    pub fn exec(program: &Path, args: &[std::ffi::OsString]) -> io::Error {
        use std::os::unix::process::CommandExt;
        std::process::Command::new(program).args(args).exec()
    }
    pub fn windows_install_path() -> Option<String> {
        None
    }
    pub fn remove_self_binary(p: &Path) -> io::Result<()> {
        std::fs::remove_file(p)
    }
    pub fn is_reparse_point(_m: &Metadata) -> bool {
        false
    }
}

#[cfg(windows)]
mod sys {
    //! Windows: "administrator-owned" means owned by BUILTIN\Administrators or
    //! SYSTEM with a DACL that grants no write to Users, Everyone,
    //! Authenticated Users or INTERACTIVE; `%ProgramData%` alone lets Users
    //! create files, so every file the helper writes gets an explicit,
    //! non-inherited DACL. Elevation is the token's TokenElevation flag.
    use std::ffi::{c_void, OsStr};
    use std::fs::{File, Metadata, OpenOptions};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, HLOCAL};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W,
        NO_MULTIPLE_TRUSTEE, SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_GROUP, TRUSTEE_IS_SID, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        CreateWellKnownSid, GetAce, GetTokenInformation, IsWellKnownSid, TokenElevation, WinAuthenticatedUserSid,
        WinBuiltinAdministratorsSid, WinBuiltinUsersSid, WinInteractiveSid, WinLocalSystemSid, WinWorldSid, ACCESS_ALLOWED_ACE,
        ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, NO_INHERITANCE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_MAX_SID_SIZE, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_EXECUTE,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, MOVEFILE_DELAY_UNTIL_REBOOT, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6432KEY, RRF_SUBKEY_WOW6464KEY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    pub fn is_elevated() -> bool {
        // SAFETY: standard token query on our own process; handles are closed.
        unsafe {
            let mut tok: HANDLE = null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) == 0 {
                return false;
            }
            let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut n = 0u32;
            let ok = GetTokenInformation(
                tok,
                TokenElevation,
                &mut e as *mut TOKEN_ELEVATION as *mut c_void,
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut n,
            ) != 0;
            CloseHandle(tok);
            ok && e.TokenIsElevated != 0
        }
    }
    pub fn is_root() -> bool {
        is_elevated()
    }
    fn refuse_reparse(f: File) -> io::Result<File> {
        if f.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other("is a reparse point (symlink or junction)"));
        }
        Ok(f)
    }
    pub fn open_read_nofollow(p: &Path) -> io::Result<File> {
        refuse_reparse(OpenOptions::new().read(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(p)?)
    }
    pub fn create_new_nofollow(p: &Path, _mode: u32) -> io::Result<File> {
        OpenOptions::new().write(true).create_new(true).open(p)
    }
    pub fn open_append_nofollow(p: &Path, _mode: u32) -> io::Result<File> {
        refuse_reparse(OpenOptions::new().append(true).create(true).custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(p)?)
    }
    pub fn owner_uid(_m: &Metadata) -> u32 {
        0
    }
    pub fn owner_gid(_m: &Metadata) -> u32 {
        0
    }

    struct Security {
        owner: PSID,
        dacl: *mut ACL,
        sd: PSECURITY_DESCRIPTOR,
    }
    impl Drop for Security {
        fn drop(&mut self) {
            // SAFETY: sd came from GetNamedSecurityInfoW and is freed once.
            unsafe {
                LocalFree(self.sd as HLOCAL);
            }
        }
    }
    fn security(p: &Path) -> Option<Security> {
        let w = wide(p.as_os_str());
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: documented call; out-pointers are valid for the call.
        let rc = unsafe {
            GetNamedSecurityInfoW(
                w.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut sd,
            )
        };
        if rc != ERROR_SUCCESS || sd.is_null() {
            return None;
        }
        Some(Security { owner, dacl, sd })
    }
    /// Owned by BUILTIN\Administrators, SYSTEM, or the TrustedInstaller
    /// service (which owns `C:\Program Files` and everything Windows Installer
    /// puts there — Git for Windows' exec path included).
    pub fn admin_owned(p: &Path, _m: &Metadata) -> bool {
        const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        let Some(sec) = security(p) else { return false };
        if sec.owner.is_null() {
            return false;
        }
        // SAFETY: owner points into sd, alive while `sec` is; the string SID is
        // LocalAlloc'd by the system and freed here.
        unsafe {
            if IsWellKnownSid(sec.owner, WinBuiltinAdministratorsSid) != 0 || IsWellKnownSid(sec.owner, WinLocalSystemSid) != 0 {
                return true;
            }
            let mut s: *mut u16 = null_mut();
            if ConvertSidToStringSidW(sec.owner, &mut s) == 0 || s.is_null() {
                return false;
            }
            let mut len = 0usize;
            while *s.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
            LocalFree(s as HLOCAL);
            text.eq_ignore_ascii_case(TRUSTED_INSTALLER)
        }
    }
    pub fn owned_by_current_user(_p: &Path, _m: &Metadata) -> bool {
        true
    }
    /// Does any allow-ACE grant a write right to a low-privilege group?
    pub fn loosely_writable(p: &Path, _m: &Metadata) -> bool {
        let Some(sec) = security(p) else { return true };
        if sec.dacl.is_null() {
            return true; // a NULL DACL grants everything to everyone
        }
        let write = FILE_GENERIC_WRITE | GENERIC_WRITE | DELETE | WRITE_DAC | WRITE_OWNER;
        // SAFETY: dacl points into sd, alive while `sec` is; GetAce bounds-checks.
        unsafe {
            let count = (*sec.dacl).AceCount as u32;
            for i in 0..count {
                let mut ace: *mut c_void = null_mut();
                if GetAce(sec.dacl, i, &mut ace) == 0 || ace.is_null() {
                    continue;
                }
                let hdr = &*(ace as *const ACE_HEADER);
                if hdr.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE as u32 {
                    continue;
                }
                let a = &*(ace as *const ACCESS_ALLOWED_ACE);
                let sid = &a.SidStart as *const u32 as PSID;
                let low = [WinWorldSid, WinBuiltinUsersSid, WinAuthenticatedUserSid, WinInteractiveSid]
                    .iter()
                    .any(|k| IsWellKnownSid(sid, *k) != 0);
                if low && a.Mask & write != 0 {
                    return true;
                }
            }
        }
        false
    }
    pub fn chown(_f: &File, _uid: u32, _gid: u32) -> io::Result<()> {
        Ok(())
    }
    /// Owner Administrators; DACL (not inherited): SYSTEM and Administrators
    /// full control, Users read and execute.
    pub fn lock_down(p: &Path) -> io::Result<()> {
        fn sid(kind: i32) -> io::Result<Vec<u8>> {
            let mut buf = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
            let mut n = buf.len() as u32;
            // SAFETY: buffer sized to SECURITY_MAX_SID_SIZE.
            if unsafe { CreateWellKnownSid(kind, null_mut(), buf.as_mut_ptr() as PSID, &mut n) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(buf)
        }
        let mut admins = sid(WinBuiltinAdministratorsSid)?;
        let mut system = sid(WinLocalSystemSid)?;
        let mut users = sid(WinBuiltinUsersSid)?;
        let inherit = if p.is_dir() { SUB_CONTAINERS_AND_OBJECTS_INHERIT } else { NO_INHERITANCE };
        let entry = |s: &mut Vec<u8>, mask: u32| EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: SET_ACCESS,
            grfInheritance: inherit,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_GROUP,
                ptstrName: s.as_mut_ptr() as *mut u16,
            },
        };
        let entries = [
            entry(&mut system, FILE_ALL_ACCESS),
            entry(&mut admins, FILE_ALL_ACCESS),
            entry(&mut users, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE),
        ];
        let mut acl: *mut ACL = null_mut();
        // SAFETY: entries and out-pointer valid for the call; acl freed below.
        let rc = unsafe { SetEntriesInAclW(entries.len() as u32, entries.as_ptr(), null_mut(), &mut acl) };
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        let w = wide(p.as_os_str());
        // SAFETY: documented call with valid pointers; acl freed afterwards.
        let rc = unsafe {
            SetNamedSecurityInfoW(
                w.as_ptr() as *mut u16,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                admins.as_mut_ptr() as PSID,
                null_mut(),
                acl,
                null_mut(),
            )
        };
        // SAFETY: acl came from SetEntriesInAclW.
        unsafe {
            LocalFree(acl as HLOCAL);
        }
        if rc != ERROR_SUCCESS {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }
    pub fn set_mode(_f: &File, _mode: u32) -> io::Result<()> {
        Ok(())
    }
    pub fn set_dir_mode(_p: &Path, _mode: u32) -> io::Result<()> {
        Ok(())
    }
    /// No symlink on Windows: git's exec path wants a real .exe, so copy.
    pub fn symlink(target: &Path, link: &Path) -> io::Result<()> {
        std::fs::copy(target, link).map(|_| ())
    }
    pub fn exec(program: &Path, args: &[std::ffi::OsString]) -> io::Error {
        match std::process::Command::new(program).args(args).status() {
            Ok(st) => std::process::exit(st.code().unwrap_or(super::exit::IO)),
            Err(e) => e,
        }
    }
    /// Git for Windows' InstallPath, 64-bit view first.
    pub fn windows_install_path() -> Option<String> {
        let key = wide(OsStr::new("SOFTWARE\\GitForWindows"));
        let val = wide(OsStr::new("InstallPath"));
        for view in [RRF_SUBKEY_WOW6464KEY, RRF_SUBKEY_WOW6432KEY] {
            let mut size = 0u32;
            // SAFETY: documented two-call pattern; buffer sized from the first call.
            unsafe {
                if RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ | view, null_mut(), null_mut(), &mut size) != ERROR_SUCCESS || size == 0 {
                    continue;
                }
                let mut buf = vec![0u16; size as usize / 2 + 1];
                if RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), val.as_ptr(), RRF_RT_REG_SZ | view, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) != ERROR_SUCCESS {
                    continue;
                }
                let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let s = String::from_utf16_lossy(&buf[..len]);
                if !s.is_empty() {
                    return Some(s);
                }
            }
        }
        None
    }
    /// A running executable cannot be deleted; remove it at the next reboot.
    pub fn remove_self_binary(p: &Path) -> io::Result<()> {
        let w = wide(p.as_os_str());
        // SAFETY: documented call with a valid NUL-terminated path.
        if unsafe { MoveFileExW(w.as_ptr(), null(), MOVEFILE_DELAY_UNTIL_REBOOT) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn is_reparse_point(m: &Metadata) -> bool {
        m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
}

// ---------------------------------------------------------------------------
// Filesystem discipline
// ---------------------------------------------------------------------------

struct Ctx {
    layout: Layout,
    root: Option<PathBuf>,
    now: String,
}

impl Ctx {
    /// Owned by the administrator (or, under the test root, by the user running
    /// the suite) and writable by nobody else.
    fn trusted(&self, path: &Path, m: &fs::Metadata) -> bool {
        let owner_ok = match &self.root {
            Some(_) => sys::owned_by_current_user(path, m),
            None => sys::admin_owned(path, m),
        };
        owner_ok && !sys::loosely_writable(path, m)
    }

    /// Where the ownership walk stops: the filesystem root in real use, the
    /// test root in tests (its own ancestors belong to the user's temp dir).
    ///
    /// Windows: `%ProgramData%` is the well-known container whose DACL
    /// deliberately lets Users create entries — that is how every per-machine
    /// application folder is born — so it can never pass a "not writable by
    /// Users" test and is the boundary instead. Everything the helper creates
    /// below it carries its own explicit DACL and is verified as usual; a
    /// folder someone else planted there is caught by the ownership check.
    fn trust_boundary(&self, dir: &Path) -> PathBuf {
        match &self.root {
            Some(r) => r.clone(),
            None => {
                if cfg!(windows) {
                    let mut p = dir.to_path_buf();
                    while let Some(parent) = p.parent() {
                        let is_program_data = p
                            .file_name()
                            .map(|n| n.to_string_lossy().eq_ignore_ascii_case("ProgramData"))
                            .unwrap_or(false);
                        if is_program_data {
                            return p;
                        }
                        p = parent.to_path_buf();
                    }
                }
                let mut p = dir.to_path_buf();
                while let Some(parent) = p.parent() {
                    p = parent.to_path_buf();
                }
                p
            }
        }
    }

    /// Every component from the boundary down to `dir` must be a real
    /// directory (no symlink or reparse point), admin-owned and not group- or
    /// world-writable. The boundary's own ancestors are trusted (root-owned
    /// `/`, or the test root). Symlinks *above* `dir` are resolved first: on
    /// macOS `/etc` is itself a root-owned symlink to `/private/etc`.
    fn verify_dir_chain(&self, dir: &Path) -> R<()> {
        let boundary = self.trust_boundary(dir);
        let boundary = boundary.canonicalize().unwrap_or(boundary);
        let parent = dir.parent().ok_or_else(|| Fail::refused("managed directory has no parent"))?;
        // Components that do not exist yet will be created by us (root, 0755);
        // the walk starts at the deepest one that does.
        let mut existing = parent.to_path_buf();
        while !existing.exists() {
            match existing.parent() {
                Some(pp) => existing = pp.to_path_buf(),
                None => break,
            }
        }
        let canonical_parent = existing
            .canonicalize()
            .map_err(|e| Fail::refused(format!("cannot resolve {}: {}", existing.display(), e)))?;
        let mut checked: Vec<PathBuf> = Vec::new();
        let mut p = canonical_parent.clone();
        loop {
            if p == boundary || !p.starts_with(&boundary) {
                break;
            }
            checked.push(p.clone());
            match p.parent() {
                Some(pp) => p = pp.to_path_buf(),
                None => break,
            }
        }
        for c in checked {
            let m = fs::symlink_metadata(&c).map_err(|e| Fail::refused(format!("cannot stat {}: {}", c.display(), e)))?;
            if !m.is_dir() || m.file_type().is_symlink() || sys::is_reparse_point(&m) {
                return Err(Fail::refused(format!("{} is not a plain directory", c.display())));
            }
            if !self.trusted(&c, &m) {
                return Err(Fail::refused(format!("{} is not an administrator-only directory", c.display())));
            }
        }
        // The managed directory itself, if it exists: never a symlink, never squatted.
        match fs::symlink_metadata(dir) {
            Ok(m) => {
                if !m.is_dir() || m.file_type().is_symlink() || sys::is_reparse_point(&m) {
                    return Err(Fail::refused(format!("{} exists but is not a plain directory", dir.display())));
                }
                if !self.trusted(dir, &m) {
                    return Err(Fail::refused(format!(
                        "{} exists but is not owned by the administrator — remove it as an administrator and try again",
                        dir.display()
                    )));
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Fail::refused(format!("cannot stat {}: {}", dir.display(), e))),
        }
    }

    /// Create a managed directory, or check the one that is there. A symlink or
    /// reparse point in its place is always refused (a write into it would land
    /// wherever the planter chose); `strict` additionally requires
    /// administrator ownership, which every registry directory must have while
    /// a Homebrew-owned `/usr/local/bin` merely gets reported.
    fn ensure_dir(&self, dir: &Path, mode: u32, strict: bool) -> R<()> {
        match fs::symlink_metadata(dir) {
            Ok(m) if m.file_type().is_symlink() || sys::is_reparse_point(&m) => {
                // Inside the registry tree nothing may ever be a link: only the
                // administrator can create anything there, and a registry that
                // points elsewhere is not a registry. Outside it — macOS's
                // `/etc -> private/etc` — an administrator-owned link is the
                // administrator's decision: follow it once and hold the target
                // to the same standard. A link anyone else planted is refused.
                if dir.starts_with(&self.layout.registry_dir) {
                    return Err(Fail::refused(format!("{} exists but is not a plain directory", dir.display())));
                }
                let link_owner_ok = match &self.root {
                    Some(_) => sys::owned_by_current_user(dir, &m),
                    None => sys::admin_owned(dir, &m),
                };
                if !link_owner_ok {
                    return Err(Fail::refused(format!("{} is a link not owned by the administrator", dir.display())));
                }
                let target = fs::canonicalize(dir).map_err(|e| Fail::refused(format!("cannot resolve {}: {}", dir.display(), e)))?;
                let tm = fs::symlink_metadata(&target).map_err(|e| Fail::refused(format!("cannot stat {}: {}", target.display(), e)))?;
                if !tm.is_dir() || tm.file_type().is_symlink() || sys::is_reparse_point(&tm) {
                    return Err(Fail::refused(format!("{} resolves to {}, which is not a plain directory", dir.display(), target.display())));
                }
                if strict && !self.trusted(&target, &tm) {
                    return Err(Fail::refused(format!("{} resolves to {}, which is not an administrator-only directory", dir.display(), target.display())));
                }
                Ok(())
            }
            Ok(m) => {
                if !m.is_dir() {
                    return Err(Fail::refused(format!("{} exists but is not a directory", dir.display())));
                }
                if strict && !self.trusted(dir, &m) {
                    return Err(Fail::refused(format!(
                        "{} exists but is not an administrator-only directory — remove it as an administrator and try again",
                        dir.display()
                    )));
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Create every missing level ourselves, deepest last, and give
                // each one the administrator-only ownership. `create_dir_all`
                // would leave the intermediate directories with whatever their
                // parent hands down — on Windows, `%ProgramData%` hands down
                // "Users may create files", and the registry directory would
                // then fail its own trust check on the very next run.
                let mut missing: Vec<PathBuf> = Vec::new();
                let mut p = dir.to_path_buf();
                while !p.exists() {
                    missing.push(p.clone());
                    match p.parent() {
                        Some(pp) => p = pp.to_path_buf(),
                        None => break,
                    }
                }
                for d in missing.iter().rev() {
                    fs::create_dir(d).map_err(|e| Fail::io(&format!("create {}", d.display()), e))?;
                    sys::set_dir_mode(d, mode).map_err(|e| Fail::io(&format!("chmod {}", d.display()), e))?;
                    if self.root.is_none() {
                        sys::lock_down(d).map_err(|e| Fail::io(&format!("chown {}", d.display()), e))?;
                    }
                }
                Ok(())
            }
            Err(e) => Err(Fail::io(&format!("stat {}", dir.display()), e)),
        }
    }

    /// Temp file next to the target, `O_EXCL`, fsync, rename. The target is
    /// never opened, so a symlink planted there is replaced, not followed.
    fn write_atomic(&self, path: &Path, bytes: &[u8], mode: u32) -> R<()> {
        let dir = path.parent().ok_or_else(|| Fail::refused("target has no parent"))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let tmp = dir.join(format!(".{}.tmp.{}", name, std::process::id()));
        let _ = fs::remove_file(&tmp);
        let result = (|| -> R<()> {
            let mut f = sys::create_new_nofollow(&tmp, mode).map_err(|e| Fail::io(&format!("create {}", tmp.display()), e))?;
            f.write_all(bytes).map_err(|e| Fail::io(&format!("write {}", tmp.display()), e))?;
            f.sync_all().map_err(|e| Fail::io(&format!("sync {}", tmp.display()), e))?;
            sys::set_mode(&f, mode).map_err(|e| Fail::io(&format!("chmod {}", tmp.display()), e))?;
            if self.root.is_none() {
                sys::chown(&f, 0, 0).map_err(|e| Fail::io(&format!("chown {}", tmp.display()), e))?;
            }
            drop(f);
            if let Ok(m) = fs::symlink_metadata(path) {
                if m.file_type().is_symlink() || sys::is_reparse_point(&m) {
                    fs::remove_file(path).map_err(|e| Fail::io(&format!("remove symlink {}", path.display()), e))?;
                }
            }
            fs::rename(&tmp, path).map_err(|e| Fail::io(&format!("rename into {}", path.display()), e))?;
            if self.root.is_none() {
                // Unix: already root-owned via the descriptor; Windows: the
                // explicit DACL goes on the final path.
                sys::lock_down(path).map_err(|e| Fail::io(&format!("lock down {}", path.display()), e))?;
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    /// Read a managed or job file without following a symlink; refuse
    /// anything that is not a regular file of sane size.
    fn read_nofollow(&self, path: &Path, max: u64) -> R<(Vec<u8>, fs::Metadata)> {
        let mut f = sys::open_read_nofollow(path).map_err(|e| Fail::refused(format!("cannot open {}: {}", path.display(), e)))?;
        let m = f.metadata().map_err(|e| Fail::refused(format!("cannot stat {}: {}", path.display(), e)))?;
        if !m.is_file() {
            return Err(Fail::refused(format!("{} is not a regular file", path.display())));
        }
        if m.len() > max {
            return Err(Fail::refused(format!("{} is larger than {} bytes", path.display(), max)));
        }
        let mut buf = Vec::with_capacity(m.len() as usize);
        f.read_to_end(&mut buf).map_err(|e| Fail::refused(format!("cannot read {}: {}", path.display(), e)))?;
        Ok((buf, m))
    }
}

// ---------------------------------------------------------------------------
// Registry and managed files
// ---------------------------------------------------------------------------

fn load_registry(ctx: &Ctx, system_gitconfig: &str) -> R<Registry> {
    ctx.verify_dir_chain(&ctx.layout.registry_dir)?;
    if !ctx.layout.locks_json.exists() {
        return Ok(Registry::new(&ctx.layout, system_gitconfig, &ctx.now));
    }
    let (bytes, m) = ctx.read_nofollow(&ctx.layout.locks_json, 4 * 1024 * 1024)?;
    if !ctx.trusted(&ctx.layout.locks_json, &m) {
        return Err(Fail::refused(format!("{} is not administrator-owned", ctx.layout.locks_json.display())));
    }
    let reg: Registry = serde_json::from_slice(&bytes).map_err(|e| Fail::refused(format!("registry is not readable JSON: {}", e)))?;
    if reg.schema > lc::SCHEMA {
        return Err(Fail::new(exit::OUTDATED, format!("registry schema {} is newer than this helper ({}); upgrade the helper", reg.schema, lc::SCHEMA)));
    }
    Ok(reg)
}

fn write_registry(ctx: &Ctx, reg: &Registry) -> R<()> {
    ctx.ensure_dir(&ctx.layout.registry_dir, 0o755, true)?;
    ctx.write_atomic(&ctx.layout.locks_json, reg.to_json().as_bytes(), 0o644)
}

fn backup_system_gitconfig(ctx: &Ctx, content: &[u8]) -> R<()> {
    ctx.ensure_dir(&ctx.layout.backups_dir, 0o755, true)?;
    let stamp = ctx.now.replace([':', '-'], "");
    let name = format!("gitconfig.{}.{}", stamp, std::process::id());
    ctx.write_atomic(&ctx.layout.backups_dir.join(name), content, 0o644)?;
    // Keep the newest MAX_BACKUPS; names are fixed-width timestamps so
    // lexicographic order is chronological.
    if let Ok(entries) = fs::read_dir(&ctx.layout.backups_dir) {
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("gitconfig.")))
            .collect();
        names.sort();
        if names.len() > MAX_BACKUPS {
            for old in &names[..names.len() - MAX_BACKUPS] {
                let _ = fs::remove_file(old);
            }
        }
    }
    Ok(())
}

/// Bring the system gitconfig's marker block in line with the registry:
/// present when there are locks, absent otherwise. Text surgery, never
/// `git config`, and always backed up first.
fn sync_system_gitconfig(ctx: &Ctx, reg: &Registry, changes: &mut Vec<Change>) -> R<()> {
    let path = &ctx.layout.system_gitconfig;
    let existing: Option<Vec<u8>> = match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() || sys::is_reparse_point(&m) => {
            return Err(Fail::refused(format!("{} is a symlink; refusing to edit it", path.display())));
        }
        Ok(_) => Some(ctx.read_nofollow(path, 16 * 1024 * 1024)?.0),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Fail::io(&format!("stat {}", path.display()), e)),
    };
    let old_text = existing
        .as_ref()
        .map(|b| String::from_utf8_lossy(b).to_string())
        .unwrap_or_default();
    if let Some(b) = &existing {
        if String::from_utf8(b.clone()).is_err() {
            return Err(Fail::refused(format!("{} is not UTF-8 text; refusing to edit it", path.display())));
        }
    }
    let include = lc::path_str(&ctx.layout.include_file);
    let new_text = if reg.locks.is_empty() {
        lc::remove_marker_block(&old_text)
    } else {
        lc::upsert_marker_block(&old_text, &include)
    };
    if new_text == old_text {
        return Ok(());
    }
    if let Some(b) = &existing {
        backup_system_gitconfig(ctx, b)?;
    }
    if new_text.trim().is_empty() {
        // Nothing but our block was ever in it; leave no empty file behind.
        if existing.is_some() {
            fs::remove_file(path).map_err(|e| Fail::io(&format!("remove {}", path.display()), e))?;
            changes.push(Change { op: "system-gitconfig".into(), repo: None, detail: format!("{} removed (it held only the GitSwitch block)", path.display()) });
        }
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        ctx.ensure_dir(dir, 0o755, true)?;
    }
    ctx.write_atomic(path, new_text.as_bytes(), 0o644)?;
    changes.push(Change {
        op: "system-gitconfig".into(),
        repo: None,
        detail: if reg.locks.is_empty() {
            format!("GitSwitch block removed from {}", path.display())
        } else {
            format!("GitSwitch block present in {}", path.display())
        },
    });
    Ok(())
}

/// Regenerate the include file and every stanza from the registry; delete
/// stanza files the registry no longer names.
fn sync_registry_files(ctx: &Ctx, reg: &Registry) -> R<()> {
    ctx.ensure_dir(&ctx.layout.registry_dir, 0o755, true)?;
    ctx.ensure_dir(&ctx.layout.stanza_dir, 0o755, true)?;
    let mut wanted: Vec<String> = Vec::new();
    for entry in &reg.locks {
        let name = lc::stanza_file_name(&entry.id);
        wanted.push(name.clone());
        ctx.write_atomic(&ctx.layout.stanza_dir.join(name), lc::render_stanza(entry).as_bytes(), 0o644)?;
    }
    if let Ok(entries) = fs::read_dir(&ctx.layout.stanza_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".gitconfig") && !wanted.contains(&name) {
                let _ = fs::remove_file(e.path());
            }
        }
    }
    if reg.locks.is_empty() {
        let _ = fs::remove_file(&ctx.layout.include_file);
    } else {
        ctx.write_atomic(&ctx.layout.include_file, lc::render_include_file(reg).as_bytes(), 0o644)?;
    }
    Ok(())
}

fn current_exe_bytes() -> R<(PathBuf, Vec<u8>)> {
    let exe = std::env::current_exe().map_err(|e| Fail::io("locate this executable", e))?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let bytes = fs::read(&exe).map_err(|e| Fail::io(&format!("read {}", exe.display()), e))?;
    Ok((exe, bytes))
}

fn helper_info(ctx: &Ctx, path: &Path, bytes: &[u8]) -> lc::HelperInfo {
    lc::HelperInfo {
        path: lc::path_str(path),
        version: VERSION.into(),
        sha256: lc::sha256_hex(bytes),
        installed_at: ctx.now.clone(),
    }
}

/// Copy the running executable into the admin-owned helper location.
fn install_self(ctx: &Ctx, reg: &mut Registry) -> R<()> {
    let (_, bytes) = current_exe_bytes()?;
    let dest = &ctx.layout.helper_path;
    if let Some(dir) = dest.parent() {
        ctx.verify_dir_chain(dir)?;
        ctx.ensure_dir(dir, 0o755, true)?;
    }
    ctx.write_atomic(dest, &bytes, 0o755)?;
    reg.helper = Some(helper_info(ctx, dest, &bytes));
    Ok(())
}

/// Is there a helper at the installed path whose bytes the registry vouches for?
fn installed_helper_valid(ctx: &Ctx, reg: &Registry) -> bool {
    let Some(info) = &reg.helper else { return false };
    let Ok((bytes, m)) = ctx.read_nofollow(&ctx.layout.helper_path, 512 * 1024 * 1024) else { return false };
    ctx.trusted(&ctx.layout.helper_path, &m) && lc::sha256_hex(&bytes) == info.sha256
}

/// The remote helper is the installed helper under a second name (a symlink
/// on Unix, a copy on Windows). A foreign file of that name is left alone and
/// reported, never replaced.
fn install_remote_helper(ctx: &Ctx, reg: &mut Registry, changes: &mut Vec<Change>, errors: &mut Vec<String>) {
    let link = &ctx.layout.remote_helper_path;
    let target = &ctx.layout.helper_path;
    let dir = match link.parent() {
        Some(d) => d.to_path_buf(),
        None => return,
    };
    if let Err(e) = ctx.ensure_dir(&dir, 0o755, false) {
        errors.push(e.msg);
        return;
    }
    let dir_admin_owned = fs::symlink_metadata(&dir).map(|m| ctx.trusted(&dir, &m)).unwrap_or(false);
    let ours = match fs::symlink_metadata(link) {
        Ok(m) if m.file_type().is_symlink() => fs::read_link(link).map(|t| t == *target).unwrap_or(false),
        Ok(_) => {
            let same = fs::read(link).ok().map(|b| lc::sha256_hex(&b)) == reg.helper.as_ref().map(|h| h.sha256.clone());
            if !same {
                errors.push(format!("{} exists and is not GitSwitch's; left alone", link.display()));
                return;
            }
            true
        }
        Err(_) => false,
    };
    if !ours {
        let _ = fs::remove_file(link);
        if let Err(e) = sys::symlink(target, link) {
            errors.push(format!("install remote helper at {}: {}", link.display(), e));
            return;
        }
        changes.push(Change { op: "install-remote-helper".into(), repo: None, detail: format!("{} installed", link.display()) });
    }
    reg.remote_helper = Some(lc::RemoteHelperInfo { path: lc::path_str(link), dir_admin_owned, installed_at: ctx.now.clone() });
}

/// The pkexec policy is a convenience (a graphical prompt instead of sudo or
/// the manual command); the lock does not depend on it. So a machine without
/// polkit, or one whose policy directory cannot be written, is reported as a
/// `polkit` change with the reason — never as a failure of the job.
fn install_polkit_policy(ctx: &Ctx, changes: &mut Vec<Change>, _errors: &mut Vec<String>) {
    let Some(policy) = &ctx.layout.polkit_policy else { return };
    let text = lc::render_polkit_policy(&lc::path_str(&ctx.layout.helper_path));
    if fs::read(policy).ok().as_deref() == Some(text.as_bytes()) {
        return;
    }
    let Some(dir) = policy.parent() else { return };
    let polkit_root = dir.parent().unwrap_or(dir);
    if ctx.root.is_none() && !polkit_root.exists() {
        changes.push(Change { op: "polkit".into(), repo: None, detail: "skipped: polkit is not installed on this machine".into() });
        return;
    }
    let note = match ctx.ensure_dir(dir, 0o755, true).and_then(|_| ctx.write_atomic(policy, text.as_bytes(), 0o644)) {
        Ok(()) => format!("pkexec policy installed at {}", policy.display()),
        Err(e) => format!("pkexec policy not installed ({}); the app uses sudo or the manual command instead", e.msg),
    };
    changes.push(Change { op: "polkit".into(), repo: None, detail: note });
}

fn append_audit(ctx: &Ctx, line: &str) {
    if ctx.ensure_dir(&ctx.layout.registry_dir, 0o755, true).is_err() {
        return;
    }
    if let Ok(mut f) = sys::open_append_nofollow(&ctx.layout.audit_log, 0o644) {
        let _ = writeln!(f, "{}", line.replace(['\n', '\r'], " "));
        if ctx.root.is_none() {
            let _ = sys::chown(&f, 0, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------------

struct JobFile {
    path: PathBuf,
    job: Job,
    owner_uid: u32,
    owner_gid: u32,
}

/// Open once, `fstat`, read once, hash, compare with the hash in the approved
/// command line. A file swapped after approval fails here.
fn read_job(ctx: &Ctx, path: &Path, expected_sha: &str) -> R<JobFile> {
    if !path.is_absolute() {
        return Err(Fail::usage("the job path must be absolute"));
    }
    let (bytes, m) = ctx.read_nofollow(path, lc::MAX_JOB_BYTES)?;
    let actual = lc::sha256_hex(&bytes);
    if actual != expected_sha {
        return Err(Fail::refused("the job file does not match the checksum that was approved"));
    }
    let job: Job = serde_json::from_slice(&bytes).map_err(|e| Fail::usage(format!("job is not valid: {}", e)))?;
    if job.schema != lc::SCHEMA {
        return Err(Fail::usage(format!("job schema {} is not {}", job.schema, lc::SCHEMA)));
    }
    Ok(JobFile { path: path.to_path_buf(), job, owner_uid: sys::owner_uid(&m), owner_gid: sys::owner_gid(&m) })
}

fn write_result(ctx: &Ctx, jf: Option<&JobFile>, result: &JobResult) {
    let json = result.to_json();
    print!("{}", json);
    let _ = std::io::stdout().flush();
    let Some(jf) = jf else { return };
    let mut name = jf.path.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| OsString::from("job"));
    name.push(".result.json");
    let out = jf.path.with_file_name(name);
    let _ = fs::remove_file(&out);
    if let Ok(mut f) = sys::create_new_nofollow(&out, 0o644) {
        let _ = f.write_all(json.as_bytes());
        if ctx.root.is_none() {
            let _ = sys::chown(&f, jf.owner_uid, jf.owner_gid);
        }
    }
}

fn summarise(changes: &[Change], errors: &[String]) -> String {
    let locked = changes.iter().filter(|c| c.op == "lock" && c.detail == "locked").count();
    let updated = changes.iter().filter(|c| c.op == "lock" && c.detail == "lock updated").count();
    let unlocked = changes.iter().filter(|c| c.op == "unlock" && c.detail == "unlocked").count();
    let mut parts = Vec::new();
    let repos = |n: usize| if n == 1 { "1 repository".to_string() } else { format!("{} repositories", n) };
    if locked > 0 {
        parts.push(format!("Locked {}.", repos(locked)));
    }
    if updated > 0 {
        parts.push(format!("Updated the lock on {}.", repos(updated)));
    }
    if unlocked > 0 {
        parts.push(format!("Unlocked {}.", repos(unlocked)));
    }
    if changes.iter().any(|c| c.op == "uninstall-all") {
        parts.push("Push locks and the lock helper were removed.".into());
    }
    if changes.iter().any(|c| c.op == "bootstrap") {
        parts.push("Lock helper installed.".into());
    }
    if parts.is_empty() {
        parts.push("Nothing needed doing.".into());
    }
    if !errors.is_empty() {
        parts.push(format!("{} problem(s) were recorded.", errors.len()));
    }
    parts.join(" ")
}

fn resolve_platform_and_layout(job_system_gitconfig: Option<&str>) -> R<(Platform, Layout)> {
    let platform = Platform::current().ok_or_else(|| Fail::usage("unsupported operating system"))?;
    let root = test_root();
    let install = sys::windows_install_path();
    let mut layout = Layout::new(platform, root.as_deref(), install.as_deref());
    if let Some(given) = job_system_gitconfig {
        let given_n = lc::norm(given);
        match platform {
            Platform::Linux => {
                if !lc::system_gitconfig_allowed(platform, &given_n) {
                    return Err(Fail::refused(format!("{} is not a system gitconfig this helper manages", given)));
                }
                layout.system_gitconfig = match &root {
                    Some(r) => r.join(given_n.trim_start_matches('/')),
                    None => PathBuf::from(&given_n),
                };
            }
            Platform::Macos | Platform::Windows => {
                let expected = match &root {
                    // Under a test root the job names the real path; compare the un-rooted forms.
                    Some(_) => lc::path_str(&Layout::new(platform, None, install.as_deref()).system_gitconfig),
                    None => lc::path_str(&layout.system_gitconfig),
                };
                if !given_n.eq_ignore_ascii_case(&expected) {
                    return Err(Fail::refused(format!("the system gitconfig on this machine is {}, not {}", expected, given)));
                }
            }
        }
    }
    Ok((platform, layout))
}

fn require_elevated(root: &Option<PathBuf>) -> R<()> {
    if root.is_some() {
        if sys::is_root() {
            return Err(Fail::refused("GITSWITCH_LOCK_ROOT is a test mode and is refused when running as the administrator"));
        }
        return Ok(());
    }
    if !sys::is_elevated() {
        return Err(Fail::new(exit::NOT_ELEVATED, "administrator rights are required"));
    }
    Ok(())
}

/// A refusal or failure, written where the app looks for results: on Windows
/// the UAC boundary swallows stderr, so this is how the reason gets back.
fn write_error_result(job_path: &Path, f: &Fail) {
    if !job_path.is_absolute() {
        return;
    }
    let mut name = job_path.file_name().map(|n| n.to_os_string()).unwrap_or_else(|| OsString::from("job"));
    name.push(".result.json");
    let out = job_path.with_file_name(name);
    if out.exists() {
        return;
    }
    let result = JobResult {
        schema: lc::SCHEMA,
        nonce: String::new(),
        ok: false,
        helper_version: VERSION.into(),
        changed: Vec::new(),
        errors: vec![f.msg.clone()],
        message: f.msg.clone(),
        registry_sha256: None,
    };
    if let Ok(mut file) = sys::create_new_nofollow(&out, 0o644) {
        let _ = file.write_all(result.to_json().as_bytes());
    }
}

fn run_job(job_path: &Path, sha: &str, bootstrap: bool) -> R<i32> {
    let r = run_job_inner(job_path, sha, bootstrap);
    if let Err(f) = &r {
        write_error_result(job_path, f);
    }
    r
}

fn run_job_inner(job_path: &Path, sha: &str, bootstrap: bool) -> R<i32> {
    let root = test_root();
    require_elevated(&root)?;
    // Read the job with a provisional context so the layout can honour a
    // Linux system-gitconfig override; destinations never come from the job.
    let (_, provisional) = resolve_platform_and_layout(None)?;
    let ctx0 = Ctx { layout: provisional, root: root.clone(), now: now() };
    let jf = read_job(&ctx0, job_path, sha)?;
    let (platform, layout) = resolve_platform_and_layout(Some(&jf.job.system_gitconfig))?;
    let ctx = Ctx { layout, root, now: ctx0.now.clone() };
    lc::validate_job(&jf.job, platform).map_err(Fail::refused)?;

    let system_gitconfig = lc::path_str(&ctx.layout.system_gitconfig);
    let mut reg = load_registry(&ctx, &system_gitconfig)?;
    let (exe, exe_bytes) = current_exe_bytes()?;
    let installed = ctx.layout.helper_path.canonicalize().unwrap_or(ctx.layout.helper_path.clone());

    if bootstrap {
        let wants_reinstall = jf.job.ops.iter().any(|o| matches!(o, Op::UpgradeHelper { sig: None, .. }));
        if installed_helper_valid(&ctx, &reg) && !wants_reinstall {
            return Err(Fail::refused(format!(
                "a lock helper is already installed at {}; run that one, or ask for an upgrade",
                ctx.layout.helper_path.display()
            )));
        }
        install_self(&ctx, &mut reg)?;
        write_registry(&ctx, &reg)?;
        append_audit(&ctx, &format!("{} nonce={} by_uid={} bootstrap installed helper from {}", ctx.now, jf.job.nonce, jf.owner_uid, exe.display()));
        // From here on only admin-owned code runs.
        let args: Vec<OsString> = vec![job_path.as_os_str().to_os_string(), OsString::from("--sha256"), OsString::from(sha)];
        let e = sys::exec(&ctx.layout.helper_path, &args);
        return Err(Fail::io(&format!("run the installed helper {}", ctx.layout.helper_path.display()), e));
    }

    if exe != installed && !ctx.layout.helper_path.exists() {
        return Err(Fail::refused(format!(
            "no lock helper is installed at {}; the app must bootstrap first",
            ctx.layout.helper_path.display()
        )));
    }
    if exe != installed {
        return Err(Fail::refused(format!(
            "only the installed helper at {} may apply jobs (this is {})",
            ctx.layout.helper_path.display(),
            exe.display()
        )));
    }
    // We are the installed copy: make sure the registry vouches for these bytes.
    let self_sha = lc::sha256_hex(&exe_bytes);
    if reg.helper.as_ref().map(|h| h.sha256.as_str()) != Some(self_sha.as_str()) {
        reg.helper = Some(helper_info(&ctx, &ctx.layout.helper_path, &exe_bytes));
    }

    let mut errors: Vec<String> = Vec::new();
    let mut changes = lc::apply_ops(&mut reg, &jf.job, &ctx.now).map_err(Fail::refused)?;

    if jf.job.ops.iter().any(|o| matches!(o, Op::UninstallAll)) {
        let code = uninstall_all(&ctx, &mut reg, &mut changes, &mut errors);
        let result = JobResult {
            schema: lc::SCHEMA,
            nonce: jf.job.nonce.clone(),
            ok: errors.is_empty(),
            helper_version: VERSION.into(),
            message: summarise(&changes, &errors),
            changed: changes,
            errors,
            registry_sha256: None,
        };
        write_result(&ctx, Some(&jf), &result);
        return Ok(code);
    }

    for op in &jf.job.ops {
        match op {
            Op::Bootstrap => {
                install_remote_helper(&ctx, &mut reg, &mut changes, &mut errors);
                install_polkit_policy(&ctx, &mut changes, &mut errors);
                changes.push(Change { op: "bootstrap".into(), repo: None, detail: format!("helper {} at {}", VERSION, ctx.layout.helper_path.display()) });
            }
            Op::InstallRemoteHelper => {
                install_remote_helper(&ctx, &mut reg, &mut changes, &mut errors);
                install_polkit_policy(&ctx, &mut changes, &mut errors);
            }
            Op::UpgradeHelper { source, sig, version } => {
                let src = PathBuf::from(source);
                let (bytes, _) = ctx.read_nofollow(&src, 512 * 1024 * 1024)?;
                match sig {
                    Some(sig_path) => {
                        let (sig_bytes, _) = ctx.read_nofollow(&PathBuf::from(sig_path), 64 * 1024)?;
                        let sig_text = String::from_utf8(sig_bytes).map_err(|_| Fail::refused("signature file is not text"))?;
                        lc::verify_minisign(lc::RELEASE_PUBKEY_B64, &bytes, &sig_text).map_err(Fail::refused)?;
                        if !lc::version_at_least(version, VERSION) {
                            return Err(Fail::refused(format!("refusing to downgrade the helper from {} to {}", VERSION, version)));
                        }
                        ctx.write_atomic(&ctx.layout.helper_path, &bytes, 0o755)?;
                        reg.helper = Some(lc::HelperInfo { path: lc::path_str(&ctx.layout.helper_path), version: version.clone(), sha256: lc::sha256_hex(&bytes), installed_at: ctx.now.clone() });
                        changes.push(Change { op: "upgrade-helper".into(), repo: None, detail: format!("helper upgraded to {} (signature verified)", version) });
                    }
                    None => {
                        // Only a --bootstrap run of the source itself may install
                        // unsigned bytes; that has already happened when the
                        // source's bytes are ours.
                        if lc::sha256_hex(&bytes) == self_sha {
                            changes.push(Change { op: "upgrade-helper".into(), repo: None, detail: format!("helper reinstalled from the app bundle ({})", VERSION) });
                        } else {
                            return Err(Fail::refused("an unsigned helper upgrade must be run as a bootstrap of the new helper itself"));
                        }
                    }
                }
            }
            Op::Lock { .. } | Op::Unlock { .. } | Op::RepairSystem | Op::UninstallAll => {}
        }
    }

    // Regenerate everything from the registry, every time: repair is not a
    // special case, it is what always happens.
    sync_registry_files(&ctx, &reg)?;
    sync_system_gitconfig(&ctx, &reg, &mut changes)?;
    write_registry(&ctx, &reg)?;
    let registry_sha = fs::read(&ctx.layout.locks_json).ok().map(|b| lc::sha256_hex(&b));

    let ops: Vec<String> = jf.job.ops.iter().map(|o| match o.repo() { Some(r) => format!("{}:{}", o.kind(), r), None => o.kind().to_string() }).collect();
    append_audit(&ctx, &format!(
        "{} nonce={} by_uid={} ops={} changes={} errors={} result={}",
        ctx.now, jf.job.nonce, jf.owner_uid, ops.join(","), changes.len(), errors.len(),
        if errors.is_empty() { "ok" } else { "partial" }
    ));

    let ok = errors.is_empty();
    let result = JobResult {
        schema: lc::SCHEMA,
        nonce: jf.job.nonce.clone(),
        ok,
        helper_version: VERSION.into(),
        message: summarise(&changes, &errors),
        changed: changes,
        errors,
        registry_sha256: registry_sha,
    };
    write_result(&ctx, Some(&jf), &result);
    Ok(if ok { exit::OK } else { exit::IO })
}

/// Remove every trace: the marker block, the registry, the remote helper, the
/// polkit policy and the helper itself. Per-repo mirrors are the user's own
/// files and the app removes those.
fn uninstall_all(ctx: &Ctx, reg: &mut Registry, changes: &mut Vec<Change>, errors: &mut Vec<String>) -> i32 {
    reg.locks.clear();
    if let Err(e) = sync_system_gitconfig(ctx, reg, changes) {
        errors.push(e.msg);
    }
    let link = &ctx.layout.remote_helper_path;
    if let Ok(m) = fs::symlink_metadata(link) {
        let ours = if m.file_type().is_symlink() {
            fs::read_link(link).map(|t| t == ctx.layout.helper_path).unwrap_or(false)
        } else {
            fs::read(link).ok().map(|b| lc::sha256_hex(&b)) == reg.helper.as_ref().map(|h| h.sha256.clone())
        };
        if ours {
            if let Err(e) = fs::remove_file(link) {
                errors.push(format!("remove {}: {}", link.display(), e));
            }
        } else {
            errors.push(format!("{} is not GitSwitch's; left alone", link.display()));
        }
    }
    if let Some(p) = &ctx.layout.polkit_policy {
        let _ = fs::remove_file(p);
    }
    if ctx.layout.registry_dir.exists() {
        if let Err(e) = fs::remove_dir_all(&ctx.layout.registry_dir) {
            errors.push(format!("remove {}: {}", ctx.layout.registry_dir.display(), e));
        }
    }
    if ctx.layout.helper_path.exists() {
        if let Err(e) = sys::remove_self_binary(&ctx.layout.helper_path) {
            errors.push(format!("remove {}: {}", ctx.layout.helper_path.display(), e));
        }
    }
    if let Some(dir) = ctx.layout.helper_path.parent() {
        if dir.ends_with("gitswitch") {
            let _ = fs::remove_dir(dir);
        }
    }
    changes.push(Change { op: "uninstall-all".into(), repo: None, detail: "marker block, registry, remote helper and helper removed".into() });
    if errors.is_empty() { exit::OK } else { exit::IO }
}

fn run_uninstall_command() -> R<i32> {
    let root = test_root();
    require_elevated(&root)?;
    let (_platform, layout) = resolve_platform_and_layout(None)?;
    let ctx = Ctx { layout, root, now: now() };
    let system_gitconfig = lc::path_str(&ctx.layout.system_gitconfig);
    let mut reg = match load_registry(&ctx, &system_gitconfig) {
        Ok(r) => r,
        Err(f) if f.code == exit::OUTDATED => Registry::new(&ctx.layout, &system_gitconfig, &ctx.now),
        Err(f) => return Err(f),
    };
    let (mut changes, mut errors) = (Vec::new(), Vec::new());
    let code = uninstall_all(&ctx, &mut reg, &mut changes, &mut errors);
    let result = JobResult {
        schema: lc::SCHEMA,
        nonce: String::new(),
        ok: errors.is_empty(),
        helper_version: VERSION.into(),
        message: summarise(&changes, &errors),
        changed: changes,
        errors,
        registry_sha256: None,
    };
    write_result(&ctx, None, &result);
    Ok(code)
}

// ---------------------------------------------------------------------------
// Remote helper mode: explain, then abort the session
// ---------------------------------------------------------------------------

fn git_config_get(key: &str) -> Option<String> {
    let out = std::process::Command::new("git").args(["config", "--get", key]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// git runs `git-remote-gitswitch-push-blocked <remote> <url>` as the user and
/// waits for a `capabilities` reply. Exiting without one aborts the session,
/// and our stderr is what the person (or agent) reads first.
fn remote_helper_mode(args: &[OsString]) -> i32 {
    let remote = args.first().map(|a| a.to_string_lossy().to_string());
    let url = args.get(1).map(|a| a.to_string_lossy().to_string());
    let remote_name = remote.as_deref().filter(|r| Some(*r) != url.as_deref());
    let text = if let Some(repo) = git_config_get("gitswitch.lockedRepo") {
        lc::locked_refusal(Some(&repo), remote_name, url.as_deref())
    } else if git_config_get("gitswitch.pushBlocked").as_deref() == Some("true") {
        lc::guardrail_refusal(remote_name)
    } else {
        lc::profile_refusal(remote_name)
    };
    eprint!("{}", text);
    1
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn run(args: &[OsString]) -> R<i32> {
    let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().to_string()).collect();
    match s.as_slice() {
        [v] if v == "--version" => {
            println!("{} {}", lc::HELPER_BIN_NAME, VERSION);
            Ok(exit::OK)
        }
        [u] if u == "--uninstall-all" => run_uninstall_command(),
        [job, flag, sha] if flag == "--sha256" && is_sha256_hex(sha) => run_job(Path::new(&args[0]), sha, false),
        [b, job, flag, sha] if b == "--bootstrap" && flag == "--sha256" && is_sha256_hex(sha) => {
            let _ = job;
            run_job(Path::new(&args[1]), sha, true)
        }
        _ => Err(Fail::usage(format!(
            "usage: {n} <job.json> --sha256 <hex> | {n} --bootstrap <job.json> --sha256 <hex> | {n} --uninstall-all | {n} --version",
            n = lc::HELPER_BIN_NAME
        ))),
    }
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args
        .first()
        .and_then(|a| Path::new(a).file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if argv0.trim_end_matches(".exe") == lc::REMOTE_HELPER_NAME {
        std::process::exit(remote_helper_mode(&args[1..]));
    }
    let code = match run(&args[1..]) {
        Ok(c) => c,
        Err(f) => {
            eprintln!("{}: {}", lc::HELPER_BIN_NAME, f.msg);
            f.code
        }
    };
    std::process::exit(code);
}
