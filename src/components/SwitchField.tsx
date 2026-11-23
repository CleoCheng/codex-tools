import type { ReactNode } from "react";
import { Switch } from "./ui/Switch";
import { SettingsRow } from "./ui/SettingsRow";
type SwitchFieldProps = {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  description?: ReactNode;
  badge?: ReactNode;
  checkedText: string;
  uncheckedText: string;
  disabled?: boolean;
  rowClassName?: string;
};

export function SwitchField({
  checked,
  onChange,
  label,
  description,
  badge,
  disabled = false,
  rowClassName,
}: SwitchFieldProps) {
  return (
    <SettingsRow label={label} description={description} badge={badge} className={rowClassName}>
      <Switch checked={checked} onChange={onChange} label={label} disabled={disabled} />
    </SettingsRow>
  );
}
