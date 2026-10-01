//! Windows specifics: hidden child processes, process-tree cleanup, single instance, login startup,
//! tray notifications.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, GetLastError, HANDLE,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
    RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateMutexW};
use windows_sys::Win32::UI::Shell::{
    NIF_INFO, NIIF_NOSOUND, NIIF_RESPECT_QUIET_TIME, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};

/// Suffixes tried when looking a CLI up on PATH (npm installs `codex.cmd`).
pub const EXE_SUFFIXES: &[&str] = &[".exe", ".cmd"];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Run a console program without ever showing a console window. Its children inherit the hidden console.
pub fn hide_window(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

/// Kills the child and everything it spawned (e.g. cmd -> node -> codex.exe) when dropped.
pub struct ProcessGuard(HANDLE);

// SAFETY: a job handle is a plain kernel handle usable from any thread.
unsafe impl Send for ProcessGuard {}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Puts the child in a job that kills its whole tree when the guard drops. Errors if Windows refuses.
// ponytail: grandchildren spawned before assignment would escape; the provider CLIs take far longer than this to start.
pub fn contain(child: &Child) -> Result<ProcessGuard, String> {
    let err = |what| format!("{what} failed (error {})", unsafe { GetLastError() });
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return Err(err("CreateJobObject"));
        }
        let guard = ProcessGuard(job);
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err(err("SetInformationJobObject"));
        }
        if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
            return Err(err("AssignProcessToJobObject"));
        }
        Ok(guard)
    }
}

/// True if no other ClankShift tray is running in this session. The mutex lives until process exit.
pub fn single_instance() -> bool {
    let name = wide("Local\\ClankShift.Tray");
    unsafe {
        let h = CreateMutexW(null(), 0, name.as_ptr());
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ClankShift";

/// Login startup via the per-user Run key (no admin rights needed).
pub fn autostart_enabled() -> bool {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            null_mut(),
        ) == 0
    }
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let err = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let data = wide(&format!("\"{}\"", exe.display()));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        }
    } else {
        match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
            ERROR_FILE_NOT_FOUND => 0,
            e => e,
        }
    };
    if err == 0 {
        Ok(())
    } else {
        Err(format!("registry error {err}"))
    }
}

pub struct Accent {
    /// Accent shade for light mode, as RGB.
    pub light: [u8; 3],
    /// Accent shade for dark mode, as RGB.
    pub dark: [u8; 3],
}

/// The user's accent color, picked the way Windows 11 does it for buttons and toggles.
pub fn accent() -> Option<Accent> {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent");
    let value = wide("AccentPalette");
    let mut buf = [0u8; 32];
    let mut len = buf.len() as u32;
    let err = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    // 8 RGBA entries, lightest first: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, unused.
    // Windows 11 uses Dark1 in light mode and Light2 in dark mode.
    let rgb = |i: usize| [buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]];
    (err == 0 && len == 32).then(|| Accent {
        light: rgb(4),
        dark: rgb(1),
    })
}

/// Shows a Windows notification from the tray icon. Silent, and held back during Do Not Disturb.
/// Returns false if Windows refused it.
pub fn notify(tray: &tray_icon::TrayIcon, text: &str) -> bool {
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = tray.window_handle();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_NOSOUND | NIIF_RESPECT_QUIET_TIME;
        let n = nid.szInfo.len() - 1; // keep the terminating zero
        for (d, c) in nid.szInfo[..n].iter_mut().zip(text.encode_utf16()) {
            *d = c;
        }
        // tray-icon doesn't expose the icon's number (uID). The hidden window belongs to this one
        // icon only, so the first number Windows accepts is ours.
        (1..=8).any(|id| {
            nid.uID = id;
            Shell_NotifyIconW(NIM_MODIFY, &nid) != 0
        })
    }
}

pub fn open_folder(path: &std::path::Path) {
    let _ = Command::new("explorer").arg(path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_kills_tree_or_reports_failure() {
        // cmd -> ping, like the npm cmd -> node -> codex chain.
        let mut c = Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        let guard = contain(&c).unwrap();
        drop(guard);
        let t = std::time::Instant::now();
        c.wait().unwrap();
        assert!(t.elapsed().as_secs() < 5, "job close did not kill the tree");

        // An exited process can't be assigned; that must be an error, not a silent no-op guard.
        let e = contain(&c).err().unwrap();
        assert!(e.contains("AssignProcessToJobObject"), "{e}");
    }
}
