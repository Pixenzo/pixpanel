// MariaDB yavaş sorgu günlüğü (yalnızca yönetici): aç/kapa, eşik ve son kayıtlar.
// Günlük bütün hesapların sorgularını içerdiğinden müşteriler bu bölümü görmez.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, LoadingBlock, Mono, Panel, Select, Table, Td, Th } from "../components/ui";
import type { SlowQueryLog } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";

export function SlowQueries() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  // Kullanıcının seçtiği eşik; seçmediyse sunucudaki değer gösterilir.
  const [chosen, setChosen] = useState<number | null>(null);

  const log = useQuery({
    queryKey: ["slow-queries"],
    queryFn: () => api.get<SlowQueryLog>("/databases/slow-log?limit=100"),
    refetchInterval: 15000,
  });

  const save = useMutation({
    mutationFn: (body: { enabled: boolean; threshold: number }) => api.put("/databases/slow-log", body),
    onSuccess: (_, body) => {
      feedback.success(body.enabled ? t("Yavaş sorgu günlüğü açık") : t("Yavaş sorgu günlüğü kapatıldı"));
      setChosen(null);
    },
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["slow-queries"] }),
  });

  if (log.isPending) return <LoadingBlock />;
  if (log.isError) return <ErrorBlock message={errorMessage(log.error)} onRetry={() => void log.refetch()} />;

  const data = log.data;
  const serverThreshold = data.thresholds.includes(Math.round(data.threshold)) ? Math.round(data.threshold) : (data.thresholds[0] ?? 2);
  const threshold = chosen ?? serverThreshold;

  return (
    <div className="flex flex-col gap-5">
      <Panel
        title={t("Yavaş sorgu günlüğü")}
        actions={<Badge tone={data.enabled ? "ok" : "idle"}>{data.enabled ? t("Açık") : t("Kapalı")}</Badge>}
      >
        <p className="mb-4 text-ink-2">
          {t("Eşikten uzun süren sorgular günlüğe yazılır. Günlük bütün hesapların sorgularını içerir; yalnızca yöneticiler görür.")}
        </p>
        <div className="flex flex-wrap items-end gap-3">
          <div className="w-44">
            <Select label={t("Eşik")} value={threshold} onChange={(event) => setChosen(Number(event.target.value))}>
              {data.thresholds.map((seconds) => (
                <option key={seconds} value={seconds}>
                  {t("{n} saniye", { n: seconds })}
                </option>
              ))}
            </Select>
          </div>
          {data.enabled ? (
            <>
              <Button
                variant="primary"
                disabled={threshold === serverThreshold}
                loading={save.isPending}
                onClick={() => save.mutate({ enabled: true, threshold })}
              >
                {t("Eşiği uygula")}
              </Button>
              <Button loading={save.isPending} onClick={() => save.mutate({ enabled: false, threshold })}>
                {t("Kapat")}
              </Button>
            </>
          ) : (
            <Button variant="primary" loading={save.isPending} onClick={() => save.mutate({ enabled: true, threshold })}>
              {t("Aç")}
            </Button>
          )}
        </div>
        {data.enabled && !data.persistent && (
          <p className="mt-4 text-[13.5px] text-warn">{t("Ayar yalnızca çalışan sunucuya uygulandı; MariaDB yeniden başlayınca kaybolur.")}</p>
        )}
        {!data.file_output && (
          <p className="mt-4 text-[13.5px] text-warn">
            {t("Sunucu günlüğü dosyaya yazmıyor; kayıtlar listelenemez. Günlüğü açınca dosya çıktısı da eklenir.")}
          </p>
        )}
      </Panel>

      <Panel
        flush
        title={t("Son yavaş sorgular")}
        actions={
          <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={log.isFetching} onClick={() => void log.refetch()}>
            {t("Yenile")}
          </Button>
        }
      >
        {data.entries.length === 0 ? (
          <EmptyState
            title={t("Henüz yavaş sorgu kaydı yok")}
            description={data.enabled ? t("Eşikten uzun süren bir sorgu çalışınca burada görünür.") : t("Günlüğü açtıktan sonra yavaş sorgular burada görünür.")}
          />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>{t("Zaman")}</Th>
                  <Th align="right">{t("Süre")}</Th>
                  <Th align="right">{t("İncelenen satır")}</Th>
                  <Th>{t("Veritabanı")}</Th>
                  <Th>{t("Kullanıcı")}</Th>
                  <Th className="w-full">{t("Sorgu")}</Th>
                </tr>
              </thead>
              <tbody>
                {data.entries.map((entry, index) => (
                  <tr key={`${entry.time}-${index}`} className="hover:bg-sunken/50">
                    <Td className="whitespace-nowrap text-ink-2">{formatDateTime(entry.time)}</Td>
                    <Td align="right" className="tabular whitespace-nowrap font-mono text-[13.5px]">
                      {t("{n} sn", { n: entry.query_time.toFixed(2) })}
                    </Td>
                    <Td align="right" className="tabular whitespace-nowrap font-mono text-[13.5px]">
                      {entry.rows_examined.toLocaleString()}
                    </Td>
                    <Td>{entry.database ? <Mono>{entry.database}</Mono> : "—"}</Td>
                    <Td>
                      <Mono>{entry.user}</Mono>
                    </Td>
                    <Td>
                      <Mono className="block max-w-[56ch] truncate text-[13px]">
                        <span title={entry.query}>{entry.query}</span>
                      </Mono>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
            <p className="border-t border-line px-4 py-3 text-[13px] text-ink-3 sm:px-6">
              {t("Günlük dosyası: {file} ({size})", { file: data.file, size: formatBytes(data.file_size) })}
            </p>
          </>
        )}
      </Panel>
    </div>
  );
}
