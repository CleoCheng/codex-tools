import assert from "node:assert/strict";
import test from "node:test";
import {
  formatFingerprintTooltip,
  getUiCopy,
  resolveFingerprintBadgeView,
  shortModelTag,
} from "../src/components/accounts/accountCopy.ts";
import { formatShortDateTime } from "../src/utils/dateFormatting.ts";
import type {
  AccountFingerprintSummary,
  ModelFingerprintProbe,
} from "../src/types/app.ts";

function makeProbe(
  overrides: Partial<ModelFingerprintProbe>,
): ModelFingerprintProbe {
  return {
    targetModel: "gpt-6.2-sol",
    effort: "xhigh",
    degraded: false,
    detectedModel: "gpt-6.2-sol",
    detectedModelId: "gpt-6.2-sol",
    confidence: 0.99,
    score: 4.1,
    runnerUpModel: "gpt-5.6-luna",
    runnerUpScore: 1.8,
    validSamples: 3,
    totalSamples: 3,
    ttftMs: 1200,
    error: null,
    ...overrides,
  };
}

function makeSummary(
  overrides: Partial<AccountFingerprintSummary>,
): AccountFingerprintSummary {
  return {
    degraded: false,
    targetModel: "6.2sol/6.2astra",
    detectedModel: "gpt-6.2-sol",
    detectedModelId: "gpt-6.2-sol",
    confidence: 0.99,
    score: 4.1,
    runnerUpModel: null,
    runnerUpScore: null,
    validSamples: 6,
    totalSamples: 6,
    ttftMs: 1200,
    checkedAt: 1795200000,
    error: null,
    ...overrides,
  };
}

test("shortModelTag collapses GPT model prefixes cleanly", () => {
  assert.equal(shortModelTag("gpt-6.2-sol"), "6.2sol");
  assert.equal(shortModelTag("gpt-6.2-astra"), "6.2astra");
  assert.equal(shortModelTag("gpt-5.6-luna"), "5.6luna");
  assert.equal(shortModelTag("gpt-6-astra/6.1-sol"), "6.1sol");
  assert.equal(shortModelTag("6.2sol/6.2astra"), "6.2sol / 6.2astra");
});

test("resolveFingerprintBadgeView combines identical degraded targets into one compact label", () => {
  const text = getUiCopy("zh-CN");
  const expectedTime = `测于 ${formatShortDateTime(1795200000)}`;
  const summary = makeSummary({
    degraded: true,
    detectedModel: "gpt-5.6-luna",
    detectedModelId: "gpt-5.6-luna",
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      degraded: true,
      detectedModel: "gpt-5.6-luna",
      detectedModelId: "gpt-5.6-luna",
    }),
    astra: makeProbe({
      targetModel: "gpt-6.2-astra",
      degraded: true,
      detectedModel: "gpt-5.6-luna",
      detectedModelId: "gpt-5.6-luna",
    }),
  });

  assert.deepEqual(resolveFingerprintBadgeView(summary, text), {
    kind: "combined",
    tone: "isDegraded",
    text: "6.2sol、6.2astra->5.6luna",
    checkedAtText: expectedTime,
    buttonTone: "isDegraded",
  });
});

