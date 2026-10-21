// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "windows")]
    configure_webview2_proxy_bypass();

    if app_lib::try_run_cli_from_env() {
        return;
    }

    app_lib::run();
}

#[cfg(target_os = "windows")]
fn configure_webview2_proxy_bypass() {
    const BYPASS_ARG: &str = "--proxy-bypass-list=<-loopback>;*.localhost;tauri.localhost;127.0.0.1";
    match std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS") {
        Ok(existing) => {
            if !existing.contains("proxy-bypass-list") {
                std::env::set_var(
                    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                    format!("{existing} {BYPASS_ARG}"),
                );
            }
        }
        Err(_) => {
            std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", BYPASS_ARG);
        }
    }
}
