//! 额度耗尽后的自动切换，以及切换成功后的任务续跑。
//!
//! 触发点：每次首页/周期额度刷新完成后先做快速判断，命中再后台执行 `run_auto_switch`。
//! 条件：当前账号任一窗口已用满 100%，且账号池里存在该窗口仍有剩余的账号。
//! 续跑：切换成功后，通过 app-server 读取 Codex App 会话状态，
//! 把时间窗口内最后一个 turn 未完成（中断/失败/进行中）的会话逐条续跑。
//! 判定细节见 `auto_switch_continue` 模块。


use serde::Serialize;
use tauri::AppHandle;
use tauri::Emitter;

use crate::auto_switch_continue;
use crate::models::AccountSourceKind;
use crate::models::AccountSummary;
use crate::state::AppState;
use crate::store;
use crate::utils::now_unix_seconds;

pub(crate) const AUTO_SWITCH_STATUS_EVENT: &str = "auto-switch-status";
const SWITCH_COOLDOWN_SECONDS: i64 = 10 * 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AutoSwitchStatus {
    /// idle | checking | switching | switched | continuing | continued | skipped | failed
    pub(crate) stage: String,
    pub(crate) message: String,
    pub(crate) from_account_id: Option<String>,
    pub(crate) from_account_label: Option<String>,
    pub(crate) to_account_id: Option<String>,
    pub(crate) to_account_label: Option<String>,
    pub(crate) thread_id: Option<String>,
    pub(crate) thread_name: Option<String>,
    pub(crate) continued: bool,
    pub(crate) used_headless_fallback: bool,
    pub(crate) updated_at: i64,
}

impl Default for AutoSwitchStatus {
    fn default() -> Self {
        Self {
            stage: "idle".to_string(),
            message: String::new(),
            from_account_id: None,
            from_account_label: None,
            to_account_id: None,
            to_account_label: None,
            thread_id: None,
            thread_name: None,
            continued: false,
            used_headless_fallback: false,
            updated_at: 0,
        }
    }
}

#[derive(Default)]
pub(crate) struct AutoSwitchRuntime {
    pub(crate) in_flight: bool,
    pub(crate) last_trigger_account_key: Option<String>,
    pub(crate) last_trigger_at: i64,
    pub(crate) status: AutoSwitchStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowKind {
    FiveHour,
    OneWeek,
}

impl WindowKind {
    fn label(self) -> &'static str {
        match self {
            Self::FiveHour => "5h",
            Self::OneWeek => "1week",
        }
    }

    fn remaining(self, account: &AccountSummary) -> Option<f64> {
        let window = match self {
            Self::FiveHour => account.usage.as_ref()?.five_hour.as_ref(),
            Self::OneWeek => account.usage.as_ref()?.one_week.as_ref(),
        }?;
        if !window.used_percent.is_finite() {
            return None;
        }
        Some((100.0 - window.used_percent).clamp(0.0, 100.0))
    }
}

fn account_has_blocking_issue(account: &AccountSummary) -> bool {
    account.auth_refresh_blocked
        || account.profile_integrity_error.is_some()
        || account.profile_last_validation_error.is_some()
        || account.auth_refresh_error.is_some()
}

/// 默认 100%，即任一窗口用满才视为耗尽。
/// 可用 `CODEX_TOOLS_AUTO_SWITCH_THRESHOLD` 调低阈值，便于提前切换或验证链路。
fn exhaustion_threshold() -> f64 {
    std::env::var("CODEX_TOOLS_AUTO_SWITCH_THRESHOLD")
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
        .unwrap_or(100.0)
}

/// 只计算并展示将要执行的动作，不真正切换账号；用于确认配置是否生效。
fn dry_run_enabled() -> bool {
    std::env::var("CODEX_TOOLS_AUTO_SWITCH_DRY_RUN")
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            value == "1" || value == "true" || value == "yes"
        })
        .unwrap_or(false)
}

