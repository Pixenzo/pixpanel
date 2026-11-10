// PixPanel marka işareti: dört pikselden biri vurgulu.

import { classNames } from "../lib/format";

/** `onDark`: koyu zemin (yan gezinme) üzerinde kullanılan renkler. */
export function BrandMark({ className, onDark }: { className?: string; onDark?: boolean }) {
  const base = onDark ? "fill-rail-ink" : "fill-ink";
  const accent = onDark ? "fill-rail-accent" : "fill-accent";
  return (
    <svg viewBox="0 0 20 20" className={classNames("size-5", className)} aria-hidden>
      <rect x="1" y="1" width="8" height="8" rx="1.5" className={base} />
      <rect x="11" y="1" width="8" height="8" rx="1.5" className={accent} />
      <rect x="1" y="11" width="8" height="8" rx="1.5" className={base} />
      <rect x="11" y="11" width="8" height="8" rx="1.5" className={base} opacity="0.35" />
    </svg>
  );
}
