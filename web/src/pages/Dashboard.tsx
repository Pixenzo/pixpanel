import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useState } from "react";
import { Link } from "react-router";
import { Trace } from "../components/Trace";
import { ErrorBlock, LoadingBlock, Meter, Mono, Panel, Table, Td, Th } from "../components/ui";
import type { ArchivePoint, Overview, Process, Sample } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime, formatDuration, formatPercent, formatRate } from "../lib/format";
import { t } from "../lib/i18n";
import { useFallbackInterval } from "../lib/live";

/** 5 dakikalık pencere: 2 saniyede bir örnek. */
const WINDOW_SAMPLES = 150;

function Channel({
  name,
  value,
  detail,
  children,
}: {
  name: string;
  value: string;
  detail: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[9.5rem_1fr] items-center gap-4 border-b border-line px-4 sm:px-6 py-3 last:border-b-0 max-sm:grid-cols-1 max-sm:gap-1">
      <div>
        <p className="text-[13.5px] text-ink-2">{name}</p>
        <p className="tabular font-mono text-[20px] leading-tight font-medium">{value}</p>
        <p className="tabular text-[13px] text-ink-3">{detail}</p>
      </div>
      {children}
    </div>
  );
}

function LiveChannels({ current, history }: { current: Sample; history: Sample[] }) {
  const memoryPercent = current.mem_total > 0 ? (current.mem_used / current.mem_total) * 100 : 0;
  return (
    <>
      <Channel
        name={t("İşlemci")}
        value={formatPercent(current.cpu, 1)}
        detail={t("Yük {a} / {b} / {c}", {
          a: current.load1.toFixed(2),
          b: current.load5.toFixed(2),
          c: current.load15.toFixed(2),
        })}
      >
        <Trace label={t("İşlemci kullanımı")} values={history.map((s) => s.cpu)} max={100} slots={WINDOW_SAMPLES} />
      </Channel>
      <Channel
        name={t("Bellek")}
        value={formatPercent(memoryPercent, 1)}
        detail={`${formatBytes(current.mem_used)} / ${formatBytes(current.mem_total)}`}
      >
        <Trace
          label={t("Bellek kullanımı")}
          values={history.map((s) => (s.mem_total > 0 ? (s.mem_used / s.mem_total) * 100 : 0))}
          max={100}
          color="--ok"
          slots={WINDOW_SAMPLES}
        />
      </Channel>
      <Channel
        name={t("Ağ")}
        value={formatRate(current.net_rx)}
        detail={t("Gönderilen {rate}", { rate: formatRate(current.net_tx) })}
      >
        <Trace
          label={t("Ağ trafiği")}
          values={history.map((s) => s.net_rx)}
          secondary={history.map((s) => s.net_tx)}
          color="--warn"
          slots={WINDOW_SAMPLES}
        />
      </Channel>
    </>
  );
}

type Range = "live" | "day" | "week" | "month" | "year";

const RANGES: { id: Range; label: string }[] = [
  { id: "live", label: "Canlı" },
  { id: "day", label: "Gün" },
  { id: "week", label: "Hafta" },
  { id: "month", label: "Ay" },
  { id: "year", label: "Yıl" },
];

function average(values: number[]): number {
  return values.length === 0 ? 0 : values.reduce((sum, value) => sum + value, 0) / values.length;
}

