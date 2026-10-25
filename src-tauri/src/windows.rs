//! Native Windows entry points; child processes retain an explicit environment allowlist.
use crate::types::{CoreError, Result};
use std::{os::windows::ffi::OsStrExt, path::Path, process::Command};

pub(crate) fn prepare_child(command: &mut Command) {
    let mut paths = Vec::new();
    if let Some(root) = std::env::var_os("SystemRoot") {
        paths.push(std::path::PathBuf::from(&root).join("System32"));
        command.env("SystemRoot", &root).env("WINDIR", root);
    }
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        paths.push(std::path::PathBuf::from(programs).join("Git/cmd"));
    }
    command.env("PATH", std::env::join_paths(paths).unwrap_or_default());
    for name in ["TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    // Keep the GUI from flashing a console for each model or checkpoint operation.
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
}

pub(crate) fn open(path: &Path, reveal: bool) -> Result<()> {
    use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};
    let target = if reveal { path.parent().unwrap_or(path) } else { path };
    let target: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // URLs are HTTPS-validated and reveal paths are course-resolved by the caller.
    let result = unsafe {
        ShellExecuteW(std::ptr::null_mut(), std::ptr::null(), target.as_ptr(),
            std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL)
    };
    if result as isize > 32 { Ok(()) }
    else { Err(CoreError::new("IO", "未能打开，请复制链接或路径后重试。")) }
}
