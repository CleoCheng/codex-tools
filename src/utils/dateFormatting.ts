import { dateFormatter } from "./intlFormatters.ts";

export function formatFullDate(
  epochSec: number | null | undefined,
  locale: string,
  emptyValue: string,
): string {
  if (!epochSec) {
    return emptyValue;
  }
  const date = new Date(epochSec * 1000);
  if (Number.isNaN(date.getTime())) return emptyValue;

  return dateFormatter(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
}

/** Formats a Unix epoch timestamp in seconds as compact local `MM/DD HH:mm`. */
export function formatShortDateTime(
  epochSec: number | null | undefined,
): string | null {
  if (!epochSec || !Number.isFinite(epochSec) || epochSec <= 0) {
    return null;
  }
  const date = new Date(epochSec * 1000);
  if (Number.isNaN(date.getTime())) {
    return null;
  }
  const month = String(date.getMonth() + 1).padStart(2, "0");
  const day = String(date.getDate()).padStart(2, "0");
  const hours = String(date.getHours()).padStart(2, "0");
  const minutes = String(date.getMinutes()).padStart(2, "0");
  return `${month}/${day} ${hours}:${minutes}`;
}