test("resolveFingerprintBadgeView stacks two lines when models differ, skip astra, or are normal", () => {
  const text = getUiCopy("zh-CN");
  const expectedTime = `测于 ${formatShortDateTime(1795200000)}`;

  const differentDegraded = makeSummary({
    degraded: true,
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      degraded: true,
      detectedModel: "gpt-5.6-luna",
      detectedModelId: "gpt-5.6-luna",
    }),
    astra: makeProbe({
      targetModel: "gpt-6.2-astra",
      degraded: true,
      detectedModel: "gpt-5.6-terra",
      detectedModelId: "gpt-5.6-terra",
    }),
  });
  assert.deepEqual(resolveFingerprintBadgeView(differentDegraded, text), {
    kind: "twoLine",
    buttonTone: "isDegraded",
    checkedAtText: expectedTime,
    lines: [
      { probeLabel: "6.2sol", text: "6.2sol->5.6luna", tone: "isDegraded" },
      { probeLabel: "6.2astra", text: "6.2astra->5.6terra", tone: "isDegraded" },
    ],
  });

  const earlyStopped = makeSummary({
    degraded: true,
    detectedModel: "gpt-5.6-luna",
    detectedModelId: "gpt-5.6-luna",
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      degraded: true,
      detectedModel: "gpt-5.6-luna",
      detectedModelId: "gpt-5.6-luna",
      validSamples: 2,
      totalSamples: 2,
    }),
    astra: makeProbe({
      targetModel: "gpt-6.2-astra",
      degraded: false,
      skipped: true,
      detectedModel: "",
      detectedModelId: "",
      validSamples: 0,
      totalSamples: 0,
    }),
  });
  assert.deepEqual(resolveFingerprintBadgeView(earlyStopped, text), {
    kind: "twoLine",
    buttonTone: "isDegraded",
    checkedAtText: expectedTime,
    lines: [
      { probeLabel: "6.2sol", text: "6.2sol->5.6luna", tone: "isDegraded" },
      { probeLabel: "6.2astra", text: "6.2astra 已跳过", tone: "isDegraded" },
    ],
  });

  const mixed = makeSummary({
    degraded: true,
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      degraded: false,
      detectedModel: "gpt-6.2-sol",
      detectedModelId: "gpt-6.2-sol",
    }),
    astra: makeProbe({
      targetModel: "gpt-6.2-astra",
      degraded: true,
      detectedModel: "gpt-5.6-luna",
      detectedModelId: "gpt-5.6-luna",
    }),
  });
  assert.deepEqual(resolveFingerprintBadgeView(mixed, text), {
    kind: "twoLine",
    buttonTone: "isDegraded",
    checkedAtText: expectedTime,
    lines: [
      { probeLabel: "6.2sol", text: "6.2sol 未降智", tone: "isNormal" },
      { probeLabel: "6.2astra", text: "6.2astra->5.6luna", tone: "isDegraded" },
    ],
  });

  const bothNormal = makeSummary({
    degraded: false,
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      degraded: false,
      detectedModel: "gpt-6.2-sol",
      detectedModelId: "gpt-6.2-sol",
    }),
    astra: makeProbe({
      targetModel: "gpt-6.2-astra",
      degraded: false,
      detectedModel: "gpt-6.2-astra",
      detectedModelId: "gpt-6.2-astra",
    }),
  });
  assert.deepEqual(resolveFingerprintBadgeView(bothNormal, text), {
    kind: "twoLine",
    buttonTone: "isNormal",
    checkedAtText: expectedTime,
    lines: [
      { probeLabel: "6.2sol", text: "6.2sol 未降智", tone: "isNormal" },
      { probeLabel: "6.2astra", text: "6.2astra 未降智", tone: "isNormal" },
    ],
  });

  const tooltip = formatFingerprintTooltip(bothNormal, text);
  assert.match(tooltip, /6\.2sol \(xhigh\): 未降智 -> 6\.2sol/);
  assert.match(tooltip, /6\.2astra \(xhigh\): 未降智 -> 6\.2astra/);
  assert.match(tooltip, /检测时间:/);
});

test("resolveFingerprintBadgeView preserves legacy single-probe and error displays", () => {
  const text = getUiCopy("zh-CN");
  const expectedTime = `测于 ${formatShortDateTime(1795200000)}`;
  assert.deepEqual(
    resolveFingerprintBadgeView(
      makeSummary({
        degraded: true,
        targetModel: "gpt-6.1-sol",
        detectedModel: "gpt-5.6-luna",
        detectedModelId: "gpt-5.6-luna",
      }),
      text,
    ),
    {
      kind: "single",
      tone: "isDegraded",
      text: "6.1sol->5.6luna",
      checkedAtText: expectedTime,
      buttonTone: "isDegraded",
    },
  );
  assert.deepEqual(
    resolveFingerprintBadgeView(
      makeSummary({
        degraded: false,
        targetModel: "gpt-6.1-sol",
        detectedModel: "",
        detectedModelId: "",
        error: "usage_limit_reached",
      }),
      text,
    ),
    {
      kind: "single",
      tone: "isError",
      text: text.fingerprintError,
      checkedAtText: expectedTime,
      buttonTone: "isError",
    },
  );
});

test("formatFingerprintTooltip includes next recovery time only for auto-disabled summaries", () => {
  const text = getUiCopy("zh-CN");
  const scheduled = makeSummary({
    degraded: true,
    autoDisabledByFingerprint: true,
    autoDisabledAt: 1795200000,
    recoveryAttempts: 1,
    nextRecoveryAt: 1795207200,
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      detectedModel: "gpt-4o",
      detectedModelId: "gpt-4o",
      degraded: true,
    }),
  });
  const scheduledTooltip = formatFingerprintTooltip(scheduled, text);
  assert.match(scheduledTooltip, /下次恢复探针:/);

  const manual = makeSummary({
    degraded: true,
    autoDisabledByFingerprint: false,
    nextRecoveryAt: 1795207200,
    sol: makeProbe({
      targetModel: "gpt-6.2-sol",
      detectedModel: "gpt-4o",
      detectedModelId: "gpt-4o",
      degraded: true,
    }),
  });
  const manualTooltip = formatFingerprintTooltip(manual, text);
  assert.doesNotMatch(manualTooltip, /下次恢复探针:/);
});