fn exhausted_window(account: &AccountSummary) -> Option<WindowKind> {
    let threshold = exhaustion_threshold();
    let usage = account.usage.as_ref()?;
    let five = usage
        .five_hour
        .as_ref()
        .map(|window| window.used_percent)
        .filter(|value| value.is_finite());
    let week = usage
        .one_week
        .as_ref()
        .map(|window| window.used_percent)
        .filter(|value| value.is_finite());
    match (five, week) {
        // 两个窗口同时耗尽时，优先按周额度判断候选可用性。
        (Some(five), Some(week)) if five >= threshold && week >= threshold => {
            Some(WindowKind::OneWeek)
        }
        (Some(five), _) if five >= threshold => Some(WindowKind::FiveHour),
        (_, Some(week)) if week >= threshold => Some(WindowKind::OneWeek),
        _ => None,
    }
}

fn remaining_score(value: Option<f64>) -> f64 {
    value.unwrap_or(-1.0)
}

fn pick_switch_candidate(
    summaries: &[AccountSummary],
    exhausted: WindowKind,
    current_id: &str,
) -> Option<AccountSummary> {
    let mut candidates = summaries
        .iter()
        .filter(|account| account.id != current_id)
        .filter(|account| matches!(account.source_kind, AccountSourceKind::Chatgpt))
        .filter(|account| !account_has_blocking_issue(account))
        .filter(|account| {
            remaining_score(exhausted.remaining(account)).gt(&0.0)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        let left_week = remaining_score(WindowKind::OneWeek.remaining(left));
        let right_week = remaining_score(WindowKind::OneWeek.remaining(right));
        let left_five = remaining_score(WindowKind::FiveHour.remaining(left));
        let right_five = remaining_score(WindowKind::FiveHour.remaining(right));
        right_week
            .total_cmp(&left_week)
            .then(right_five.total_cmp(&left_five))
            .then_with(|| left.label.cmp(&right.label))
    });
    candidates.into_iter().next()
}

async fn update_status(
    app: &AppHandle,
    state: &AppState,
    mutate: impl FnOnce(&mut AutoSwitchStatus),
) -> AutoSwitchStatus {
    let status = {
        let mut runtime = state.auto_switch.lock().await;
        mutate(&mut runtime.status);
        runtime.status.updated_at = now_unix_seconds();
        runtime.status.clone()
    };
    if let Err(error) = app.emit(AUTO_SWITCH_STATUS_EVENT, &status) {
        log::warn!("发送自动切换状态事件失败: {error}");
    }
    status
}

pub(crate) async fn current_status(state: &AppState) -> AutoSwitchStatus {
    state.auto_switch.lock().await.status.clone()
}

/// 每次额度刷新后调用；只有开启自动切换且当前账号耗尽时才真正执行。
/// 额度刷新路径只做一次快速判断，真正切换放到后台任务里，
/// 避免阻塞 `refresh_all_usage` 的返回和首页刷新。
pub(crate) async fn should_check_auto_switch(
    app: &AppHandle,
    state: &AppState,
    summaries: &[AccountSummary],
) -> bool {
    let enabled = {
        let _guard = state.store_lock.lock().await;
        store::load_store(app)
            .map(|store| store.settings.auto_switch_on_exhaustion)
            .unwrap_or(false)
    };
    if !enabled {
        return false;
    }
    let Some(current) = summaries.iter().find(|account| account.is_current) else {
        return false;
    };
    exhausted_window(current).is_some()
}