/** Geçmiş aralığının kanalları. Üç iz aynı zaman eksenini paylaşır; imleç hepsinde birlikte gezer. */
function ArchiveChannels({ range }: { range: Exclude<Range, "live"> }) {
  const [cursor, setCursor] = useState<number | null>(null);
  const archive = useQuery({
    queryKey: ["metrics-archive", range],
    queryFn: () => api.get<ArchivePoint[]>(`/system/metrics/archive?range=${range}`),
    refetchInterval: 60000,
  });
  if (archive.isPending) return <LoadingBlock />;
  if (archive.isError) return <ErrorBlock message={errorMessage(archive.error)} onRetry={() => void archive.refetch()} />;
  const points = archive.data;
  if (points.length < 2) {
    return (
      <p className="px-4 sm:px-6 py-8 text-ink-2">
        {t("Bu aralık için henüz yeterli veri yok. Panel sunucu yükünü dakikada bir kaydeder; grafik zamanla dolar.")}
      </p>
    );
  }
  const at = cursor === null ? null : points[cursor];
  const cpu = points.map((point) => point.cpu);
  const memory = points.map((point) => point.mem);
  const received = points.map((point) => point.net_rx);
  const sent = points.map((point) => point.net_tx);
  const shared = { slots: points.length, cursor, onHover: setCursor };

  return (
    <>
      <Channel
        name={t("İşlemci")}
        value={formatPercent(at ? at.cpu : average(cpu), 1)}
        detail={at ? t("Yük {load}", { load: at.load1.toFixed(2) }) : t("ortalama · en yüksek {peak}", { peak: formatPercent(Math.max(...cpu), 1) })}
      >
        <Trace label={t("İşlemci kullanımı")} values={cpu} max={100} {...shared} />
      </Channel>
      <Channel
        name={t("Bellek")}
        value={formatPercent(at ? at.mem : average(memory), 1)}
        detail={at ? " " : t("ortalama · en yüksek {peak}", { peak: formatPercent(Math.max(...memory), 1) })}
      >
        <Trace label={t("Bellek kullanımı")} values={memory} max={100} color="--ok" {...shared} />
      </Channel>
      <Channel
        name={t("Ağ")}
        value={formatRate(at ? at.net_rx : average(received))}
        detail={at ? t("Gönderilen {rate}", { rate: formatRate(at.net_tx) }) : t("ortalama alınan · en yüksek {peak}", { peak: formatRate(Math.max(...received)) })}
      >
        <Trace label={t("Ağ trafiği")} values={received} secondary={sent} color="--warn" {...shared} />
      </Channel>
      <div className="tabular flex justify-between gap-3 border-t border-line px-4 sm:px-6 py-2 text-[13px] text-ink-3">
        <span>{formatDateTime(points[0].ts)}</span>
        <span className={at ? "font-medium text-ink" : undefined}>{at ? formatDateTime(at.ts) : t("Bir ana bakmak için grafiğin üzerinde gezin")}</span>
        <span>{formatDateTime(points[points.length - 1].ts)}</span>
      </div>
    </>
  );
}

function Disks({ overview }: { overview: Overview }) {
  const swap = overview.current;
  return (
    <Panel title={t("Depolama")}>
      <ul className="flex flex-col gap-5">
        {overview.disks.map((disk) => {
          const percent = (disk.used / disk.total) * 100;
          return (
            <li key={disk.mount} className="flex flex-col gap-1.5">
              <div className="flex items-baseline justify-between gap-3">
                <Mono className="font-medium">{disk.mount}</Mono>
                <span className="tabular text-[13.5px] text-ink-2">
                  {formatBytes(disk.used)} / {formatBytes(disk.total)}
                </span>
              </div>
              <Meter value={percent} label={t("{mount} doluluk", { mount: disk.mount })} />
              <p className="tabular text-[13px] text-ink-3">
                {t("{free} boş, {type}", { free: formatBytes(disk.available), type: disk.fs_type })}
              </p>
            </li>
          );
        })}
        {swap.swap_total > 0 && (
          <li className="flex flex-col gap-1.5">
            <div className="flex items-baseline justify-between gap-3">
              <span className="font-medium">{t("Takas alanı")}</span>
              <span className="tabular text-[13.5px] text-ink-2">
                {formatBytes(swap.swap_used)} / {formatBytes(swap.swap_total)}
              </span>
            </div>
            <Meter value={(swap.swap_used / swap.swap_total) * 100} label={t("Takas alanı doluluk")} />
          </li>
        )}
      </ul>
    </Panel>
  );
}

function HostFacts({ overview }: { overview: Overview }) {
  const { host } = overview;
  const rows: [string, ReactNode][] = [
    [t("İşletim sistemi"), host.os],
    [t("Çekirdek"), <Mono>{host.kernel}</Mono>],
    [t("İşlemci"), t("{model}, {cores} çekirdek", { model: host.cpu_model || t("Bilinmiyor"), cores: host.cpu_cores })],
    [t("Sanallaştırma"), host.virtualization && host.virtualization !== "none" ? host.virtualization : t("Yok (fiziksel)")],
    [t("Çalışma süresi"), formatDuration(host.uptime_secs)],
    [t("Panel sürümü"), <Mono>{host.panel_version}</Mono>],
  ];
  return (
    <Panel title={t("Sunucu")}>
      <dl className="grid grid-cols-[auto_1fr] gap-x-5 gap-y-2">
        {rows.map(([label, value]) => (
          <div key={label} className="contents">
            <dt className="text-ink-2">{label}</dt>
            <dd className="min-w-0 break-words">{value}</dd>
          </div>
        ))}
      </dl>
    </Panel>
  );
}

