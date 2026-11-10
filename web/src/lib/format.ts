// Biçimlendirme yardımcıları.

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

export function formatBytes(bytes: number, digits = 1): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), BYTE_UNITS.length - 1);
  const value = bytes / 1024 ** exponent;
  return `${value.toFixed(exponent === 0 ? 0 : digits)} ${BYTE_UNITS[exponent]}`;
}

export function formatRate(bytesPerSecond: number): string {
  return `${formatBytes(bytesPerSecond)}/sn`;
}

export function formatPercent(value: number, digits = 0): string {
  return `%${value.toFixed(digits)}`;
}

/** Süreyi en büyük iki birimle yazar: "3 gün 4 sa", "12 dk 5 sn". */
export function formatDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  const parts: [number, string][] = [
    [Math.floor(seconds / 86400), "gün"],
    [Math.floor((seconds % 86400) / 3600), "sa"],
    [Math.floor((seconds % 3600) / 60), "dk"],
    [seconds % 60, "sn"],
  ];
  const first = parts.findIndex(([value]) => value > 0);
  if (first === -1) return "0 sn";
  return parts
    .slice(first, first + 2)
    .filter(([value]) => value > 0)
    .map(([value, unit]) => `${value} ${unit}`)
    .join(" ");
}

const dateTimeFormat = new Intl.DateTimeFormat("tr-TR", {
  day: "2-digit",
  month: "2-digit",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

const timeFormat = new Intl.DateTimeFormat("tr-TR", { hour: "2-digit", minute: "2-digit", second: "2-digit" });

export function formatDateTime(unixSeconds: number | null | undefined): string {
  if (!unixSeconds) return "—";
  return dateTimeFormat.format(new Date(unixSeconds * 1000));
}

export function formatTime(unixSeconds: number): string {
  return timeFormat.format(new Date(unixSeconds * 1000));
}

/** "5 dakika önce" gibi göreli zaman. */
export function formatRelative(unixSeconds: number | null | undefined): string {
  if (!unixSeconds) return "hiç";
  const diff = Math.floor(Date.now() / 1000) - unixSeconds;
  if (diff < 60) return "az önce";
  if (diff < 3600) return `${Math.floor(diff / 60)} dk önce`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} sa önce`;
  if (diff < 30 * 86400) return `${Math.floor(diff / 86400)} gün önce`;
  return formatDateTime(unixSeconds);
}

export function classNames(...values: (string | false | null | undefined)[]): string {
  return values.filter(Boolean).join(" ");
}

/** Karışabilen karakterler (0/O, 1/l) olmadan rastgele parola üretir. */
export function generatePassword(length = 20): string {
  const alphabet = "abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const values = crypto.getRandomValues(new Uint32Array(length));
  return Array.from(values, (value) => alphabet[value % alphabet.length]).join("");
}
