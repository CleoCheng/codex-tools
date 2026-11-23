import type { ReactNode } from "react";

export function SettingsRow({
  label,
  description,
  badge,
  children,
  layout = "inline",
  className = "",
}: {
  label: ReactNode;
  description?: ReactNode;
  badge?: ReactNode;
  children: ReactNode;
  layout?: "inline" | "stacked";
  className?: string;
}) {
  return <div className={`settingRow uiSettingsRow uiSettingsRow-${layout} ${className}`}>
    <div className="settingMeta">
      <strong>{label}{badge}</strong>
      {description ? <span className="settingDescription">{description}</span> : null}
    </div>
    <div className="uiSettingsControl">{children}</div>
  </div>;
}
