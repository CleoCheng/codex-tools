import { useI18n } from "../../i18n/I18nProvider";
import type { AccountSummary, AutoSwitchStatus } from "../../types/app";
import {
  accountHasBlockingIssue,
  accountHasExhaustedWindow,
} from "../accounts/accountModel";
import { Switch } from "../ui/Switch";

type MetaStripProps = {
  accounts: AccountSummary[];
  exportingAccounts: boolean;
  onExportAccounts: () => void;
  autoSwitchStatus?: AutoSwitchStatus | null;
  autoSwitchOnExhaustion?: boolean;
  codexProxyBound?: boolean;
  savingSettings?: boolean;
  onToggleAutoSwitch?: (checked: boolean) => void;
};

type MetricTone = "blue" | "green" | "amber" | "red";

type MetricIconProps = {
  tone: MetricTone;
};

function MetricIcon({ tone }: MetricIconProps) {
  if (tone === "green") {
    return (
      <svg
        className="metricIconGlyph"
        viewBox="0 0 24 24"
        aria-hidden="true"
        focusable="false"
      >
        <path d="m5 12 4 4L19 6" />
      </svg>
    );
  }

  if (tone === "amber") {
    return (
      <svg
        className="metricIconGlyph"
        viewBox="0 0 24 24"
        aria-hidden="true"
        focusable="false"
      >
        <circle cx="12" cy="12" r="8" />
        <path d="M12 7v5l3 2" />
      </svg>
    );
  }

  if (tone === "red") {
    return (
      <svg
        className="metricIconGlyph"
        viewBox="0 0 24 24"
        aria-hidden="true"
        focusable="false"
      >
        <circle cx="12" cy="12" r="8" />
        <path d="M12 8v5" />
        <path d="M12 16h.01" />
      </svg>
    );
  }

  return (
    <svg
      className="metricIconGlyph"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M16 21v-2a4 4 0 0 0-8 0v2" />
      <circle cx="12" cy="7" r="4" />
    </svg>
  );
}

export function ClassicMetaStrip({
  accounts,
  exportingAccounts,
  onExportAccounts,
  autoSwitchStatus,
  autoSwitchOnExhaustion = false,
  codexProxyBound = false,
  savingSettings = false,
  onToggleAutoSwitch,
}: MetaStripProps) {
  const { copy, locale } = useI18n();
  const isChinese = locale === "zh-CN";
  const issueCount = accounts.filter(accountHasBlockingIssue).length;
  const exhaustedCount = accounts.filter(
    (account) =>
      !accountHasBlockingIssue(account) && accountHasExhaustedWindow(account),
  ).length;
  const activeCount = accounts.filter(
    (account) =>
      !accountHasBlockingIssue(account) &&
      !accountHasExhaustedWindow(account) &&
      account.profileAuthReady,
  ).length;
  const metrics: Array<{
    label: string;
    value: number;
    helper: string;
    tone: MetricTone;
  }> = [
    {
      label: copy.metaStrip.accountCount,
      value: accounts.length,
      helper: isChinese ? "个账号" : "total",
      tone: "blue",
    },
    {
      label: isChinese ? "活跃账号" : "Active",
      value: activeCount,
      helper: isChinese ? "正常使用" : "ready",
      tone: "green",
    },
    {
      label: isChinese ? "已耗尽" : "Exhausted",
      value: exhaustedCount,
      helper: isChinese ? "使用率 100%" : "used 100%",
      tone: "amber",
    },
    {
      label: isChinese ? "异常账号" : "Needs attention",
      value: issueCount,
      helper: isChinese ? "需要关注" : "review",
      tone: "red",
    },
  ];

  return (
    <section className="metaStrip" aria-label={copy.metaStrip.ariaLabel}>
      {metrics.map((metric) => (
        <article
          key={metric.label}
          className={`metaPill metric-${metric.tone}`}
          title={`${metric.label} · ${metric.helper}`}
        >
          <span className="metricIcon">
            <MetricIcon tone={metric.tone} />
          </span>
          <span className="metricCopy">
            <span>{metric.label}</span>
            <strong>{metric.value}</strong>
          </span>
        </article>
      ))}
      <button
        className="ghost metaExportButton"
        onClick={onExportAccounts}
        disabled={exportingAccounts || accounts.length === 0}
        aria-label={copy.metaStrip.exportAll}
      >
        {copy.metaStrip.exportAll}
      </button>
      {onToggleAutoSwitch ? (
        <div
          className={`autoSwitchQuickToggle ${
            codexProxyBound
              ? "isProxyLocked"
              : autoSwitchOnExhaustion
                ? "isEnabled"
                : "isDisabled"
          }`}
        >
          <div className="autoSwitchQuickCopy">
            <span className="autoSwitchQuickHeader">
              <svg
                className="autoSwitchQuickIcon"
                viewBox="0 0 24 24"
                fill="none"
                aria-hidden="true"
              >
                {codexProxyBound ? (
                  <>
                    <path
                      d="M7 11V8a5 5 0 0 1 10 0v3"
                      stroke="currentColor"
                      strokeWidth="1.9"
                      strokeLinecap="round"
                    />
                    <rect
                      x="5"
                      y="11"
                      width="14"
                      height="9"
                      rx="2.5"
                      stroke="currentColor"
                      strokeWidth="1.9"
                    />
                  </>
                ) : (
                  <>
                    <path
                      d="M16 4l4 4-4 4M20 8H7a3 3 0 0 0-3 3v1"
                      stroke="currentColor"
                      strokeWidth="1.9"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                    <path
                      d="M8 20l-4-4 4-4M4 16h13a3 3 0 0 0 3-3v-1"
                      stroke="currentColor"
                      strokeWidth="1.9"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </>
                )}
              </svg>
              <strong className="autoSwitchQuickTitle">
                {copy.metaStrip.autoSwitchLabel}
              </strong>
              <span className="autoSwitchQuickState">
                {codexProxyBound
                  ? copy.metaStrip.autoSwitchProxyBadge
                  : autoSwitchOnExhaustion
                    ? copy.metaStrip.autoSwitchOn
                    : copy.metaStrip.autoSwitchOff}
              </span>
            </span>
            <span className="autoSwitchQuickHint">
              {codexProxyBound
                ? copy.metaStrip.autoSwitchProxyHint
                : copy.metaStrip.autoSwitchHint}
            </span>
          </div>
          <div className="autoSwitchQuickControl">
            <Switch
              checked={codexProxyBound ? false : Boolean(autoSwitchOnExhaustion)}
              onChange={(checked) => {
                if (!codexProxyBound) {
                  onToggleAutoSwitch(checked);
                }
              }}
              label={copy.metaStrip.autoSwitchLabel}
              disabled={Boolean(savingSettings || codexProxyBound)}
            />
          </div>
        </div>
      ) : null}
      {autoSwitchStatus && autoSwitchStatus.stage !== "idle" ? (
        <span
          className={`autoSwitchBadge stage-${autoSwitchStatus.stage}`}
          role="status"
          title={autoSwitchStatus.message}
        >
          {autoSwitchStatus.message}
        </span>
      ) : null}
    </section>
  );
}
