//! Process identity snapshots and desktop launch readiness.
use super::*;

#[cfg(target_os = "windows")]
pub(super) fn windows_desktop_descendants(
    mut targets: HashSet<sysinfo::Pid>,
    same_user_parents: &[(sysinfo::Pid, Option<sysinfo::Pid>)],
) -> HashSet<sysinfo::Pid> {
    loop {
        let previous = targets.len();
        for (pid, parent) in same_user_parents {
            if parent.is_some_and(|parent| targets.contains(&parent)) {
                targets.insert(*pid);
            }
        }
        if targets.len() == previous {
            return targets;
        }
    }
}

// Keep identity/path data required by the switch gate, without collecting CPU,
// memory, I/O, environment or command lines on every process poll.
#[cfg(target_os = "windows")]
pub(super) fn refresh_windows_switch_processes(system: &mut sysinfo::System) {
    system.refresh_processes_specifics(
        sysinfo::ProcessRefreshKind::new()
            .with_user(sysinfo::UpdateKind::Always)
            .with_exe(sysinfo::UpdateKind::Always),
    );
}

#[cfg(target_os = "windows")]
pub(super) fn wait_for_windows_codex_process(executable: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_millis(WINDOWS_STORE_LAUNCH_TIMEOUT_MS);
    let mut confirmed_since = None;
    let mut system = sysinfo::System::new();
    loop {
        refresh_windows_switch_processes(&mut system);
        let pids = verified_windows_desktop_process_ids(&system, &[executable.to_path_buf()])
            .unwrap_or_default();
        // Activation can return a broker PID. Require the installed GUI path to
        // stay alive instead of accepting any PID or a matching process name.
        if !pids.is_empty() {
            let start = confirmed_since.get_or_insert_with(Instant::now);
            if start.elapsed() >= Duration::from_millis(500) {
                log::info!("CODEX_DESKTOP_LAUNCH verified_pids={pids:?}");
                return true;
            }
        } else {
            confirmed_since = None;
        }

        if Instant::now() >= deadline {
            return false;
        }

        thread::sleep(Duration::from_millis(WINDOWS_STORE_LAUNCH_POLL_MS));
    }
}

#[cfg(target_os = "windows")]
pub(super) fn verified_windows_desktop_process_ids(
    system: &sysinfo::System,
    executables: &[PathBuf],
) -> Result<HashSet<sysinfo::Pid>, String> {
    current_session_process_ids(system, |path| {
        executables
            .iter()
            .any(|expected| windows_paths_equal(path, expected))
    })
}

/// Shutdown covers the currently running desktops, even if Store updated its
/// registered version or the user selected a different installation to launch.
pub(super) fn running_windows_desktop_process_ids(
    system: &sysinfo::System,
    expected: &[PathBuf],
) -> Result<HashSet<sysinfo::Pid>, String> {
    current_session_process_ids(system, |path| desktop_path_requires_stop(path, expected))
}

pub(super) fn desktop_path_requires_stop(path: &Path, expected: &[PathBuf]) -> bool {
    is_windows_codex_app_file(path)
        || expected
            .iter()
            .any(|target| windows_paths_equal(path, target))
}

fn current_session_process_ids(
    system: &sysinfo::System,
    accepts_path: impl Fn(&Path) -> bool,
) -> Result<HashSet<sysinfo::Pid>, String> {
    let current_pid = sysinfo::get_current_pid().map_err(|error| error.to_string())?;
    let current = system
        .process(current_pid)
        .ok_or_else(|| "无法识别当前进程，未操作桌面进程。".to_string())?;
    let user = current
        .user_id()
        .ok_or_else(|| "无法识别当前用户，未操作桌面进程。".to_string())?;
    let session = current
        .session_id()
        .ok_or_else(|| "无法识别当前会话，未操作桌面进程。".to_string())?;
    Ok(system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            (process.user_id() == Some(user)
                && process.session_id() == Some(session)
                && process.exe().is_some_and(&accepts_path))
            .then_some(*pid)
        })
        .collect())
}

#[cfg(target_os = "windows")]
pub(super) fn windows_paths_equal(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\"))
}

#[derive(Clone, Debug)]
pub(super) struct WindowsSwitchProcess {
    pub(super) pid: sysinfo::Pid,
    pub(super) parent: Option<sysinfo::Pid>,
    pub(super) executable: Option<PathBuf>,
    pub(super) started_at: u64,
}

pub(super) fn windows_switch_process_snapshot(
    system: &sysinfo::System,
) -> Result<Vec<WindowsSwitchProcess>, String> {
    let pid = sysinfo::get_current_pid().map_err(|error| error.to_string())?;
    let current = system.process(pid).ok_or("无法识别当前进程，未停止应用。")?;
    let user = current.user_id().ok_or("无法识别当前用户，未停止应用。")?;
    let session = current.session_id().ok_or("无法识别当前会话，未停止应用。")?;
    Ok(system
        .processes()
        .iter()
        .filter(|(_, process)| process.user_id() == Some(user) && process.session_id() == Some(session))
        .map(|(pid, process)| WindowsSwitchProcess {
            pid: *pid,
            parent: process.parent(),
            executable: process.exe().map(Path::to_path_buf),
            started_at: process.start_time(),
        })
        .collect())
}

// A child relationship is not application ownership: a terminal can start an
// independent IDE, browser, or server. Only recognized Codex components qualify.
pub(super) fn is_windows_codex_stop_component(
    path: &Path,
    desktops: &[PathBuf],
    runtime_root: Option<&Path>,
) -> bool {
    let normalize = |path: &Path| path.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    let path = normalize(path);
    if path.split('\\').any(|part| part == "." || part == "..") {
        return false;
    }
    for desktop in desktops {
        if path == normalize(desktop) {
            return true;
        }
        if let Some(directory) = desktop.parent() {
            let prefix = format!("{}\\", normalize(&directory.join("resources")));
            if path.starts_with(&prefix) && path.ends_with("\\codex.exe") {
                return true;
            }
        }
    }
    runtime_root.is_some_and(|root| {
        let prefix = format!("{}\\", normalize(root));
        let Some(relative) = path.strip_prefix(&prefix) else { return false };
        let parts = relative.split('\\').collect::<Vec<_>>();
        parts.len() == 2 && !parts[0].is_empty() && parts[1] == "codex.exe"
    })
}

pub(super) fn windows_switch_stop_targets(
    roots: HashSet<sysinfo::Pid>,
    processes: &[WindowsSwitchProcess],
    current_pid: sysinfo::Pid,
    runtime_root: Option<&Path>,
) -> HashSet<sysinfo::Pid> {
    let records = processes.iter().map(|process| (process.pid, process)).collect::<std::collections::HashMap<_, _>>();
    let desktops = processes.iter()
        .filter(|process| roots.contains(&process.pid))
        .filter_map(|process| process.executable.clone())
        .collect::<Vec<_>>();
    let mut targets = roots;
    targets.remove(&current_pid);
    loop {
        let previous = targets.len();
        for process in processes {
            if process.pid == current_pid || targets.contains(&process.pid) {
                continue;
            }
            let Some(parent) = process.parent.and_then(|pid| records.get(&pid)) else { continue };
            if targets.contains(&parent.pid)
                && parent.started_at > 0
                && process.started_at >= parent.started_at
                && process.executable.as_deref().is_some_and(|path| {
                    is_windows_codex_stop_component(path, &desktops, runtime_root)
                })
            {
                targets.insert(process.pid);
            }
        }
        if targets.len() == previous {
            return targets;
        }
    }
}
