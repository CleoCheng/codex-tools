import type { AccountSummary, CodexAccountTokenUsage } from "../../types/app";
import { formatTokenCount } from "../../utils/usage";
import {
  formatCreditsBalance,
  formatTps,
  getAccountStatsCopy,
} from "./accountTokenStats";

/** Compact per-account figures shown in each list row. */
export function ClassicAccountStats({
  account,
  usage,
  locale,
}: {
  account: AccountSummary;
  usage: CodexAccountTokenUsage | undefined;
  locale: string;
}) {
  const copy = getAccountStatsCopy(locale);
  const credits = formatCreditsBalance(account, locale, copy);
  const tps = usage?.outputTps24h ?? null;
  // Keep older recorded usage visible when the last 24h is empty; always label
  // the actual period and retain all three periods in the tooltip.
  const period = usage?.last24h.totalTokens ? "24h"
    : usage?.last7d.totalTokens ? "7d"
    : usage?.last30d.totalTokens ? "30d" : "24h";
  const tokens = usage?.[period === "24h" ? "last24h" : period === "7d" ? "last7d" : "last30d"].totalTokens;

  return (
    <div className="accountStatsCell">
      <span
        className="accountStat"
        title={copy.tokenTitle(
          formatTokenCount(usage?.last24h.totalTokens ?? 0, locale),
          formatTokenCount(usage?.last7d.totalTokens ?? 0, locale),
          formatTokenCount(usage?.last30d.totalTokens ?? 0, locale),
        )}
      >
        <em>{period === "24h" ? copy.tokens24h : period === "7d" ? copy.tokens7d : copy.tokens30d}</em>
        <b className={tokens ? undefined : "isEmpty"}>
          {tokens === undefined ? "—" : formatTokenCount(tokens, locale)}
        </b>
      </span>
      <span
        className="accountStat"
        title={copy.tpsTitle(
          formatTps(tps, locale),
          formatTps(usage?.latestOutputTps, locale),
          formatTps(usage?.outputTps7d, locale),
        )}
      >
        <em>{copy.tps}</em>
        <b className={tps === null ? "isEmpty" : undefined}>
          {formatTps(tps, locale)}
        </b>
      </span>
      <span
        className="accountStat"
        title={`${copy.creditsTitle} · ${credits.value}`}
      >
        <em>{copy.credits}</em>
        <b className={credits.empty ? "isEmpty" : undefined}>{credits.value}</b>
      </span>
    </div>
  );
}