pub(crate) async fn run_auto_switch(app: &AppHandle, state: &AppState, summaries: &[AccountSummary]) {
    let settings = {
        let _guard = state.store_lock.lock().await;
        match store::load_store(app) {
            Ok(store) => store.settings,
            Err(error) => {
                log::warn!("读取自动切换设置失败: {error}");
                return;
            }
        }
    };
    if !settings.auto_switch_on_exhaustion {
        return;
    }

    let Some(current) = summaries.iter().find(|account| account.is_current) else {
        return;
    };
    let Some(exhausted) = exhausted_window(current) else {
        return;
    };

    let account_key = current.account_key.clone();
    {
        let mut runtime = state.auto_switch.lock().await;
        if runtime.in_flight {
            return;
        }
        if runtime.last_trigger_account_key.as_deref() == Some(account_key.as_str())
            && now_unix_seconds() - runtime.last_trigger_at < SWITCH_COOLDOWN_SECONDS
        {
            return;
        }
        runtime.in_flight = true;
        runtime.last_trigger_account_key = Some(account_key);
        runtime.last_trigger_at = now_unix_seconds();
    }

    let from_label = current.label.clone();
    let from_id = current.id.clone();
    update_status(app, state, |status| {
        status.stage = "checking".to_string();
        status.message = format!(
            "当前账号 {from_label} 的 {} 额度已耗尽，正在寻找可用账号",
            exhausted.label()
        );
        status.from_account_id = Some(from_id.clone());
        status.from_account_label = Some(from_label.clone());
        status.to_account_id = None;
        status.to_account_label = None;
        status.thread_id = None;
        status.thread_name = None;
        status.continued = false;
        status.used_headless_fallback = false;
    })
    .await;

    let Some(target) = pick_switch_candidate(summaries, exhausted, &from_id) else {
        update_status(app, state, |status| {
            status.stage = "skipped".to_string();
            status.message = format!(
                "{} 额度已耗尽，但账号池里没有 {} 仍有剩余的账号，保持当前账号",
                from_label,
                exhausted.label()
            );
        })
        .await;
        state.auto_switch.lock().await.in_flight = false;
        return;
    };

    let target_id = target.id.clone();
    let target_label = target.label.clone();

    if dry_run_enabled() {
        update_status(app, state, |status| {
            status.stage = "skipped".to_string();
            status.message = format!(
                "[预演] {from_label} 的 {} 额度已耗尽，将切换到 {target_label}；未实际执行",
                exhausted.label()
            );
            status.to_account_id = Some(target_id.clone());
            status.to_account_label = Some(target_label.clone());
        })
        .await;
        state.auto_switch.lock().await.in_flight = false;
        return;
    }

    update_status(app, state, |status| {
        status.stage = "switching".to_string();
        status.message = format!("正在从 {from_label} 切换到 {target_label}");
        status.to_account_id = Some(target_id.clone());
        status.to_account_label = Some(target_label.clone());
    })
    .await;

    // 自动切换一律重启 Codex App：
    // 1. 只有重启才能让 App 及其 app-server 读取新的 auth.json；
    // 2. 旧进程持有的会话写锁会随退出释放，续跑才不会被 active writer 拒绝。
    // 额度耗尽时保护旧会话没有意义，因此这里忽略 launch_codex_after_switch。
    let _ = settings.launch_codex_after_switch;
    // 切换账号前，先停止旧 Codex App，写入新授权配置；暂不启动 App，等续跑完成后再拉起，避免写锁冲突。
    let switch_result = crate::switch_account_internal(
        app,
        state,
        target_id.clone(),
        None,
        Some(false),
        None,
        None,
    )
    .await;

    match switch_result {
        Ok(result) => {
            log::info!(
                "AUTO_SWITCH action=switched from={from_label} to={target_label} no_op={}",
                result.no_op
            );
        }
        Err(error) => {
            update_status(app, state, |status| {
                status.stage = "failed".to_string();
                status.message = format!("自动切换失败：{error}");
            })
            .await;
            state.auto_switch.lock().await.in_flight = false;
            return;
        }
    }

    // 切换成功后，读取最新额度快照，确认目标账号确实可用。
    let verification = refresh_after_switch(app, state).await;
    let (verified_id, verified_label) = verification
        .as_ref()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|account| account.is_current)
                .map(|account| (account.id.clone(), account.label.clone()))
        })
        .unwrap_or((target_id.clone(), target_label.clone()));

    update_status(app, state, |status| {
        status.stage = "switched".to_string();
        status.message = format!("已切换到 {verified_label}，正在检查 Codex 任务状态");
        status.to_account_id = Some(verified_id.clone());
        status.to_account_label = Some(verified_label.clone());
    })
    .await;

    if settings.auto_continue_after_switch {
        update_status(app, state, |status| {
            status.stage = "continuing".to_string();
            status.message = format!(
                "已切换账号，正在自动续跑未完成任务（最长等待 20 秒），完成后将拉起 Codex App"
            );
        })
        .await;

        auto_switch_continue::run_continue_and_release(settings.auto_continue_window_minutes).await;
    }

    // 续跑完成且会话写锁已完全释放后，正式启动 Codex App！
    if let Err(error) = crate::launch_codex_desktop_internal(app, state).await {
        log::warn!("自动切换后启动 Codex App 失败: {error}");
    }

    update_status(app, state, |status| {
        status.stage = "continued".to_string();
        status.continued = true;
        status.message = format!(
            "已切换账号并恢复中断任务，Codex App 已启动就绪"
        );
    })
    .await;

    state.auto_switch.lock().await.in_flight = false;
}

