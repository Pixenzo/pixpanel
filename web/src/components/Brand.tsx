// PixPanel marka işareti: dört pikselden biri vurgulu.

import { brandName, useBranding } from "../lib/branding";
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

/** Yöneticinin yüklediği logo; yoksa panelin kendi işareti. */
export function BrandLogo({ className, onDark }: { className?: string; onDark?: boolean }) {
  const branding = useBranding();
  if (branding.logo === "") return <BrandMark className={className} onDark={onDark} />;
  return <img src={branding.logo} alt="" className={classNames("size-5 object-contain", className)} />;
}

/** Panelin görünen adı (yönetici değiştirmediyse PixPanel). */
export function BrandName() {
  return <>{brandName(useBranding())}</>;
}
