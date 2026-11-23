import { useI18n } from "../../i18n/I18nProvider";
import { getUiCopy } from "../accounts/accountCopy";

type AddAccountSectionProps = {
  onOpenAddDialog: () => void;
  onSmartSwitch: () => void;
  onLaunchCodexApp: () => void;
  onProbeAllFingerprints?: () => void;
  probingAllFingerprints?: boolean;
  smartSwitching: boolean;
  launchingCodexApp: boolean;
};

function ShieldIcon() {
  return (
    <svg
      className="buttonIcon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M12 3 4.5 6v6c0 4.5 3.2 8.4 7.5 9.5 4.3-1.1 7.5-5 7.5-9.5V6L12 3z" />
      <path d="m9.5 12 1.8 1.8 3.5-3.8" />
    </svg>
  );
}

function PlayIcon() {
  return (
    <svg
      className="buttonIcon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M7 4.5v15l12-7.5z" />
    </svg>
  );
}

function SparkIcon() {
  return (
    <svg
      className="buttonIcon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M12 3v4" />
      <path d="M12 17v4" />
      <path d="M3 12h4" />
      <path d="M17 12h4" />
      <path d="m5.6 5.6 2.8 2.8" />
      <path d="m15.6 15.6 2.8 2.8" />
      <path d="m18.4 5.6-2.8 2.8" />
      <path d="m8.4 15.6-2.8 2.8" />
    </svg>
  );
}

function PlusIcon() {
  return (
    <svg
      className="buttonIcon"
      viewBox="0 0 24 24"
      aria-hidden="true"
      focusable="false"
    >
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </svg>
  );
}

export function ClassicAccountActions({
  onOpenAddDialog,
  onSmartSwitch,
  onLaunchCodexApp,
  onProbeAllFingerprints,
  probingAllFingerprints = false,
  smartSwitching,
  launchingCodexApp,
}: AddAccountSectionProps) {
  const { copy, locale } = useI18n();
  const text = getUiCopy(locale);

  return (
    <section className="importBar">
      {onProbeAllFingerprints && (
        <button
          className="ghost probeFingerprintsButton"
          onClick={onProbeAllFingerprints}
          disabled={probingAllFingerprints || smartSwitching}
          title={text.fingerprintAction}
          aria-label={text.fingerprintAction}
        >
          <ShieldIcon />
          {probingAllFingerprints
            ? text.fingerprintProbing
            : locale === "zh-CN"
              ? "检测降智"
              : "Probe models"}
        </button>
      )}
      <button
        className="ghost smartSwitchButton importSmartSwitch"
        onClick={onSmartSwitch}
        disabled={smartSwitching}
        title={copy.addAccount.smartSwitch}
        aria-label={copy.addAccount.smartSwitch}
      >
        <SparkIcon />
        {copy.addAccount.smartSwitch}
      </button>
      <button
        className="ghost launchCodexButton"
        onClick={onLaunchCodexApp}
        disabled={launchingCodexApp}
        title={copy.addAccount.launchCodexApp}
        aria-label={copy.addAccount.launchCodexApp}
      >
        <PlayIcon />
        {launchingCodexApp
          ? copy.addAccount.launchingCodexApp
          : copy.addAccount.launchCodexApp}
      </button>
      <button className="primary importPrimary" onClick={onOpenAddDialog}>
        <PlusIcon />
        {copy.addAccount.startButton}
      </button>
    </section>
  );
}
