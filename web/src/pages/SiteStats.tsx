// Sitenin ziyaret ve trafik istatistikleri: dönem toplamları, günlük ziyaretçi grafiği,
// yanıt durumları ve en çok görüntülenen sayfalar. Veriler erişim günlüklerinden çıkarılır.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { EmptyState, ErrorBlock, LoadingBlock, Mono, Panel, Status, Table, Td, Th } from "../components/ui";
import type { Tone } from "../components/ui";
import type { Site, SiteDayStats, SiteStats } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes } from "../lib/format";
import { getLanguage, t } from "../lib/i18n";

const PERIODS = [7, 30, 90];
/** Grafiğin çizim alanının yüksekliği (piksel). */
const PLOT_HEIGHT = 168;

const numberFormat = new Intl.NumberFormat(getLanguage() === "en" ? "en-GB" : "tr-TR");
const dayFormat = new Intl.DateTimeFormat(getLanguage() === "en" ? "en-GB" : "tr-TR", { day: "numeric", month: "short" });
const longDayFormat = new Intl.DateTimeFormat(getLanguage() === "en" ? "en-GB" : "tr-TR", { day: "numeric", month: "long", weekday: "long" });

/** `YYYYMMDD` gün anahtarını tarihe çevirir. */
function dayDate(key: number): Date {
  return new Date(Math.floor(key / 10000), (Math.floor(key / 100) % 100) - 1, key % 100);
}

function dayKey(date: Date): number {
  return date.getFullYear() * 10000 + (date.getMonth() + 1) * 100 + date.getDate();
}

/** Dönemin her günü için bir kayıt üretir; veri olmayan günler sıfırla doldurulur ki eksen kesintisiz olsun. */
function fillDays(days: SiteDayStats[], period: number): SiteDayStats[] {
  const byKey = new Map(days.map((day) => [day.day, day]));
  const filled: SiteDayStats[] = [];
  const cursor = new Date();
  cursor.setHours(12, 0, 0, 0);
  cursor.setDate(cursor.getDate() - (period - 1));
  for (let index = 0; index < period; index += 1) {
    const key = dayKey(cursor);
    filled.push(byKey.get(key) ?? { day: key, requests: 0, pages: 0, visitors: 0, bytes: 0, bots: 0, s2xx: 0, s3xx: 0, s4xx: 0, s5xx: 0 });
    cursor.setDate(cursor.getDate() + 1);
  }
  return filled;
}

/** Eksenin üst sınırı: en büyük değerin üstündeki "temiz" sayı (1, 2, 5 × 10ⁿ katları). */
function niceCeiling(value: number): number {
  if (value <= 4) return 4;
  const magnitude = 10 ** Math.floor(Math.log10(value));
  const step = [1, 2, 4, 5, 10].find((factor) => factor * magnitude >= value) ?? 10;
  return step * magnitude;
}

function Figure({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="min-w-0">
      <dt className="text-[12.5px] text-ink-3">{label}</dt>
      <dd className="tabular truncate font-mono text-[20px] leading-tight font-medium">{value}</dd>
      {hint && <dd className="text-[12.5px] text-ink-3">{hint}</dd>}
    </div>
  );
}

