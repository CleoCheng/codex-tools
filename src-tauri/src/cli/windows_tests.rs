use super::*;

#[test]
fn lean_process_snapshot_retains_switch_identity_fields() {
    let full = sysinfo::System::new_all();
    let mut lean = sysinfo::System::new();
    refresh_windows_switch_processes(&mut lean);
    let pid = sysinfo::get_current_pid().unwrap();
    let expected = full.process(pid).unwrap();
    let actual = lean.process(pid).unwrap();
    assert!(actual.user_id().is_some());
    assert!(actual.exe().is_some());
    assert!(actual.session_id().is_some());
    assert!(actual.start_time() > 0);
    assert_eq!(actual.user_id(), expected.user_id());
    assert_eq!(actual.exe(), expected.exe());
    assert_eq!(actual.parent(), expected.parent());
    assert_eq!(actual.session_id(), expected.session_id());
    assert_eq!(actual.start_time(), expected.start_time());
    refresh_windows_switch_processes(&mut lean);
    let verified =
        verified_windows_desktop_process_ids(&lean, &[std::env::current_exe().unwrap()]).unwrap();
    assert!(verified.contains(&pid));
}

#[test]
fn registered_chatgpt_desktop_is_not_a_cli_or_regular_chatgpt() {
    let package = Path::new(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.901.5280.0_x64__2p2nqsd0c76g0\app",
    );
    assert!(is_windows_codex_app_file(&package.join("ChatGPT.exe")));
    assert!(is_windows_codex_app_file(&package.join("Codex.exe")));
    assert!(!is_codex_cli_file(&package.join("Codex.exe")));
    assert!(is_codex_cli_file(
        &package.join("resources").join("codex.exe")
    ));
    assert!(!is_windows_codex_app_file(
        &package.join("resources").join("codex.exe")
    ));
    assert!(!is_windows_codex_app_file(Path::new(
        r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin\version\codex.exe"
    )));
    assert!(!is_windows_codex_app_file(Path::new(
        r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_1_x64__publisher\app\ChatGPT.exe"
    )));
    assert!(!is_windows_codex_app_file(Path::new(
        r"C:\Tools\ChatGPT.exe"
    )));
    assert!(!is_windows_codex_app_file(Path::new(r"C:\Tools\codex.exe")));
}

#[test]
fn windows_paths_are_case_and_separator_insensitive() {
    assert!(windows_paths_equal(
        Path::new(r"C:\Apps\ChatGPT.exe"),
        Path::new("c:/apps/chatgpt.exe")
    ));
    assert!(!windows_paths_equal(
        Path::new(r"C:\Apps\ChatGPT.exe"),
        Path::new(r"C:\Other\ChatGPT.exe")
    ));
}

#[test]
fn desktop_descendants_leave_unrelated_cli_and_chat_clients_alone() {
    let pid = sysinfo::Pid::from_u32;
    let targets = windows_desktop_descendants(
        HashSet::from([pid(10)]),
        &[
            (pid(10), Some(pid(1))),
            (pid(11), Some(pid(10))),
            (pid(12), Some(pid(11))), // desktop-owned CLI
            (pid(20), Some(pid(1))),  // independent CLI
            (pid(21), Some(pid(20))),
            (pid(30), Some(pid(1))), // ordinary ChatGPT
        ],
    );
    assert_eq!(targets, HashSet::from([pid(10), pid(11), pid(12)]));
}

const STOP_TEST_DESKTOP: &str = r"C:\Program Files\WindowsApps\OpenAI.Codex_26_x64__publisher\app\ChatGPT.exe";
const STOP_TEST_RUNTIME: &str = r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin";
const STOP_TEST_SERVER: &str = r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin\2993767bd1dfb009\codex.exe";

fn stop_test_process(id: u32, parent: u32, path: Option<&str>, started_at: u64) -> WindowsSwitchProcess {
    WindowsSwitchProcess {
        pid: sysinfo::Pid::from_u32(id),
        parent: Some(sysinfo::Pid::from_u32(parent)),
        executable: path.map(PathBuf::from),
        started_at,
    }
}

fn stop_test_targets(processes: &[WindowsSwitchProcess], current: u32) -> HashSet<sysinfo::Pid> {
    windows_switch_stop_targets(
        HashSet::from([sysinfo::Pid::from_u32(10)]),
        processes,
        sysinfo::Pid::from_u32(current),
        Some(Path::new(STOP_TEST_RUNTIME)),
    )
}

