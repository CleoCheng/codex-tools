//! Handle-based desktop shutdown. Keep handles open across revalidation so a
//! recycled PID can never redirect a termination request to another process.
use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};
use windows::{
    core::{BOOL, PWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM},
        System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
            PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
        UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
        },
    },
};

pub(crate) struct ProcessHandle {
    pub(crate) pid: sysinfo::Pid,
    handle: HANDLE,
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
impl ProcessHandle {
    pub(crate) fn open(pid: sysinfo::Pid) -> Result<Self, String> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
                false,
                pid.as_u32(),
            )
        }
        .map_err(|e| format!("打开已选进程句柄失败: {e}"))?;
        Ok(Self { pid, handle })
    }
    pub(crate) fn exited(&self) -> Result<bool, String> {
        match unsafe { WaitForSingleObject(self.handle, 0) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => Ok(false),
            _ => Err("无法确认进程退出，未更改当前账号。".into()),
        }
    }
    pub(crate) fn executable(&self) -> Result<PathBuf, String> {
        let mut buffer = vec![0u16; 32768];
        let mut len = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                self.handle,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut len,
            )
        }
        .map_err(|e| e.to_string())?;
        Ok(PathBuf::from(String::from_utf16_lossy(
            &buffer[..len as usize],
        )))
    }
    fn terminate(&self) -> Result<(), String> {
        if self.exited()? {
            return Ok(());
        }
        log::info!("CODEX_DESKTOP_STOP action=terminate pid={} executable={:?}", self.pid, self.executable().ok());
        let result = unsafe { TerminateProcess(self.handle, 1) };
        if result.is_err() && !self.exited()? {
            return Err("已验证进程终止失败，未更改当前账号。".into());
        }
        Ok(())
    }
}

struct WindowQuery {
    pids: HashSet<u32>,
    windows: Vec<HWND>,
}
unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
    let query = &mut *(data.0 as *mut WindowQuery);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if query.pids.contains(&pid) && IsWindowVisible(hwnd).as_bool() {
        query.windows.push(hwnd);
    }
    BOOL(1)
}
fn windows(pids: impl Iterator<Item = sysinfo::Pid>) -> Result<Vec<HWND>, String> {
    let mut query = WindowQuery {
        pids: pids.map(|p| p.as_u32()).collect(),
        windows: vec![],
    };
    unsafe { EnumWindows(Some(collect_window), LPARAM(&mut query as *mut _ as isize)) }
        .map_err(|e| e.to_string())?;
    Ok(query.windows)
}
fn all_exited(handles: &[ProcessHandle]) -> Result<bool, String> {
    for handle in handles {
        if !handle.exited()? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn wait_exited(handles: &[ProcessHandle], timeout: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if all_exited(handles)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
pub(crate) fn stop(handles: &[ProcessHandle], roots: &HashSet<sysinfo::Pid>) -> Result<(), String> {
    if handles.is_empty() {
        return Ok(());
    }
    let graceful = {
        let _timing = crate::switch_timing::Phase::start("desktop_graceful_close");
        for hwnd in windows(
            handles
                .iter()
                .filter(|h| roots.contains(&h.pid))
                .map(|h| h.pid),
        )? {
            // A successful WM_CLOSE post can merely hide a window. Only a
            // signaled process handle proves that the process exited.
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
            }
            if handles
                .iter()
                .any(|h| h.pid.as_u32() == pid && roots.contains(&h.pid))
            {
                log::info!("CODEX_DESKTOP_STOP action=request_close pid={pid}");
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        wait_exited(handles, Duration::from_millis(750))?
    };
    if !graceful {
        let _timing = crate::switch_timing::Phase::start("desktop_force_stop_and_wait");
        // Stop desktop roots first to prevent them launching further children.
        for handle in handles.iter().filter(|h| roots.contains(&h.pid)) {
            handle.terminate()?;
        }
        for handle in handles.iter().filter(|h| !roots.contains(&h.pid)) {
            handle.terminate()?;
        }
        if !wait_exited(handles, Duration::from_secs(3))? {
            return Err("等待桌面进程退出超时，未更改当前账号。".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct ChildGuard(std::process::Child);
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn sleeper() -> ChildGuard {
        ChildGuard(
            crate::utils::new_background_command("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "Start-Sleep -Seconds 30",
                ])
                .spawn()
                .unwrap(),
        )
    }
    #[test]
    fn native_stop_waits_for_disposable_process_and_preserves_other_child() {
        let mut target = sleeper();
        let mut unrelated = sleeper();
        let handle = ProcessHandle::open(sysinfo::Pid::from_u32(target.0.id())).unwrap();
        assert!(handle.executable().unwrap().ends_with("powershell.exe"));
        assert!(!handle.exited().unwrap());
        let handles = vec![handle];
        stop(&handles, &HashSet::new()).unwrap();
        assert!(all_exited(&handles).unwrap());
        // Repeating a stop on an already signaled handle is harmless.
        stop(&handles, &HashSet::new()).unwrap();
        target.0.wait().unwrap();
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }
}
