import {
  formatFullDate,
  formatShortDateTime,
} from "../../utils/dateFormatting.ts";
import type {
  AccountFingerprintSummary,
  ModelFingerprintProbe,
} from "../../types/app";
import type { AccountStatus, UiCopy } from "./types";

export function getUiCopy(locale: string): UiCopy {
  if (locale === "zh-CN") {
    return {
      searchPlaceholder: "搜索账号 / 邮箱",
      allStatuses: "全部状态",
      allPlans: "所有套餐",
      proxyEnabled: "反代开启",
      statusUsing: "使用中",
      statusAvailable: "可用",
      statusLow: "即将耗尽",
      statusExhausted: "已耗尽",
      statusIssue: "异常账号",
      issueFallbackReason: "查看账号授权和用量状态",
      tokenUsageTitle: "Token 使用量",
      tokenUsageError: "Token 用量读取失败",
      detailsTitle: "详情",
      usageOverview: "使用概览",
      fiveHourUsage: "5 小时额度",
      weekUsage: "每周额度",
      remainingSuffix: (value) => `剩余 ${value}`,
      resetTime: "重置时间",
      resetCreditsTitle: "重置卡",
      resetCreditsAvailable: (count) =>
        count === null ? "可用数量未知" : `可用 ${count} 张`,
      resetCreditsExpiresAt: "过期时间（系统本地时间）",
      resetCreditsExpand: (hiddenCount) => `展开 ${hiddenCount} 张`,
      resetCreditsCollapse: "收起",
      planType: "套餐类型",
      recentSwitches: "最近切换记录",
      switchRecordAction: "切换到此账号",
      noSwitchRecords: "暂无切换记录",
      fromPrefix: "从",
      quickActions: "快捷操作",
      reauthorize: "重新登录",
      warmup: "激活 5h 窗口",
      warming: "预热中",
      exportAccount: "导出账号",
      exportAll: "全部导出",
      deleteAccount: "删除账号",
      switchAccount: "切换",
      edit: "编辑",
      save: "保存",
      cancel: "取消",
      noMatchesTitle: "没有匹配账号",
      noMatchesDescription: "调整搜索或筛选条件后再查看列表。",
      emptyValue: "--",
      filterEnabled: "已启用",
      filterDisabled: "已停用",
      filterNormal: "未降智",
      filterDegraded: "已降智",
      stateEnabled: "已启用",
      stateDisabled: "已停用",
      fingerprintNormal: "未降智",
      fingerprintDegraded: "已降智",
      fingerprintUntested: "测指纹",
      fingerprintSkipped: "已跳过",
      fingerprintProbing: "检测中...",
      fingerprintError: "检测失败",
      fingerprintAction: "检测模型指纹",
      fingerprintSectionTitle: "模型指纹检测",
      fingerprintTargetModel: "目标模型",
      fingerprintDetectedModel: "实际指纹",
      fingerprintConfidence: "置信度",
      fingerprintTtft: "首字延迟",
      fingerprintCheckedAt: "检测时间",
      fingerprintCheckedPrefix: "测于",
      fingerprintEmpty:
        "尚未进行模型指纹检测，点击下方按钮检测 gpt-6.2-sol 与 gpt-6.2-astra (xhigh) 是否被降智。",
      fingerprintNextRecovery: "下次恢复探针",
      fingerprintAutoDisabledHint:
        "检测到降智后已自动关闭启用状态，自动切换与反代均会跳过该账号。",
      enableToggleLabel: "启用",
    };
  }

  return {
    searchPlaceholder: "Search account / email",
    allStatuses: "All statuses",
    allPlans: "All plans",
    proxyEnabled: "Proxy enabled",
    statusUsing: "In use",
    statusAvailable: "Available",
    statusLow: "Low quota",
    statusExhausted: "Exhausted",
    statusIssue: "Needs attention",
    issueFallbackReason: "Check account auth and usage state",
    tokenUsageTitle: "Token usage",
    tokenUsageError: "Token usage unavailable",
    detailsTitle: "Details",
    usageOverview: "Usage overview",
    fiveHourUsage: "5-hour quota",
    weekUsage: "Weekly quota",
    remainingSuffix: (value) => `${value} remaining`,
    resetTime: "Reset time",
    resetCreditsTitle: "Reset cards",
    resetCreditsAvailable: (count) =>
      count === null ? "Available count unknown" : `${count} available`,
    resetCreditsExpiresAt: "Expires (system local time)",
    resetCreditsExpand: (hiddenCount) => `Show ${hiddenCount} more`,
    resetCreditsCollapse: "Show less",
    planType: "Plan",
    recentSwitches: "Recent switches",
    switchRecordAction: "Switched to this account",
    noSwitchRecords: "No switch records yet",
    fromPrefix: "from",
    quickActions: "Quick actions",
    reauthorize: "Re-login",
    warmup: "Activate 5h window",
    warming: "Warming up",
    exportAccount: "Export account",
    exportAll: "Export all",
    deleteAccount: "Delete",
    switchAccount: "Switch",
    edit: "Edit",
    save: "Save",
    cancel: "Cancel",
    noMatchesTitle: "No matching accounts",
    noMatchesDescription: "Change the search or filters to view accounts.",
    emptyValue: "--",
    filterEnabled: "Enabled",
    filterDisabled: "Disabled",
    filterNormal: "Not degraded",
    filterDegraded: "Degraded",
    stateEnabled: "Enabled",
    stateDisabled: "Disabled",
    fingerprintNormal: "Normal",
    fingerprintDegraded: "Degraded",
    fingerprintUntested: "Probe",
    fingerprintSkipped: "Skipped",
    fingerprintProbing: "Probing...",
    fingerprintError: "Probe failed",
    fingerprintAction: "Probe fingerprint",
    fingerprintSectionTitle: "Model fingerprint",
    fingerprintTargetModel: "Target model",
    fingerprintDetectedModel: "Detected fingerprint",
    fingerprintConfidence: "Confidence",
    fingerprintTtft: "TTFT",
    fingerprintCheckedAt: "Checked at",
    fingerprintCheckedPrefix: "Checked",
    fingerprintEmpty:
      "Not probed yet. Run a fingerprint check for gpt-6.2-sol and gpt-6.2-astra (xhigh).",
    fingerprintNextRecovery: "Next recovery probe",
    fingerprintAutoDisabledHint:
      "Degraded accounts are automatically disabled and skipped by auto-switch and the API proxy.",
    enableToggleLabel: "ON",
  };
}