/** Günlük ziyaretçi sütunları. Tek seri olduğu için gösterge kutusu yoktur; başlık neyin çizildiğini söyler. */
function VisitorsChart({ days }: { days: SiteDayStats[] }) {
  const [active, setActive] = useState<number | null>(null);
  const ceiling = niceCeiling(Math.max(...days.map((day) => day.visitors)));
  const ticks = [ceiling, ceiling / 2, 0];
  // Etiketler seyrek tutulur: ilk gün, ortası ve bugün.
  const labelled = new Set([0, Math.floor((days.length - 1) / 2), days.length - 1]);
  const hovered = active === null ? null : days[active];

  return (
    <div>
      <div className="flex gap-2">
        <div className="tabular flex shrink-0 flex-col justify-between text-right text-[12px] text-ink-3" style={{ height: PLOT_HEIGHT }} aria-hidden>
          {ticks.map((tick) => (
            <span key={tick} className="leading-none">
              {numberFormat.format(tick)}
            </span>
          ))}
        </div>
        <div className="relative min-w-0 flex-1" style={{ height: PLOT_HEIGHT }} onMouseLeave={() => setActive(null)}>
          {/* Izgara çizgileri: yüzeyden bir ton farklı, ince ve düz. */}
          {ticks.map((tick, index) => (
            <span key={tick} aria-hidden className="absolute inset-x-0 h-px bg-line" style={{ top: `${(index / (ticks.length - 1)) * 100}%` }} />
          ))}
          <div className="absolute inset-0 flex items-end">
            {days.map((day, index) => (
              <button
                key={day.day}
                type="button"
                // Dokunma ve fare hedefi çubuğun kendisi değil, o günün bütün sütunudur.
                onMouseEnter={() => setActive(index)}
                onFocus={() => setActive(index)}
                onBlur={() => setActive(null)}
                onClick={() => setActive(index)}
                aria-label={t("{day}: {visitors} ziyaretçi, {pages} sayfa görüntüleme", {
                  day: longDayFormat.format(dayDate(day.day)),
                  visitors: day.visitors,
                  pages: day.pages,
                })}
                className={classNames("flex h-full min-w-0 flex-1 items-end justify-center px-px outline-offset-[-2px]", active === index && "bg-sunken/70")}
              >
                <span
                  className={classNames("w-full max-w-6 rounded-t bg-(--chart-1) transition-opacity", active !== null && active !== index && "opacity-45")}
                  style={{ height: day.visitors === 0 ? 0 : `max(2px, ${(day.visitors / ceiling) * 100}%)` }}
                />
              </button>
            ))}
          </div>
          {hovered && active !== null && (
            <div
              role="status"
              className={classNames(
                "pointer-events-none absolute top-1 z-10 w-max max-w-[15rem] rounded-lg border border-line bg-surface px-3 py-2 text-[13px] shadow-pop",
                // İpucu sütunun yanında durur; sağ yarıda sola açılır ki alanın dışına taşmasın.
                active > days.length / 2 ? "-translate-x-full" : "",
              )}
              style={{ left: `${((active + (active > days.length / 2 ? 0 : 1)) / days.length) * 100}%` }}
            >
              <p className="font-medium">{longDayFormat.format(dayDate(hovered.day))}</p>
              <dl className="tabular mt-1 grid grid-cols-[auto_auto] gap-x-4 gap-y-0.5">
                <dt className="text-ink-2">{t("Ziyaretçi")}</dt>
                <dd className="text-right font-mono font-medium">{numberFormat.format(hovered.visitors)}</dd>
                <dt className="text-ink-2">{t("Sayfa görüntüleme")}</dt>
                <dd className="text-right font-mono">{numberFormat.format(hovered.pages)}</dd>
                <dt className="text-ink-2">{t("Aktarılan veri")}</dt>
                <dd className="text-right font-mono">{formatBytes(hovered.bytes)}</dd>
              </dl>
            </div>
          )}
        </div>
      </div>
      <div className="mt-1.5 flex justify-between pl-8 text-[12px] text-ink-3" aria-hidden>
        {days.map((day, index) => (labelled.has(index) ? <span key={day.day}>{dayFormat.format(dayDate(day.day))}</span> : null))}
      </div>
    </div>
  );
}

const STATUS_ROWS: { key: "s2xx" | "s3xx" | "s4xx" | "s5xx"; label: string; tone: Tone }[] = [
  { key: "s2xx", label: "Başarılı (2xx)", tone: "ok" },
  { key: "s3xx", label: "Yönlendirme (3xx)", tone: "idle" },
  { key: "s4xx", label: "Bulunamadı ya da reddedildi (4xx)", tone: "warn" },
  { key: "s5xx", label: "Sunucu hatası (5xx)", tone: "bad" },
];

