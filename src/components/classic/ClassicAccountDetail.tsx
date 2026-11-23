import { uiCopy } from "../../i18n/uiCopy";
import { ClassicQuickActions } from "./ClassicQuickActions";
import { ClassicAccountDetailHeader } from "./ClassicAccountDetailHeader";
import { ClassicRecentSwitches } from "./ClassicRecentSwitches";
import { MembershipExpiry } from "../accounts/MembershipExpiry";
import { UsageFreshnessBadge } from "../accounts/UsageFreshnessBadge";
import { AccountOrderControls } from "../accounts/AccountOrderControls";
import { ResetCreditsSection } from "../accounts/ResetCreditsSection";
import { UsageMeter } from "./ClassicUsageMeter";
import { ClassicAccountTokenCard } from "./ClassicAccountTokenCard";
import type { ClassicAccountsWorkspace } from "./useClassicAccounts";
import type { ModelFingerprintProbe } from "../../types/app";
import {
  resolveFingerprintBadgeView,
  shortModelTag,
} from "../accounts/accountCopy";
import { FingerprintBadge } from "../accounts/FingerprintBadge";
export function ClassicAccountDetail({
  workspace,
}: {
  workspace: ClassicAccountsWorkspace;
}) {
  const {
    copy,
    locale,
    text,
    selectedRow,
    switchRecords,
    expandedResetCreditsByAccount,
    toggleResetCredits,
    usageRefreshing,
    showInitialUsageRefresh,
    usageRefreshError,
    authBusy,
    onReauthorize,
    tokenUsageByAccount,
    actions,
  } = workspace;

  const selectedFingerprint = selectedRow
    ? actions.accountFingerprints?.[selectedRow.account.accountKey]
    : undefined;
  const selectedProbingFingerprint = selectedRow
    ? Boolean(
        actions.probingAccountKeys?.includes(selectedRow.account.accountKey),
      )
    : false;

  return (
    <aside className="accountDetailPanel" aria-label={text.detailsTitle}>
      {selectedRow ? (
        <>
          <ClassicAccountDetailHeader workspace={workspace} />

          <section className="detailCard">
            <div className="detailCardTitle">
              <h3>{text.usageOverview}</h3>
              <UsageFreshnessBadge
                account={selectedRow.account}
                refreshing={usageRefreshing}
                showInitialRefresh={showInitialUsageRefresh}
                refreshError={usageRefreshError}
                locale={locale}
                copy={copy.accountsGrid}
              />
            </div>
            <UsageMeter
              label={text.fiveHourUsage}
              windowLabel="5h"
              window={selectedRow.account.usage?.fiveHour ?? null}
              text={text}
              locale={locale}
            />
            <UsageMeter
              label={text.weekUsage}
              windowLabel="1w"
              window={selectedRow.account.usage?.oneWeek ?? null}
              text={text}
              locale={locale}
            />
          </section>

          {selectedRow.account.sourceKind !== "relay" && (
            <section className="detailCard fingerprintDetailCard">
              <div className="detailCardTitle">
                <h3>{text.fingerprintSectionTitle}</h3>
                {(selectedProbingFingerprint || selectedFingerprint) && (
                  <FingerprintBadge
                    view={resolveFingerprintBadgeView(selectedFingerprint, text)}
                    probing={selectedProbingFingerprint}
                    probingLabel={text.fingerprintProbing}
                  />
                )}
              </div>

              {selectedFingerprint ? (
                <>
                  <dl className="detailFingerprintGrid">
                    <div>
                      <dt>{text.fingerprintTargetModel}</dt>
                      <dd>
                        {shortModelTag(selectedFingerprint.targetModel) ||
                          selectedFingerprint.targetModel}
                      </dd>
                    </div>
                    <div>
                      <dt>{text.fingerprintDetectedModel}</dt>
                      <dd
                        className={`detailFingerprintValue ${
                          selectedFingerprint.detectedModel ||
                          selectedFingerprint.detectedModelId
                            ? selectedFingerprint.degraded
                              ? "isDegraded"
                              : "isNormal"
                            : selectedFingerprint.error
                              ? "isError"
                              : ""
                        }`}
                      >
                        {shortModelTag(
                          selectedFingerprint.detectedModel ||
                            selectedFingerprint.detectedModelId,
                        ) ||
                          (selectedFingerprint.error
                            ? text.fingerprintError
                            : text.emptyValue)}
                      </dd>
                    </div>
                    {[selectedFingerprint.sol, selectedFingerprint.astra]
                      .filter((probe): probe is ModelFingerprintProbe =>
                        Boolean(probe),
                      )
                      .map((probe) => {
                        const probeName = `${shortModelTag(probe.targetModel) || probe.targetModel} (${probe.effort})`;
                        const probeDetected = shortModelTag(
                          probe.detectedModel || probe.detectedModelId,
                        );
                        const probeTone = probe.skipped
                          ? "isDegraded"
                          : probeDetected
                            ? probe.degraded
                              ? "isDegraded"
                              : "isNormal"
                            : probe.error
                              ? "isError"
                              : "";
                        const parts: string[] = [];
                        if (probe.skipped) {
                          parts.push(text.fingerprintSkipped);
                        } else if (probeDetected) {
                          parts.push(
                            probe.degraded
                              ? `-> ${probeDetected}`
                              : `${text.fingerprintNormal} · ${probeDetected}`,
                          );
                        } else if (probe.error) {
                          parts.push(probe.error);
                        } else {
                          parts.push(text.emptyValue);
                        }
                        if (
                          probe.confidence !== null &&
                          probe.confidence !== undefined
                        ) {
                          parts.push(`${(probe.confidence * 100).toFixed(1)}%`);
                        }
                        if (
                          probe.ttftMs !== null &&
                          probe.ttftMs !== undefined
                        ) {
                          parts.push(`${probe.ttftMs}ms`);
                        }
                        return (
                          <div key={probe.targetModel}>
                            <dt>{probeName}</dt>
                            <dd
                              className={`detailFingerprintValue ${probeTone}`}
                              title={probe.error || undefined}
                            >
                              {parts.join(" · ")}
                            </dd>
                          </div>
                        );
                      })}
                    <div>
                      <dt>{text.fingerprintConfidence}</dt>
                      <dd>
                        {selectedFingerprint.confidence !== null &&
                        selectedFingerprint.confidence !== undefined
                          ? `${(selectedFingerprint.confidence * 100).toFixed(2)}%${
                              selectedFingerprint.score !== null &&
                              selectedFingerprint.score !== undefined
                                ? ` (${selectedFingerprint.score.toFixed(2)})`
                                : ""
                            }`
                          : selectedFingerprint.score !== null &&
                              selectedFingerprint.score !== undefined
                            ? selectedFingerprint.score.toFixed(2)
                            : text.emptyValue}
                      </dd>
                    </div>
                    <div>
                      <dt>{text.fingerprintTtft}</dt>
                      <dd>
                        {selectedFingerprint.ttftMs !== null &&
                        selectedFingerprint.ttftMs !== undefined
                          ? `${(selectedFingerprint.ttftMs / 1000).toFixed(2)}s`
                          : text.emptyValue}
                      </dd>
                    </div>
                    <div>
                      <dt>{text.fingerprintCheckedAt}</dt>
                      <dd>
                        {new Date(
                          selectedFingerprint.checkedAt * 1000,
                        ).toLocaleString(locale === "zh-CN" ? "zh-CN" : "en-US")}
                      </dd>
                    </div>
                    {selectedFingerprint.autoDisabledByFingerprint &&
                      !selectedRow.account.apiProxyEnabled &&
                      selectedFingerprint.nextRecoveryAt != null && (
                        <div>
                          <dt>{text.fingerprintNextRecovery}</dt>
                          <dd>
                            {new Date(
                              selectedFingerprint.nextRecoveryAt * 1000,
                            ).toLocaleString(
                              locale === "zh-CN" ? "zh-CN" : "en-US",
                            )}
                          </dd>
                        </div>
                      )}
                  </dl>
                  {selectedFingerprint.error && (
                    <p className="fingerprintDetailNote isError">
                      {selectedFingerprint.error}
                    </p>
                  )}
                  {selectedFingerprint.degraded && !selectedRow.account.apiProxyEnabled && (
                    <p className="fingerprintDetailNote isDegraded">
                      {text.fingerprintAutoDisabledHint}
                    </p>
                  )}
                </>
              ) : (
                <p className="fingerprintDetailNote">{text.fingerprintEmpty}</p>
              )}

              <button
                type="button"
                className="ghost fingerprintDetailProbeButton"
                disabled={
                  selectedProbingFingerprint ||
                  Boolean(actions.probingAllFingerprints) ||
                  authBusy
                }
                onClick={() =>
                  void actions.onProbeFingerprints?.([selectedRow.account])
                }
              >
                {selectedProbingFingerprint
                  ? text.fingerprintProbing
                  : text.fingerprintAction}
              </button>
            </section>
          )}

          <ClassicAccountTokenCard
            account={selectedRow.account}
            usage={tokenUsageByAccount.get(selectedRow.account.accountId)}
            locale={locale}
            emptyValue={text.emptyValue}
          />

          <details className="inspectorSecondary"><summary>{uiCopy(locale).accountManagement}</summary>
          <section className="detailMetaGrid">
            <MembershipExpiry
              account={selectedRow.account}
              locale={locale}
              authBusy={authBusy}
              reauthorizeLabel={text.reauthorize}
              onReauthorize={onReauthorize}
            />
          </section>

          <AccountOrderControls account={selectedRow.account} actions={workspace.actions} />
          <ResetCreditsSection
            key={selectedRow.account.id}
            busy={workspace.actions.authBusy}
            onUseCredit={workspace.actions.onUseResetCredit}
            account={selectedRow.account}
            expanded={Boolean(
              expandedResetCreditsByAccount[selectedRow.account.id],
            )}
            locale={locale}
            text={text}
            onToggle={() => toggleResetCredits(selectedRow.account.id)}
          />

          <ClassicRecentSwitches
            records={switchRecords}
            locale={locale}
            text={text}
          />

          </details>
          <ClassicQuickActions workspace={workspace} />
        </>
      ) : (
        <div className="detailEmpty">
          <h3>{copy.accountsGrid.emptyTitle}</h3>
          <p>{copy.accountsGrid.emptyDescription}</p>
        </div>
      )}
    </aside>
  );
}
