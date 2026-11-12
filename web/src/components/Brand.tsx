// PixPanel marka işareti: dört pikselden biri vurgulu.

import { brandName, useBranding } from "../lib/branding";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

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

/** Markanın destek bağlantısı; tanımlı değilse hiçbir şey çizilmez. Sunucu yalnızca https: ve mailto:
 *  kabul eder; burada da aynı beyaz liste uygulanır. */
export function SupportLink({ className }: { className?: string }) {
  const { support_url: url } = useBranding();
  if (!/^(https:\/\/|mailto:)/.test(url)) return null;
  const external = url.startsWith("https://");
  return (
    <a href={url} {...(external ? { target: "_blank", rel: "noopener noreferrer" } : {})} className={className}>
      {t("Destek")}
    </a>
  );
}
