// Genel bakış'taki "Kara liste denetimi" kutusu: posta gönderen adreslerin DNSBL durumu.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import type { BlacklistVerdict, BlacklistView } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import type { Tone } from "./ui";
import { Badge, Button, ErrorBlock, LoadingBlock, Mono, Panel } from "./ui";

const KEY = ["blacklist"];

function verdictView(verdict: BlacklistVerdict): { tone: Tone; label: string } {
  switch (verdict.kind) {
    case "listed":
      return { tone: "bad", label: t("Listede") };
    case "clean":
      return { tone: "ok", label: t("Temiz") };
    case "refused":
      return { tone: "warn", label: t("Sorgu reddedildi") };
    default:
      return { tone: "idle", label: t("Bilinmiyor") };
  }
}

export function BlacklistPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const view = useQuery({ queryKey: KEY, queryFn: () => api.get<BlacklistView>("/health/blacklist") });
  const check = useMutation({
    mutationFn: () => api.post<BlacklistView>("/health/blacklist/check"),
    onSuccess: (next) => {
      queryClient.setQueryData(KEY, next);
      void queryClient.invalidateQueries({ queryKey: ["health"] });
    },
    onError: (error) => feedback.error(error),
  });

  if (view.isPending) return <LoadingBlock />;
  if (view.isError) return <ErrorBlock message={errorMessage(view.error)} onRetry={() => void view.refetch()} />;
  const data = view.data;
  const ips = Array.from(new Set(data.entries.map((entry) => entry.ip)));

  return (
    <Panel
      flush
      title={t("Kara liste denetimi")}
      actions={
        data.mail_installed && (
          <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={check.isPending} onClick={() => check.mutate()}>
            {t("Şimdi sorgula")}
          </Button>
        )
      }
    >
      <div className="flex flex-col gap-3 px-4 py-4 sm:px-6">
        {!data.mail_installed ? (
          <p className="text-ink-2">{t("Posta bileşeni kurulu değil; kara liste denetimi yapılmıyor.")}</p>
        ) : data.checked_at === 0 ? (
          <p className="text-ink-2">{t("Henüz sorgulanmadı. Panel posta gönderen adresleri günde bir kez bilinen kara listelerde sorgular.")}</p>
        ) : (
          <>
            {ips.length === 0 && (
              <p className="text-ink-2">{t("Sorgulanacak genel (internetten erişilebilen) IP adresi yok.")}</p>
            )}
            {ips.map((ip) => (
              <div key={ip}>
                <Mono className="font-medium">{ip}</Mono>
                <ul className="mt-1 flex flex-wrap gap-x-4 gap-y-1">
                  {data.entries
                    .filter((entry) => entry.ip === ip)
                    .map((entry) => {
                      const view = verdictView(entry.verdict);
                      return (
                        <li key={entry.list} className="inline-flex items-center gap-1.5 text-[13.5px]">
                          <span className="text-ink-2">{entry.label}</span>
                          <Badge tone={view.tone}>{view.label}</Badge>
                        </li>
                      );
                    })}
                </ul>
              </div>
            ))}
            {data.skipped.length > 0 && (
              <p className="text-[13px] text-ink-3">{t("Genel olmadığı için sorgulanmayan adresler: {list}", { list: data.skipped.join(", ") })}</p>
            )}
            <p className="tabular text-[13px] text-ink-3">{t("Son sorgu: {when}", { when: formatDateTime(data.checked_at) })}</p>
          </>
        )}
      </div>
    </Panel>
  );
}
