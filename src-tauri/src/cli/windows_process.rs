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

pub(super) fn desktop_path_requires_stop(path: &Path, _expected: &[PathBuf]) -> bool {
    // A configured path must not grant shutdown authority over an arbitrary app.
    is_windows_codex_app_file(path)
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
    let roots = roots.into_iter().filter(|pid| {
        records.get(pid).and_then(|process| process.executable.as_deref())
            .is_some_and(|path| desktop_path_requires_stop(path, &[]))
    }).collect::<HashSet<_>>();
    let protected = windows_controller_owned_processes(&roots, processes, current_pid);
    let mut targets = roots.difference(&protected).copied().collect::<HashSet<_>>();
    loop {
        let previous = targets.len();
        for process in processes {
            if protected.contains(&process.pid) || targets.contains(&process.pid) {
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

// Protect the controller and its UI/helpers as a branch. A separately verified
// Codex desktop launched by Tools remains restartable outside that branch.
fn windows_controller_owned_processes(
    roots: &HashSet<sysinfo::Pid>,
    processes: &[WindowsSwitchProcess],
    current_pid: sysinfo::Pid,
) -> HashSet<sysinfo::Pid> {
    let controller = processes.iter().find(|process| process.pid == current_pid);
    let mut protected = HashSet::from([current_pid]);
    for process in processes {
        if controller.and_then(|process| process.executable.as_deref())
            .zip(process.executable.as_deref())
            .is_some_and(|(left, right)| windows_paths_equal(left, right))
        {
            protected.insert(process.pid);
        }
    }
    loop {
        let before = protected.len();
        for process in processes {
            if !roots.contains(&process.pid)
                && process.parent.is_some_and(|parent| protected.contains(&parent))
            {
                protected.insert(process.pid);
            }
        }
        if protected.len() == before { return protected; }
    }
}

pub(super) fn validate_windows_controller_independence(
    targets: &HashSet<sysinfo::Pid>,
    processes: &[WindowsSwitchProcess],
    current_pid: sysinfo::Pid,
    job_members: &HashSet<sysinfo::Pid>,
) -> Result<(), String> {
    let blocked = || "为保持 Codex Tools 和其他应用运行，本次未停止 Codex 或切换账号：工具与 Codex 存在启动或进程作业依赖。请从开始菜单或资源管理器独立启动 Codex Tools。".to_string();
    if targets.contains(&current_pid) || !targets.is_disjoint(job_members) {
        return Err(blocked());
    }
    let records = processes.iter().map(|process| (process.pid, process)).collect::<std::collections::HashMap<_, _>>();
    let mut cursor = current_pid;
    let mut seen = HashSet::new();
    while seen.insert(cursor) {
        let Some(process) = records.get(&cursor) else { break };
        let Some(parent) = process.parent.and_then(|pid| records.get(&pid)) else { break };
        if parent.started_at == 0 || parent.started_at > process.started_at { break; }
        if targets.contains(&parent.pid) { return Err(blocked()); }
        cursor = parent.pid;
    }
    Ok(())
}
