//! Stop verified Codex components, preserving applications started from its tasks.
use super::*;

fn log_stop_plan(
    roots: &HashSet<sysinfo::Pid>,
    targets: &HashSet<sysinfo::Pid>,
    processes: &[WindowsSwitchProcess],
) {
    let parents = processes.iter().map(|process| (process.pid, process.parent)).collect::<Vec<_>>();
    let descendants = windows_desktop_descendants(roots.clone(), &parents);
    log::info!("CODEX_DESKTOP_STOP action=plan roots={} selected={} preserved={}", roots.len(), targets.len(), descendants.difference(targets).count());
    for process in processes.iter().filter(|process| descendants.contains(&process.pid)) {
        let action = if targets.contains(&process.pid) { "select" } else { "preserve_non_codex" };
        log::info!("CODEX_DESKTOP_STOP action={action} pid={} parent={:?} executable={:?}", process.pid, process.parent, process.executable);
    }
}

pub(super) fn stop_desktop(executable: Option<&Path>) -> Result<(), String> {
    let _timing = crate::switch_timing::Phase::start("desktop_stop");
    let expected = executable.map(Path::to_path_buf).into_iter().collect::<Vec<_>>();
    let runtime_root = env::var_os("LOCALAPPDATA").map(PathBuf::from).map(|path| path.join("OpenAI").join("Codex").join("bin"));
    let current_pid = sysinfo::get_current_pid().map_err(|error| error.to_string())?;
    let mut system = sysinfo::System::new();
    refresh_windows_switch_processes(&mut system);
    let roots = running_windows_desktop_process_ids(&system, &expected)?;
    let processes = windows_switch_process_snapshot(&system)?;
    let targets = windows_switch_stop_targets(roots.clone(), &processes, current_pid, runtime_root.as_deref());
    log_stop_plan(&roots, &targets, &processes);
    // Hold process handles across authoritative path/ancestry revalidation.
    let mut handles = Vec::new();
    for pid in targets {
        match crate::windows_desktop_lifecycle::ProcessHandle::open(pid) {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                refresh_windows_switch_processes(&mut system);
                if system.process(pid).is_some() { return Err(error); }
            }
        }
    }
    system = sysinfo::System::new();
    refresh_windows_switch_processes(&mut system);
    let roots = running_windows_desktop_process_ids(&system, &expected)?;
    let verified_processes = windows_switch_process_snapshot(&system)?;
    let verified = windows_switch_stop_targets(roots.clone(), &verified_processes, current_pid, runtime_root.as_deref());
    // Do not close Codex if its teardown can also end this controller. This
    // check runs before WM_CLOSE or any termination and never relaunches Tools.
    let job_members = crate::windows_desktop_lifecycle::current_job_process_ids()?;
    log::info!("CODEX_DESKTOP_STOP controller_pid={current_pid} controller_job_members={job_members:?}");
    if let Err(error) = validate_windows_controller_independence(&verified, &verified_processes, current_pid, &job_members) {
        log::warn!("CODEX_DESKTOP_STOP action=blocked_controller_dependency error={error}");
        return Err(error);
    }
    for handle in &handles {
        if handle.exited()? { continue; }
        let process = system.process(handle.pid).ok_or("进程核验状态变化，未更改当前账号。")?;
        if !verified.contains(&handle.pid)
            || !process.exe().is_some_and(|path| handle.executable().is_ok_and(|actual| windows_paths_equal(path, &actual)))
        {
            return Err("进程身份或归属发生变化，未更改当前账号。".into());
        }
    }
    if verified.iter().any(|pid| !handles.iter().any(|handle| handle.pid == *pid)) {
        return Err("停止前出现新的 Codex 组件，未更改当前账号，请重试。".into());
    }
    crate::windows_desktop_lifecycle::stop(&handles, &roots)?;
    refresh_windows_switch_processes(&mut system);
    let mut survivors = windows_switch_process_snapshot(&system)?;
    // Exited parents retain pinned handles. Preserve their verified identities
    // to recognize a Codex helper spawned during shutdown, without following
    // unrelated application branches or confusing recycled parent PIDs.
    for process in verified_processes.iter().filter(|process| verified.contains(&process.pid)) {
        if !survivors.iter().any(|survivor| survivor.pid == process.pid) {
            survivors.push(process.clone());
        }
    }
    let remaining = windows_switch_stop_targets(roots, &survivors, current_pid, runtime_root.as_deref());
    if remaining.iter().any(|pid| system.process(*pid).is_some()) {
        return Err("停止期间出现残留 Codex 组件，未更改当前账号，请重试。".into());
    }
    if !running_windows_desktop_process_ids(&system, &expected)?.is_empty() {
        return Err("ChatGPT/Codex 在停止期间重新启动，未更改当前账号，请关闭该桌面后重试。".into());
    }
    log::info!("CODEX_DESKTOP_STOP action=complete");
    // Leave writer-lock ownership to Codex; other clients may still be active.
    Ok(())
}