#[test]
fn switching_tool_survives_when_launched_from_codex_terminal() {
    let targets = stop_test_targets(&[
        stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(11, 10, Some(STOP_TEST_DESKTOP), 11),
        stop_test_process(20, 11, Some(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"), 12),
        stop_test_process(100, 20, Some(r"C:\Apps\Codex Tools\app.exe"), 13),
        stop_test_process(101, 100, Some(r"C:\Apps\msedgewebview2.exe"), 14),
    ], 100);
    assert_eq!(targets, HashSet::from([sysinfo::Pid::from_u32(10), sysinfo::Pid::from_u32(11)]));
}

#[test]
fn restart_preserves_antigravity_studio_and_its_editor_even_when_codex_started_them() {
    let targets = stop_test_targets(&[
        stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(11, 10, Some(STOP_TEST_DESKTOP), 11),
        stop_test_process(12, 10, Some(STOP_TEST_SERVER), 12),
        stop_test_process(20, 10, Some(r"C:\Program Files\Antigravity Studio\Antigravity Studio.exe"), 13),
        stop_test_process(21, 20, Some(r"C:\Users\tester\AppData\Local\Programs\Antigravity\Antigravity.exe"), 14),
        stop_test_process(22, 21, Some(STOP_TEST_SERVER), 15),
    ], 100);
    assert_eq!(targets, [10, 11, 12].map(sysinfo::Pid::from_u32).into_iter().collect());
}

#[test]
fn restart_never_follows_a_terminal_branch_into_external_apps_or_other_codex_sessions() {
    let targets = stop_test_targets(&[
        stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(20, 10, Some(r"C:\Windows\System32\cmd.exe"), 11),
        stop_test_process(21, 20, Some(r"C:\Apps\Antigravity Studio.exe"), 12),
        stop_test_process(22, 21, Some(STOP_TEST_SERVER), 13),
        stop_test_process(30, 10, Some(r"C:\Apps\Antigravity.exe"), 14),
        stop_test_process(31, 30, Some(STOP_TEST_SERVER), 15),
    ], 100);
    assert_eq!(targets, HashSet::from([sysinfo::Pid::from_u32(10)]));
}

#[test]
fn restart_preserves_unknown_executables_and_unreadable_process_paths() {
    let mut processes = vec![stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10)];
    for (index, path) in [
        Some(r"C:\Tools\codex.exe"),
        Some(r"C:\Tools\node.exe"),
        Some(r"C:\Apps\OpenCode.exe"),
        Some(r"C:\Apps\Cursor.exe"),
        Some(r"C:\Apps\Code.exe"),
        Some(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"),
        Some(r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_1_x64__publisher\app\ChatGPT.exe"),
        None,
    ].into_iter().enumerate() {
        processes.push(stop_test_process(20 + index as u32, 10, path, 11));
    }
    assert_eq!(stop_test_targets(&processes, 100), HashSet::from([sysinfo::Pid::from_u32(10)]));
}

#[test]
fn restart_still_stops_codex_renderers_and_installed_or_bundled_app_servers() {
    let targets = stop_test_targets(&[
        stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(11, 10, Some(STOP_TEST_DESKTOP), 11),
        stop_test_process(12, 10, Some(STOP_TEST_SERVER), 12),
        stop_test_process(13, 12, Some(STOP_TEST_SERVER), 13),
        stop_test_process(14, 10, Some(r"C:\Program Files\WindowsApps\OpenAI.Codex_26_x64__publisher\app\resources\codex.exe"), 14),
    ], 100);
    assert_eq!(targets, [10, 11, 12, 13, 14].map(sysinfo::Pid::from_u32).into_iter().collect());
}

#[test]
fn restart_does_not_target_an_independent_codex_cli_with_the_same_executable() {
    let targets = stop_test_targets(&[
        stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(12, 10, Some(STOP_TEST_SERVER), 12),
        stop_test_process(20, 1, Some(STOP_TEST_SERVER), 11),
        stop_test_process(21, 20, Some(STOP_TEST_SERVER), 13),
    ], 100);
    assert_eq!(targets, [10, 12].map(sysinfo::Pid::from_u32).into_iter().collect());
}

#[test]
fn restart_rejects_recycled_or_unverifiable_parent_process_identity() {
    for root_started_at in [0, 50] {
        let targets = stop_test_targets(&[
            stop_test_process(10, 1, Some(STOP_TEST_DESKTOP), root_started_at),
            stop_test_process(20, 10, Some(STOP_TEST_SERVER), 40),
            stop_test_process(21, 20, Some(STOP_TEST_SERVER), 41),
        ], 100);
        assert_eq!(targets, HashSet::from([sysinfo::Pid::from_u32(10)]));
    }
}

#[test]
fn codex_started_by_tools_still_restarts_without_targeting_tools() {
    let targets = stop_test_targets(&[
        stop_test_process(100, 1, Some(r"C:\Apps\Codex Tools\app.exe"), 9),
        stop_test_process(10, 100, Some(STOP_TEST_DESKTOP), 10),
        stop_test_process(11, 10, Some(STOP_TEST_DESKTOP), 11),
    ], 100);
    assert_eq!(targets, [10, 11].map(sysinfo::Pid::from_u32).into_iter().collect());
}

#[test]
fn component_identity_requires_a_known_installation_path_and_handles_case_or_slashes() {
    let desktops = vec![PathBuf::from(STOP_TEST_DESKTOP)];
    for path in [
        STOP_TEST_SERVER,
        "c:/users/tester/appdata/local/openai/codex/bin/2993767BD1DFB009/CODEX.EXE",
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26_x64__publisher\app\resources\app.asar.unpacked\vendor\codex.exe",
    ] {
        assert!(is_windows_codex_stop_component(Path::new(path), &desktops, Some(Path::new(STOP_TEST_RUNTIME))), "{path}");
    }
    for path in [
        r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin-foreign\version\codex.exe",
        r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin\version\foreign\codex.exe",
        r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin\..\codex.exe",
        r"C:\Users\tester\AppData\Local\OpenAI\Codex\bin\version\Antigravity.exe",
        r"C:\Apps\Antigravity\resources\codex.exe",
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26_x64__publisher\app\resources\..\foreign\codex.exe",
    ] {
        assert!(!is_windows_codex_stop_component(Path::new(path), &desktops, Some(Path::new(STOP_TEST_RUNTIME))), "{path}");
    }
}

#[test]
fn missing_launch_targets_fail_before_switch_with_both_causes() {
    let plan = WindowsCodexLaunchPlan {
        desktop: None,
        selected_desktop_path: None,
        fallback_cli: Err("CLI not found".to_string()),
        discovery_error: Some("Store query failed".to_string()),
        as_admin: false,
    };
    let error = plan.validate().unwrap_err();
    assert!(error.contains("未停止应用或更改当前账号"));
    assert!(error.contains("Store query failed"));
    assert!(error.contains("CLI not found"));
}

#[test]
fn desktop_does_not_require_a_separate_cli_installation() {
    let plan = WindowsCodexLaunchPlan {
        desktop: Some(WindowsDesktopTarget::Store(WindowsStoreCodexTarget {
            aumid: "OpenAI.Codex_publisher!App".to_string(),
            executable: PathBuf::from(r"C:\package\app\ChatGPT.exe"),
        })),
        selected_desktop_path: None,
        fallback_cli: Err("CLI not found".to_string()),
        discovery_error: None,
        as_admin: false,
    };
    assert!(plan.validate().is_ok());
    let error = windows_launch_failure(Some("AUMID activation failed HRESULT"), "CLI not found");
    assert!(error.contains("AUMID activation failed HRESULT"));
    assert!(error.contains("CLI not found"));
}

#[test]
#[ignore = "read-only installed Windows desktop diagnostic; run explicitly on a real host"]
fn installed_windows_desktop_diagnostic() {
    let plan = prepare_windows_codex_launch(None, false).expect("resolve installed desktop");
    let target = plan.desktop.as_ref().expect("installed desktop target");
    let system = sysinfo::System::new_all();
    let pids = verified_windows_desktop_process_ids(&system, &[target.executable().to_path_buf()])
        .expect("read current-user desktop process identity");
    println!(
        "desktop={target:?}; verified_pids={pids:?}; standalone_cli_available={}",
        plan.fallback_cli.is_ok()
    );
    assert!(target.executable().is_file());
    // Intentionally do not activate, stop, or switch anything in this diagnostic.
}

#[test]
fn switching_launch_installation_still_selects_running_store_desktop() {
    let old = PathBuf::from(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.830_x64__publisher\app\Codex.exe",
    );
    let new = PathBuf::from(
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.901_x64__publisher\app\ChatGPT.exe",
    );
    // The old algorithm matched only the newly selected launch path.
    assert!(!windows_paths_equal(&old, &new));
    assert!(desktop_path_requires_stop(&old, &[new.clone()]));
    assert!(desktop_path_requires_stop(&new, &[new.clone()]));
    // CLI fallback must also stop a running desktop when discovery found no target.
    assert!(desktop_path_requires_stop(&old, &[]));
    for unrelated in [
        r"C:\Program Files\WindowsApps\OpenAI.ChatGPT_1_x64__publisher\app\ChatGPT.exe",
        r"C:\Program Files\WindowsApps\OpenAI.Codex_26.830_x64__publisher\app\resources\codex.exe",
        r"C:\Tools\codex.exe",
    ] {
        assert!(!desktop_path_requires_stop(
            Path::new(unrelated),
            &[new.clone()]
        ));
    }
}