async fn refresh_after_switch(
    app: &AppHandle,
    state: &AppState,
) -> Option<Vec<AccountSummary>> {
    match crate::account_service::refresh_all_usage_coordinated(
        app,
        state,
        false,
        "auto-switch-verify",
    )
    .await
    {
        Ok(summaries) => {
            let _ = crate::tray::update_usage_surfaces_snapshot(app, &summaries);
            let _ = app.emit(crate::PERIODIC_USAGE_REFRESHED_EVENT, &summaries);
            Some(summaries)
        }
        Err(error) => {
            log::warn!("自动切换后刷新额度失败: {error}");
            None
        }
    }
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::UsageSnapshot;
    use crate::models::UsageWindow;

    fn account(
        id: &str,
        current: bool,
        five: Option<f64>,
        week: Option<f64>,
    ) -> AccountSummary {
        AccountSummary {
            id: id.to_string(),
            label: id.to_string(),
            source_kind: AccountSourceKind::Chatgpt,
            email: None,
            account_key: id.to_string(),
            account_id: id.to_string(),
            plan_type: Some("plus".to_string()),
            subscription_active_until: None,
            api_base_url: None,
            model_name: None,
            balance_text: None,
            profile_auth_ready: true,
            profile_config_ready: true,
            profile_integrity_error: None,
            profile_last_validated_at: None,
            profile_last_validation_error: None,
            added_at: 0,
            updated_at: 0,
            usage: Some(UsageSnapshot {
                fetched_at: 0,
                plan_type: Some("plus".to_string()),
                five_hour: five.map(|value| UsageWindow {
                    used_percent: value,
                    window_seconds: 18_000,
                    reset_at: None,
                }),
                one_week: week.map(|value| UsageWindow {
                    used_percent: value,
                    window_seconds: 604_800,
                    reset_at: None,
                }),
                credits: None,
                reset_credits: None,
            }),
            usage_error: None,
            auth_refresh_blocked: false,
            auth_refresh_error: None,
            api_proxy_enabled: false,
            is_current: current,
        }
    }

    #[test]
    fn exhausted_window_prefers_weekly_when_both_are_full() {
        let account = account("a", true, Some(100.0), Some(100.0));
        assert_eq!(exhausted_window(&account), Some(WindowKind::OneWeek));
    }

    #[test]
    fn exhausted_window_ignores_accounts_with_remaining_quota() {
        let account = account("a", true, Some(99.0), Some(10.0));
        assert_eq!(exhausted_window(&account), None);
    }

    #[test]
    fn candidate_must_have_remaining_quota_on_the_exhausted_window() {
        let summaries = vec![
            account("current", true, Some(100.0), Some(10.0)),
            account("week-empty", false, Some(10.0), Some(100.0)),
            account("usable", false, Some(60.0), Some(20.0)),
            account("best", false, Some(70.0), Some(5.0)),
        ];
        let picked = pick_switch_candidate(&summaries, WindowKind::FiveHour, "current")
            .expect("a candidate should remain");
        assert_eq!(picked.id, "best");
    }

    #[test]
    fn candidate_selection_skips_current_account_and_blocked_accounts() {
        let mut blocked = account("blocked", false, Some(10.0), Some(10.0));
        blocked.auth_refresh_error = Some("expired".to_string());
        let summaries = vec![
            account("current", true, Some(100.0), Some(10.0)),
            blocked,
            account("usable", false, Some(50.0), Some(50.0)),
        ];
        let picked = pick_switch_candidate(&summaries, WindowKind::FiveHour, "current")
            .expect("a candidate should remain");
        assert_eq!(picked.id, "usable");
    }
}
