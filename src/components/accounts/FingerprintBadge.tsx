import type { MouseEvent } from "react";
import type { FingerprintBadgeView } from "./accountCopy";

type Props = {
  view: FingerprintBadgeView;
  probing?: boolean;
  probingLabel?: string;
  title?: string;
  disabled?: boolean;
  onClick?: (event: MouseEvent<HTMLButtonElement>) => void;
};

/** Renders the single-line, combined, or stacked two-line fingerprint badge. */
export function FingerprintBadge({
  view,
  probing = false,
  probingLabel,
  title,
  disabled = false,
  onClick,
}: Props) {
  if (probing) {
    return (
      <span className="accountFingerprintBadge isProbing">{probingLabel}</span>
    );
  }

  const hasTime = Boolean(view.checkedAtText);
  const stacked = view.kind === "twoLine" || hasTime;
  const className = [
    "accountFingerprintBadge",
    view.kind === "twoLine" ? "isTwoLine" : null,
    stacked ? "isStacked" : null,
    view.buttonTone,
  ]
    .filter(Boolean)
    .join(" ");

  const content = (
    <>
      {view.kind === "twoLine" ? (
        view.lines.map((line) => (
          <span key={line.probeLabel} className={`fpBadgeLine ${line.tone}`}>
            {line.text}
          </span>
        ))
      ) : (
        <span className={`fpBadgeLine ${view.tone}`}>{view.text}</span>
      )}
      {view.checkedAtText && (
        <span className="fpBadgeTime">{view.checkedAtText}</span>
      )}
    </>
  );

  if (onClick) {
    return (
      <button
        type="button"
        className={className}
        title={title}
        disabled={disabled}
        onClick={onClick}
      >
        {content}
      </button>
    );
  }

  return (
    <span className={className} title={title}>
      {content}
    </span>
  );
}