export function statusLabel(status: AccountStatus, text: UiCopy): string {
  if (status === "using") return text.statusUsing;
  if (status === "exhausted") return text.statusExhausted;
  if (status === "low") return text.statusLow;
  if (status === "issue") return text.statusIssue;
  return text.statusAvailable;
}

export type FingerprintBadgeTone =
  | "isNormal"
  | "isDegraded"
  | "isError"
  | "isUntested";

export type FingerprintBadgeLine = {
  probeLabel: string;
  text: string;
  tone: FingerprintBadgeTone;
};

export type FingerprintBadgeView =
  | {
      kind: "single";
      tone: FingerprintBadgeTone;
      text: string;
      checkedAtText: string | null;
      buttonTone: FingerprintBadgeTone;
    }
  | {
      kind: "combined";
      tone: "isDegraded";
      text: string;
      checkedAtText: string | null;
      buttonTone: "isDegraded";
    }
  | {
      kind: "twoLine";
      lines: [FingerprintBadgeLine, FingerprintBadgeLine];
      checkedAtText: string | null;
      buttonTone: FingerprintBadgeTone;
    };

/** Collapses verbose backend model identifiers into short UI tags. */
export function shortModelTag(model: string | null | undefined): string {
  const trimmed = (model ?? "").trim();
  if (!trimmed) return "";
  const lower = trimmed.toLowerCase();
  if (lower === "gpt-6-astra/6.1-sol" || lower === "6-astra/6.1-sol") {
    return "6.1sol";
  }
  if (lower === "6.2sol/6.2astra" || lower === "gpt-6.2-sol/gpt-6.2-astra") {
    return "6.2sol / 6.2astra";
  }
  return trimmed
    .replace(/^gpt-/i, "")
    .replace(/^(\d+(?:\.\d+)?)-([a-z]+)/i, "$1$2");
}