function TopProcesses() {
  const { data, isPending } = useQuery({
    queryKey: ["processes"],
    queryFn: () => api.get<Process[]>("/system/processes"),
    refetchInterval: 5000,
  });
  const top = data?.slice(0, 6) ?? [];
  return (
    <Panel
      flush
      title={t("En çok kaynak kullanan süreçler")}
      actions={
        <Link to="/processes" className="text-[13.5px] font-medium text-accent hover:underline">
          {t("Tüm süreçler")}
        </Link>
      }
    >
      {isPending && <LoadingBlock />}
      <Table className={isPending ? "hidden" : undefined}>
        <thead>
          <tr>
            <Th>{t("Süreç")}</Th>
            <Th>{t("Kullanıcı")}</Th>
            <Th align="right">{t("İşlemci")}</Th>
            <Th align="right">{t("Bellek")}</Th>
          </tr>
        </thead>
        <tbody>
          {top.map((process) => (
            <tr key={process.pid}>
              <Td className="max-w-0 w-full">
                <span className="block truncate font-mono text-[13px]" title={process.command}>
                  {process.command}
                </span>
              </Td>
              <Td className="whitespace-nowrap">{process.user}</Td>
              <Td align="right">{formatPercent(process.cpu, 1)}</Td>
              <Td align="right" className="whitespace-nowrap">
                {formatBytes(process.memory)}
              </Td>
            </tr>
          ))}
        </tbody>
      </Table>
    </Panel>
  );
}

export function DashboardPage() {
  const [range, setRange] = useState<Range>("live");
  const fallbackInterval = useFallbackInterval(2000);
  const overview = useQuery({
    queryKey: ["overview"],
    queryFn: () => api.get<Overview>("/system/overview"),
    // Anlık yük canlı akıştan gelir; disk ve çalışma süresi için seyrek yenileme yeter.
    refetchInterval: 30000,
  });
  const history = useQuery({
    queryKey: ["metrics", WINDOW_SAMPLES],
    queryFn: () => api.get<Sample[]>(`/system/metrics?count=${WINDOW_SAMPLES}`),
    refetchInterval: fallbackInterval,
  });

  if (overview.isPending) return <LoadingBlock />;
  if (overview.isError) return <ErrorBlock message={errorMessage(overview.error)} onRetry={() => void overview.refetch()} />;

  const samples = history.data ?? [];
  const current = samples.at(-1) ?? overview.data.current;

  return (
    <div className="flex flex-col gap-5">
      <header className="mb-1">
        <h1 className="font-mono text-[22px] leading-tight font-medium tracking-[-0.01em]">{overview.data.host.hostname}</h1>
        <p className="mt-1 text-ink-2">
          {t("{os}, {uptime} süredir açık", {
            os: overview.data.host.os,
            uptime: formatDuration(overview.data.host.uptime_secs),
          })}
        </p>
      </header>
      <Panel
        flush
        title={range === "live" ? t("Canlı durum") : t("Sunucu yükü")}
        actions={
          <div role="group" aria-label={t("Aralık")} className="inline-flex rounded-md border border-line-strong p-0.5">
            {RANGES.map((item) => (
              <button
                key={item.id}
                type="button"
                aria-pressed={range === item.id}
                onClick={() => setRange(item.id)}
                className={classNames("h-7 rounded px-2.5 text-[13.5px] font-medium transition-colors max-sm:h-9", range === item.id ? "bg-accent-soft text-accent" : "text-ink-2 hover:text-ink")}
              >
                {t(item.label)}
              </button>
            ))}
          </div>
        }
      >
        {range === "live" ? <LiveChannels current={current} history={samples} /> : <ArchiveChannels range={range} />}
      </Panel>
      <div className="grid gap-4 lg:grid-cols-2">
        <Disks overview={overview.data} />
        <HostFacts overview={overview.data} />
      </div>
      <TopProcesses />
    </div>
  );
}
