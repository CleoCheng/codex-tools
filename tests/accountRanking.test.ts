import assert from "node:assert/strict";
import test from "node:test";
import {
  pickBestRemainingAccount,
  pickBestSmartSwitchAccount,
} from "../src/utils/accountRanking.ts";
import type { AccountSummary } from "../src/types/app.ts";

function account(
  id: string,
  usedPercent: number,
  apiProxyEnabled: boolean,
  sourceKind: "chatgpt" | "relay" = "chatgpt",
): AccountSummary {
  return {
    id,
    label: id,
    sourceKind,
    email: `${id}@example.com`,
    accountKey: `${id}@example.com|${id}`,
    accountId: id,
    planType: "plus",
    subscriptionActiveUntil: null,
    apiBaseUrl: null,
    modelName: null,
    balanceText: null,
    profileAuthReady: true,
    profileConfigReady: true,
    profileIntegrityError: null,
    profileLastValidatedAt: null,
    profileLastValidationError: null,
    addedAt: 1,
    updatedAt: 1,
    usage: {
      planType: "plus",
      fiveHour: { usedPercent, windowSeconds: 18000, resetAt: null },
      oneWeek: { usedPercent, windowSeconds: 604800, resetAt: null },
      credits: null,
    },
    usageError: null,
    authRefreshBlocked: false,
    authRefreshError: null,
    apiProxyEnabled,
    isCurrent: false,
  };
}

test("smart switch and best-remaining skip disabled/degraded accounts", () => {
  const degraded = account("degraded-high-quota", 0, false);
  const healthy = account("healthy-lower-quota", 35, true);

  assert.equal(pickBestRemainingAccount([degraded, healthy])?.id, healthy.id);
  assert.equal(
    pickBestSmartSwitchAccount([degraded, healthy], true)?.id,
    healthy.id,
  );
  assert.equal(pickBestRemainingAccount([degraded]), null);
  assert.equal(pickBestSmartSwitchAccount([degraded], true), null);
});