export function StatsSection({ site }: { site: Site }) {
  const [period, setPeriod] = useState(30);
  const stats = useQuery({
    queryKey: ["site-stats", site.id, period],
    queryFn: () => api.get<SiteStats>(`/sites/${site.id}/stats?days=${period}`),
    refetchInterval: 60000,
  });

  if (stats.isPending) return <LoadingBlock />;
  if (stats.isError) return <ErrorBlock message={errorMessage(stats.error)} onRetry={() => void stats.refetch()} />;

  const days = fillDays(stats.data.days, period);
  const sum = (key: keyof SiteDayStats) => days.reduce((total, day) => total + day[key], 0);
  const requests = sum("requests");
  const hasData = requests > 0;

  return (
    <div className="flex flex-col gap-4">
      {/* Dönem seçimi grafiklerin üstünde tek satırda durur ve aşağıdaki her şeyi birlikte değiştirir. */}
      <div role="group" aria-label={t("Dönem")} className="inline-flex self-start rounded-md border border-line-strong bg-surface p-0.5">
        {PERIODS.map((value) => (
          <button
            key={value}
            type="button"
            aria-pressed={period === value}
            onClick={() => setPeriod(value)}
            className={classNames("h-8 rounded px-3 text-[13px] font-medium transition-colors max-sm:h-9", period === value ? "bg-accent-soft text-accent" : "text-ink-2 hover:text-ink")}
          >
            {t("Son {days} gün", { days: value })}
          </button>
        ))}
      </div>

      {!hasData ? (
        <Panel>
          <EmptyState
            title={t("Bu dönemde ziyaret yok")}
            description={t("İstatistikler sitenin erişim günlüklerinden çıkarılır; ilk ziyaretlerden birkaç dakika sonra burada görünür.")}
          />
        </Panel>
      ) : (
        <>
          <Panel>
            <dl className="grid grid-cols-2 gap-x-6 gap-y-4 lg:grid-cols-4">
              <Figure label={t("Ziyaretçi")} value={numberFormat.format(sum("visitors"))} hint={t("günlük tekil ziyaretçilerin toplamı")} />
              <Figure label={t("Sayfa görüntüleme")} value={numberFormat.format(sum("pages"))} />
              <Figure label={t("Aktarılan veri")} value={formatBytes(sum("bytes"))} />
              <Figure label={t("Bu ayki trafik")} value={formatBytes(stats.data.month_bytes)} />
            </dl>
          </Panel>

          <Panel title={t("Günlük ziyaretçi")}>
            <VisitorsChart days={days} />
            {/* Grafikteki değerlerin tablo karşılığı; ekran okuyucular ve renk ayırt edemeyenler için. */}
            <table className="sr-only">
              <caption>{t("Günlük ziyaretçi")}</caption>
              <thead>
                <tr>
                  <th>{t("Gün")}</th>
                  <th>{t("Ziyaretçi")}</th>
                  <th>{t("Sayfa görüntüleme")}</th>
                </tr>
              </thead>
              <tbody>
                {days.map((day) => (
                  <tr key={day.day}>
                    <td>{longDayFormat.format(dayDate(day.day))}</td>
                    <td>{day.visitors}</td>
                    <td>{day.pages}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </Panel>

          <div className="grid items-start gap-4 lg:grid-cols-2">
            <Panel flush title={t("En çok görüntülenen sayfalar")}>
              {stats.data.top_paths.length === 0 ? (
                <p className="px-4 py-6 text-ink-2">{t("Bu dönemde sayfa görüntülemesi yok.")}</p>
              ) : (
                <Table>
                  <thead>
                    <tr>
                      <Th>{t("Sayfa")}</Th>
                      <Th align="right">{t("Görüntüleme")}</Th>
                    </tr>
                  </thead>
                  <tbody>
                    {stats.data.top_paths.map((item) => (
                      <tr key={item.path}>
                        <Td className="max-w-0 w-full">
                          <Mono className="block truncate">{item.path}</Mono>
                        </Td>
                        <Td align="right">{numberFormat.format(item.hits)}</Td>
                      </tr>
                    ))}
                  </tbody>
                </Table>
              )}
            </Panel>

            <Panel title={t("İstekler")}>
              <dl className="flex flex-col gap-2.5">
                {STATUS_ROWS.map((row) => {
                  const count = sum(row.key);
                  return (
                    <div key={row.key} className="flex items-baseline justify-between gap-3">
                      <dt>
                        <Status tone={count === 0 ? "idle" : row.tone}>{t(row.label)}</Status>
                      </dt>
                      <dd className="tabular font-mono">
                        {numberFormat.format(count)}
                        <span className="ml-2 text-ink-3">%{requests === 0 ? 0 : Math.round((count / requests) * 100)}</span>
                      </dd>
                    </div>
                  );
                })}
                <div className="mt-1 flex items-baseline justify-between gap-3 border-t border-line pt-2.5">
                  <dt className="text-ink-2">{t("Botlar ve arama motorları")}</dt>
                  <dd className="tabular font-mono">{numberFormat.format(sum("bots"))}</dd>
                </div>
                <div className="flex items-baseline justify-between gap-3">
                  <dt className="font-medium">{t("Toplam istek")}</dt>
                  <dd className="tabular font-mono font-medium">{numberFormat.format(requests)}</dd>
                </div>
              </dl>
            </Panel>
          </div>
        </>
      )}
    </div>
  );
}