function formatBadgeCheckedAt(
  fingerprint: AccountFingerprintSummary | null | undefined,
  text: UiCopy,
): string | null {
  const formatted = formatShortDateTime(fingerprint?.checkedAt);
  return formatted ? `${text.fingerprintCheckedPrefix} ${formatted}` : null;
}

function resolveProbeLine(
  probe: ModelFingerprintProbe | null | undefined,
  fallbackLabel: string,
  text: UiCopy,
): FingerprintBadgeLine & { detectedTag: string; degraded: boolean } {
  const label = shortModelTag(probe?.targetModel) || fallbackLabel;
  if (!probe) {
    return {
      probeLabel: label,
      text: `${label} ${text.fingerprintUntested}`,
      tone: "isUntested",
      detectedTag: "",
      degraded: false,
    };
  }
  if (probe.skipped) {
    return {
      probeLabel: label,
      text: `${label} ${text.fingerprintSkipped}`,
      tone: "isDegraded",
      detectedTag: "",
      degraded: false,
    };
  }
  const detectedTag = shortModelTag(
    probe.detectedModel || probe.detectedModelId,
  );
  if (detectedTag) {
    if (probe.degraded) {
      return {
        probeLabel: label,
        text: `${label}->${detectedTag}`,
        tone: "isDegraded",
        detectedTag,
        degraded: true,
      };
    }
    return {
      probeLabel: label,
      text: `${label} ${text.fingerprintNormal}`,
      tone: "isNormal",
      detectedTag,
      degraded: false,
    };
  }
  if (probe.error) {
    return {
      probeLabel: label,
      text: `${label} ${text.fingerprintError}`,
      tone: "isError",
      detectedTag: "",
      degraded: false,
    };
  }
  return {
    probeLabel: label,
    text: `${label} ${text.fingerprintUntested}`,
    tone: "isUntested",
    detectedTag: "",
    degraded: false,
  };
}

/** Builds the single-line or stacked two-line badge representation with test time. */
export function resolveFingerprintBadgeView(
  fingerprint: AccountFingerprintSummary | null | undefined,
  text: UiCopy,
): FingerprintBadgeView {
  if (!fingerprint) {
    return {
      kind: "single",
      tone: "isUntested",
      text: text.fingerprintUntested,
      checkedAtText: null,
      buttonTone: "isUntested",
    };
  }

  const checkedAtText = formatBadgeCheckedAt(fingerprint, text);
  const solProbe = fingerprint.sol ?? null;
  const astraProbe = fingerprint.astra ?? null;
  if (solProbe || astraProbe) {
    const solLine = resolveProbeLine(solProbe, "6.2sol", text);
    const astraLine = resolveProbeLine(astraProbe, "6.2astra", text);
    if (
      solLine.degraded &&
      astraLine.degraded &&
      solLine.detectedTag &&
      astraLine.detectedTag &&
      solLine.detectedTag.toLowerCase() === astraLine.detectedTag.toLowerCase()
    ) {
      return {
        kind: "combined",
        tone: "isDegraded",
        text: `${solLine.probeLabel}、${astraLine.probeLabel}->${solLine.detectedTag}`,
        checkedAtText,
        buttonTone: "isDegraded",
      };
    }
    const buttonTone: FingerprintBadgeTone =
      solLine.tone === "isNormal" && astraLine.tone === "isNormal"
        ? "isNormal"
        : solLine.tone === "isDegraded" || astraLine.tone === "isDegraded"
          ? "isDegraded"
          : solLine.tone === "isError" || astraLine.tone === "isError"
            ? "isError"
            : "isUntested";
    return {
      kind: "twoLine",
      lines: [
        {
          probeLabel: solLine.probeLabel,
          text: solLine.text,
          tone: solLine.tone,
        },
        {
          probeLabel: astraLine.probeLabel,
          text: astraLine.text,
          tone: astraLine.tone,
        },
      ],
      checkedAtText,
      buttonTone,
    };
  }

  const legacyTag = shortModelTag(
    fingerprint.detectedModel || fingerprint.detectedModelId,
  );
  if (legacyTag) {
    if (fingerprint.degraded) {
      const targetTag = shortModelTag(fingerprint.targetModel) || "6.1sol";
      return {
        kind: "single",
        tone: "isDegraded",
        text: `${targetTag}->${legacyTag}`,
        checkedAtText,
        buttonTone: "isDegraded",
      };
    }
    return {
      kind: "single",
      tone: "isNormal",
      text: `${text.fingerprintNormal} · ${legacyTag}`,
      checkedAtText,
      buttonTone: "isNormal",
    };
  }

  if (fingerprint.error) {
    return {
      kind: "single",
      tone: "isError",
      text: text.fingerprintError,
      checkedAtText,
      buttonTone: "isError",
    };
  }

  return {
    kind: "single",
    tone: "isUntested",
    text: text.fingerprintUntested,
    checkedAtText: null,
    buttonTone: "isUntested",
  };
}

