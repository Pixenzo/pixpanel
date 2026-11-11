// Sitenin erişilebilirlik geçmişi: panel her dakika siteyi yoklar; son 24 saat
// burada bir şerit olarak, yüzdeler ve yanıt süresiyle birlikte gösterilir.

import { useQuery } from "@tanstack/react-query";
import type { SiteUptime, UptimeBucket } from "../lib/api";
import { api } from "../lib/api";
import { classNames, formatTime } from "../lib/format";
import { t } from "../lib/i18n";
import { Panel, Skeleton } from "./ui";

/** Şeritteki çubuk sayısı; sunucudan gelen 5 dakikalık dilimler bu sayıya toplanır. */
const BARS = 72;

interface Bar {
  at: number;
  checks: number;
  failures: number;
  millis: number;
}

function toBars(buckets: UptimeBucket[]): Bar[] {
  const size = Math.max(1, Math.ceil(buckets.length / BARS));
  const bars: Bar[] = [];
  for (let index = 0; index < buckets.length; index += size) {
    const group = buckets.slice(index, index + size);
    const checks = group.reduce((sum, bucket) => sum + bucket.checks, 0);
    const failures = group.reduce((sum, bucket) => sum + bucket.failures, 0);
    const timed = group.filter((bucket) => bucket.checks > bucket.failures);
    const millis = timed.length > 0 ? Math.round(timed.reduce((sum, bucket) => sum + bucket.millis, 0) / timed.length) : 0;
    bars.push({ at: group[0].at, checks, failures, millis });
  }
  return bars;
}

function barTone(bar: Bar): string {
  if (bar.checks === 0) return "bg-sunken";
  if (bar.failures === 0) return "bg-ok";
  return bar.failures === bar.checks ? "bg-bad" : "bg-warn";
}

function barTitle(bar: Bar): string {
  const time = formatTime(bar.at);
  if (bar.checks === 0) return t("{time}: veri yok", { time });
  if (bar.failures === 0) return t("{time}: sorunsuz, ortalama {millis} ms", { time, millis: bar.millis });
  return t("{time}: {failures} / {checks} yoklama başarısız", { time, failures: bar.failures, checks: bar.checks });
}

function percentText(value: number | null): string {
  return value === null ? "—" : `%${value.toLocaleString("tr-TR", { maximumFractionDigits: 1 })}`;
}

function Figure({ label, value, tone }: { label: string; value: string; tone?: string }) {
  return (
    <div>
      <dt className="text-[13px] text-ink-3">{label}</dt>
      <dd className={classNames("tabular font-mono text-[18px] leading-tight font-medium", tone)}>{value}</dd>
    </div>
  );
}

export function UptimePanel({ siteId }: { siteId: number }) {
  const uptime = useQuery({
    queryKey: ["site-uptime", siteId],
    queryFn: () => api.get<SiteUptime>(`/sites/${siteId}/uptime`),
    refetchInterval: 60000,
  });

  if (uptime.isPending) {
    return (
      <Panel title={t("Erişilebilirlik")}>
        <Skeleton className="h-16 w-full" />
      </Panel>
    );
  }
  if (uptime.isError) return null;

  const data = uptime.data;
  const hasData = data.buckets.some((bucket) => bucket.checks > 0);
  if (!hasData) {
    return (
      <Panel title={t("Erişilebilirlik")}>
        <p className="text-ink-2">
          {t("Panel siteyi dakikada bir yoklar; ilk sonuçlar birkaç dakika içinde burada görünür. Yoklama, Genel bakış sayfasından kapatılmış olabilir.")}
        </p>
      </Panel>
    );
  }

  const bars = toBars(data.buckets);
  const down = data.last_status !== null && (data.last_status === 0 || data.last_status >= 500);
  const dayTone = data.uptime_day === null ? undefined : data.uptime_day >= 99.9 ? "text-ok" : data.uptime_day >= 99 ? "text-warn" : "text-bad";

  return (
    <Panel
      title={t("Erişilebilirlik")}
      actions={
        <span className={classNames("text-[13.5px] font-medium", down ? "text-bad" : "text-ok")}>
          {down ? t("Şu an yanıt vermiyor") : t("Şu an yayında")}
        </span>
      }
    >
      <dl className="mb-4 flex flex-wrap gap-x-8 gap-y-3">
        <Figure label={t("Son 24 saat")} value={percentText(data.uptime_day)} tone={dayTone} />
        <Figure label={t("Son 7 gün")} value={percentText(data.uptime_week)} />
        <Figure label={t("Ortalama yanıt")} value={data.average_millis === null ? "—" : data.average_millis === 0 ? "<1 ms" : `${data.average_millis} ms`} />
      </dl>
      <div role="img" aria-label={t("Son 24 saatin erişilebilirlik şeridi")} className="flex h-8 items-stretch gap-px">
        {bars.map((bar) => (
          <span key={bar.at} title={barTitle(bar)} className={classNames("min-w-0 flex-1 rounded-[2px]", barTone(bar))} />
        ))}
      </div>
      <div className="mt-1.5 flex justify-between text-[13px] text-ink-3">
        <span>{t("24 saat önce")}</span>
        <span>{t("şimdi")}</span>
      </div>
    </Panel>
  );
}
