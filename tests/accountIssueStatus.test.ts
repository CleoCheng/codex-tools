import assert from "node:assert/strict";
import test from "node:test";
import {
  accountHasAuthorizationUsageError,
  accountHasBlockingIssue,
  accountIssueReason,
  accountStatus,
} from "../src/components/accounts/accountModel.ts";
import type { AccountSummary } from "../src/types/app.ts";

function account(usageError: string | null): AccountSummary {
  return { usageError } as AccountSummary;
}

test("authorization usage refresh failures count as blocking account issues", () => {
  const authorizationErrors = [
    "usage refresh failed: 401 Unauthorized",
    "更新失败：授权异常 (401)",
    "403 Forbidden",
    "invalid_grant",
    "需要重新登录",
    "account blocked / deactivated",
  ];
  for (const error of authorizationErrors) {
    assert.equal(accountHasAuthorizationUsageError(account(error)), true, error);
    assert.equal(accountHasBlockingIssue(account(error)), true, error);
    assert.equal(accountStatus(account(error)), "issue", error);
    assert.equal(accountIssueReason(account(error), "fallback"), error, error);
  }
});

test("transient usage refresh failures do not mark an account as an issue", () => {
  const transientErrors = [
    "network connection failed",
    "request timed out",
    "429 too many requests",
    "503 service unavailable",
    "invalid json response",
  ];
  for (const error of transientErrors) {
    assert.equal(accountHasAuthorizationUsageError(account(error)), false, error);
    assert.equal(accountHasBlockingIssue(account(error)), false, error);
    assert.notEqual(accountStatus(account(error)), "issue", error);
  }
});

test("accounts without a usage error keep their ordinary status", () => {
  assert.equal(accountHasAuthorizationUsageError(account(null)), false);
  assert.equal(accountHasBlockingIssue(account(null)), false);
  assert.notEqual(accountStatus(account(null)), "issue");
});
