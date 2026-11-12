// Servis ayrıntısı: bellek ve işlemci geçmişi, yeniden başlama bilgisi. Servisler sayfasında satır açılınca görünür.

import { useQuery } from "@tanstack/react-query";
import { TriangleAlert } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import type { ServiceHistory, ServiceRange } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime, formatPercent } from "../lib/format";
import { t } from "../lib/i18n";
import { Trace } from "./Trace";
import { ErrorBlock, LoadingBlock } from "./ui";

const RANGES: { id: ServiceRange; label: string }[] = [
  { id: "day", label: "Son 24 saat" },
  { id: "week", label: "7 gün" },
  { id: "month", label: "30 gün" },
];

function Row({ name, value, detail, children }: { name: string; value: string; detail: ReactNode; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[9rem_1fr] items-center gap-4 py-2 max-sm:grid-cols-1 max-sm:gap-1">
      <div>
        <p className="text-[13.5px] text-ink-2">{name}</p>
        <p className="tabular font-mono text-[18px] leading-tight font-medium">{value}</p>
        <p className="tabular text-[13px] text-ink-3">{detail}</p>
      </div>
      {children}
    </div>
  );
}

/** Ölçülmemiş (boş) noktalar çizgiyi sıfıra düşürmesin diye bir öncekinin değeriyle doldurulur. */
function filled(values: (number | null)[]): number[] {
  let last = 0;
  return values.map((value) => {
    if (value !== null) last = value;
    return last;
  });
}

function average(values: (number | null)[]): number {
  const present = values.filter((value): value is number => value !== null);
  return present.length === 0 ? 0 : present.reduce((sum, value) => sum + value, 0) / present.length;
}

export function ServiceHistoryPanel({ unit }: { unit: string }) {
  const [range, setRange] = useState<ServiceRange>("day");
  const [cursor, setCursor] = useState<number | null>(null);
  const history = useQuery({
    queryKey: ["service-history", unit, range],
    queryFn: () => api.get<ServiceHistory>(`/system/services/${encodeURIComponent(unit)}/history?range=${range}`),
    refetchInterval: 60000,
  });

  if (history.isPending) return <LoadingBlock />;
  if (history.isError) return <ErrorBlock message={errorMessage(history.error)} onRetry={() => void history.refetch()} />;
  const { points, summary, monitored } = history.data;

  if (!monitored) {
    return <p className="text-ink-2">{t("Bu servis için ölçüm geçmişi tutulmuyor; yalnızca panelin izlediği servislerin (nginx, PHP, veritabanları, posta, Redis, panel) geçmişi kaydedilir.")}</p>;
  }

  const at = cursor === null ? null : points[cursor];
  const memory = points.map((point) => point.memory);
  const cpu = points.map((point) => point.cpu);
  const failedCount = points.filter((point) => point.failed).length;
  const shared = { slots: points.length, cursor, onHover: setCursor };
  const hasMemory = memory.some((value) => value !== null);
  const hasCpu = cpu.some((value) => value !== null);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex flex-col gap-1">
          {summary && (
            <p className="font-medium">
              {summary.restarts_7d === 0
                ? t("Son 7 günde yeniden başlamadı")
                : t("Son 7 günde {count} kez yeniden başladı", { count: summary.restarts_7d })}
              <span className="font-normal text-ink-3">
                {" "}
                · {t("son 24 saatte {count}", { count: summary.restarts_24h })}
              </span>
            </p>
          )}
          {summary?.leak_suspect && summary.trend && (
            <p className="inline-flex items-center gap-1.5 text-[13.5px] text-warn">
              <TriangleAlert aria-hidden className="size-4" />
              {t("Bellek {days} gündür düzenli büyüyor (sızıntı şüphesi): yaklaşık {size} arttı.", {
                days: Math.round(summary.trend.span_days),
                size: formatBytes(summary.trend.growth_bytes, 0),
              })}
            </p>
          )}
        </div>
        <div role="group" aria-label={t("Aralık")} className="inline-flex rounded-md border border-line-strong p-0.5">
          {RANGES.map((item) => (
            <button
              key={item.id}
              type="button"
              aria-pressed={range === item.id}
              onClick={() => {
                setCursor(null);
                setRange(item.id);
              }}
              className={classNames("h-7 rounded px-2.5 text-[13.5px] font-medium transition-colors max-sm:h-9", range === item.id ? "bg-accent-soft text-accent" : "text-ink-2 hover:text-ink")}
            >
              {t(item.label)}
            </button>
          ))}
        </div>
      </div>

      {points.length < 2 ? (
        <p className="py-4 text-ink-2">{t("Bu aralık için henüz yeterli veri yok. Panel servisleri dakikada bir ölçer; grafik zamanla dolar.")}</p>
      ) : (
        <>
          {hasMemory ? (
            <Row
              name={t("Bellek")}
              value={formatBytes(at?.memory ?? average(memory), 0)}
              detail={at ? " " : t("ortalama · en yüksek {peak}", { peak: formatBytes(Math.max(...memory.map((value) => value ?? 0)), 0) })}
            >
              <Trace label={t("Servis bellek kullanımı")} values={filled(memory)} color="--ok" {...shared} />
            </Row>
          ) : (
            <p className="text-[13.5px] text-ink-3">{t("Bu servis için bellek ölçümü alınamıyor (systemd bellek muhasebesi kapalı olabilir).")}</p>
          )}
          {hasCpu ? (
            <Row
              name={t("İşlemci")}
              value={formatPercent(at?.cpu ?? average(cpu), 1)}
              detail={at ? " " : t("ortalama · en yüksek {peak}", { peak: formatPercent(Math.max(...cpu.map((value) => value ?? 0)), 1) })}
            >
              <Trace label={t("Servis işlemci kullanımı")} values={filled(cpu)} {...shared} />
            </Row>
          ) : (
            <p className="text-[13.5px] text-ink-3">{t("Bu servis için işlemci ölçümü alınamıyor (systemd işlemci muhasebesi kapalı olabilir).")}</p>
          )}
          <div className="tabular flex flex-wrap justify-between gap-x-4 gap-y-1 text-[13px] text-ink-3">
            <span>{formatDateTime(points[0].ts)}</span>
            <span className={at ? "font-medium text-ink" : undefined}>
              {at ? `${formatDateTime(at.ts)} · ${t("{count} görev", { count: Math.round(at.tasks ?? 0) })}${at.restarts > 0 ? ` · ${t("{count} yeniden başlama", { count: at.restarts })}` : ""}` : t("Bir ana bakmak için grafiğin üzerinde gezin")}
            </span>
            <span>{formatDateTime(points[points.length - 1].ts)}</span>
          </div>
          {failedCount > 0 && (
            <p className="text-[13.5px] text-bad">{t("Bu aralıkta servis {count} noktada hata durumunda görüldü.", { count: failedCount })}</p>
          )}
        </>
      )}
    </div>
  );
}
