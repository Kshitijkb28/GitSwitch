//! Windows file ownership, shared by the app and the privileged helper so
//! "administrator-owned" means exactly the same thing on both sides of the
//! push lock. Owned by BUILTIN\Administrators, SYSTEM or TrustedInstaller,
//! with a DACL that grants no write right to Users, Everyone, Authenticated
//! Users or INTERACTIVE.

// Win32's security API is unsafe by nature; every block below states why it
// is sound. The crate forbids unsafe code everywhere else (lib.rs).
#![allow(unsafe_code)]

use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, GENERIC_WRITE, HLOCAL};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows_sys::Win32::Security::{
    GetAce, IsWellKnownSid, WinAuthenticatedUserSid, WinBuiltinAdministratorsSid, WinBuiltinUsersSid, WinInteractiveSid,
    WinLocalSystemSid, WinWorldSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID,
};
use windows_sys::Win32::Storage::FileSystem::{DELETE, WRITE_DAC, WRITE_OWNER};
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
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
pub fn admin_owned(p: &Path) -> bool {
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
/// Does any allow-ACE grant a write right to a low-privilege group?
pub fn loosely_writable(p: &Path) -> bool {
    let Some(sec) = security(p) else { return true };
    if sec.dacl.is_null() {
        return true; // a NULL DACL grants everything to everyone
    }
    // Only rights that change something. FILE_GENERIC_WRITE would be
    // wrong here: it carries READ_CONTROL and SYNCHRONIZE, which a plain
    // read-and-execute ACE (our own Users entry) carries too, so every
    // directory the helper locks down would count as writable.
    const FILE_WRITE_DATA: u32 = 0x0002;
    const FILE_APPEND_DATA: u32 = 0x0004;
    const FILE_WRITE_EA: u32 = 0x0010;
    const FILE_DELETE_CHILD: u32 = 0x0040;
    const FILE_WRITE_ATTRIBUTES: u32 = 0x0100;
    const GENERIC_ALL: u32 = 0x1000_0000;
    let write = FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | FILE_WRITE_EA
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | GENERIC_WRITE
        | GENERIC_ALL;
    // SAFETY: dacl points into sd, alive while `sec` is; GetAce bounds-checks.
    unsafe {
        let count = (*sec.dacl).AceCount as u32;
        for i in 0..count {
            let mut ace: *mut c_void = null_mut();
            if GetAce(sec.dacl, i, &mut ace) == 0 || ace.is_null() {
                continue;
            }
            let hdr = &*(ace as *const ACE_HEADER);
            // ACCESS_ALLOWED_ACE_TYPE is already a u32 in windows-sys; the AceType byte widens.
            if u32::from(hdr.AceType) != ACCESS_ALLOWED_ACE_TYPE {
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
