//! Windows specifics: hidden child processes, process-tree cleanup, single instance, login startup.

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
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, CreateMutexW};

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
        if !self.0.is_null() {
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ponytail: grandchildren spawned before assignment would escape; the provider CLIs take far longer than this to start.
pub fn contain(child: &Child) -> ProcessGuard {
    unsafe {
        let job = CreateJobObjectW(null(), null());
        if job.is_null() {
            return ProcessGuard(job);
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE);
        ProcessGuard(job)
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

pub fn open_folder(path: &std::path::Path) {
    let _ = Command::new("explorer").arg(path).spawn();
}