function formatConfidence(confidence: number | null | undefined): string | null {
  return confidence !== null && confidence !== undefined
    ? `${(confidence * 100).toFixed(2)}%`
    : null;
}

/** Formats tooltip text for a single model probe or an account's dual-model summary. */
export function formatFingerprintTooltip(
  fingerprint: AccountFingerprintSummary | null | undefined,
  text: UiCopy,
): string {
  if (!fingerprint) return text.fingerprintAction;
  const locale = text.fingerprintNormal === "未降智" ? "zh-CN" : "en-US";
  const checkedLine =
    fingerprint.checkedAt > 0
      ? `${text.fingerprintCheckedAt}: ${formatFullDate(fingerprint.checkedAt, locale, text.emptyValue)}`
      : null;
  const nextRecoveryLine =
    fingerprint.autoDisabledByFingerprint && fingerprint.nextRecoveryAt != null
      ? `${text.fingerprintNextRecovery}: ${formatFullDate(fingerprint.nextRecoveryAt, locale, text.emptyValue)}`
      : null;
  const probes = [fingerprint.sol, fingerprint.astra].filter(
    (item): item is ModelFingerprintProbe => Boolean(item),
  );
  if (probes.length > 0) {
    const lines = probes.map((probe) => {
      const label = `${shortModelTag(probe.targetModel) || probe.targetModel} (${probe.effort})`;
      if (probe.skipped) {
        return `${label}: ${text.fingerprintSkipped}`;
      }
      const detected = probe.detectedModel || probe.detectedModelId;
      const confidence = formatConfidence(probe.confidence);
      if (detected) {
        const statusText = probe.degraded
          ? `${text.fingerprintDegraded} -> ${shortModelTag(detected)}`
          : `${text.fingerprintNormal} -> ${shortModelTag(detected)}`;
        return `${label}: ${statusText}${confidence ? ` (${confidence})` : ""}${probe.error ? ` [${probe.error}]` : ""}`;
      }
      return `${label}: ${probe.error || text.fingerprintUntested}`;
    });
    if (checkedLine) {
      lines.push(checkedLine);
    }
    if (nextRecoveryLine) {
      lines.push(nextRecoveryLine);
    }
    if (
      fingerprint.error &&
      !probes.some((item) => item.error === fingerprint.error)
    ) {
      lines.push(fingerprint.error);
    }
    return lines.join("\n");
  }
  const detected = fingerprint.detectedModel || fingerprint.detectedModelId;
  const confidence = formatConfidence(fingerprint.confidence);
  if (detected) {
    const base = `${fingerprint.targetModel} -> ${detected}${confidence ? ` (${confidence})` : ""}`;
    return [
      base,
      checkedLine,
      nextRecoveryLine,
      fingerprint.error ? `(${fingerprint.error})` : null,
    ]
      .filter(Boolean)
      .join("\n");
  }
  return [fingerprint.error || text.fingerprintAction, checkedLine, nextRecoveryLine]
    .filter(Boolean)
    .join("\n");
}
