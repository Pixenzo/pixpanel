// Posta sayfası "Gönderim sınırı" sekmesi (yalnızca yönetici): kutu başına saatlik alıcı
// sınırının sunucu geneli varsayılanı ve son günlerde sınıra takılan kutu/alan adları.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, Input, LoadingBlock, Mono, Panel, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";

interface LimitSettings {
  default_limit: number;
  domain_factor: number;
}

interface LimitHit {
  id: number;
  kind: "mailbox" | "domain";
  address: string;
  domain: string;
  limit_value: number;
  hit_at: number;
}

function DefaultLimitForm({ settings }: { settings: LimitSettings }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [value, setValue] = useState(String(settings.default_limit));
  const [localError, setLocalError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (defaultLimit: number) => api.put<LimitSettings>("/mail/limits", { default_limit: defaultLimit }),
    onSuccess: (updated) => {
      queryClient.setQueryData(["mail-limits"], updated);
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      void queryClient.invalidateQueries({ queryKey: ["mail-domain"] });
      feedback.success(t("Gönderim sınırı kaydedildi"));
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    const parsed = Number(value);
    if (value.trim() === "" || !Number.isInteger(parsed) || parsed < 0 || parsed > 1_000_000) {
      setLocalError(t("Sınır, 0 ile 1.000.000 arasında bir tam sayı olmalıdır."));
      return;
    }
    setLocalError(null);
    save.mutate(parsed);
  }

  const domainLimit = Number(value) * settings.domain_factor;
  return (
    <form onSubmit={submit} className="flex max-w-2xl flex-col gap-4">
      <p className="max-w-[75ch] text-[13.5px] text-ink-2">
        {t(
          "Ele geçirilmiş bir kutunun istenmeyen posta göndermesini sınırlamak için her kutunun saatte en fazla kaç alıcıya gönderebileceği sayılır. Sınır aşılınca yeni gönderimler geçici olarak reddedilir; pencere kaydıkça kendiliğinden açılır. Sunucunun içinden (sendmail, PHP mail()) gönderilen postalar bu sınıra dahil değildir.",
        )}
      </p>
      <Input
        label={t("Kutu başına saatlik alıcı sınırı")}
        type="number"
        min={0}
        value={value}
        onChange={(event) => setValue(event.target.value)}
        hint={
          Number(value) > 0
            ? t("Alan adı başına toplam sınır, ayrıca belirlenmediyse bunun {factor} katıdır ({limit}). 0 yazarsanız sınır kalkar.", {
                factor: settings.domain_factor,
                limit: Number.isFinite(domainLimit) ? domainLimit : 0,
              })
            : t("Şu an sınırsız. Kutu ya da alan adı için ayrıca sınır tanımlanabilir.")
        }
        className="max-w-48"
        required
      />
      {localError && <ErrorBlock message={localError} />}
      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div>
        <Button type="submit" variant="primary" loading={save.isPending}>
          {t("Kaydet")}
        </Button>
      </div>
    </form>
  );
}

export function MailLimitsTab() {
  const settings = useQuery({ queryKey: ["mail-limits"], queryFn: () => api.get<LimitSettings>("/mail/limits") });
  const hits = useQuery({ queryKey: ["mail-limit-hits"], queryFn: () => api.get<LimitHit[]>("/mail/limit-hits"), refetchInterval: 30000 });

  return (
    <div className="flex flex-col gap-5">
      <Panel title={t("Saatlik gönderim sınırı")}>
        {settings.isPending ? (
          <LoadingBlock />
        ) : settings.isError ? (
          <ErrorBlock message={errorMessage(settings.error)} onRetry={() => void settings.refetch()} />
        ) : (
          // `key`, kaydetme sonrası formun gelen değerle sıfırlanmasını sağlar.
          <DefaultLimitForm key={settings.data.default_limit} settings={settings.data} />
        )}
      </Panel>

      <Panel flush title={t("Sınıra takılanlar")}>
        {hits.isPending ? (
          <LoadingBlock />
        ) : hits.isError ? (
          <ErrorBlock message={errorMessage(hits.error)} onRetry={() => void hits.refetch()} />
        ) : hits.data.length === 0 ? (
          <EmptyState title={t("Sınıra takılan yok")} description={t("Bir kutu ya da alan adı saatlik sınırına ulaşınca burada listelenir (saat başına en fazla bir kayıt).")} />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Zaman")}</Th>
                <Th>{t("Tür")}</Th>
                <Th>{t("Adres")}</Th>
                <Th align="right">{t("Sınır")}</Th>
              </tr>
            </thead>
            <tbody>
              {hits.data.map((hit) => (
                <tr key={hit.id} className="hover:bg-sunken/50">
                  <Td className="whitespace-nowrap text-ink-2">{formatDateTime(hit.hit_at)}</Td>
                  <Td>{hit.kind === "mailbox" ? <Badge tone="warn">{t("Kutu")}</Badge> : <Badge tone="bad">{t("Alan adı")}</Badge>}</Td>
                  <Td>
                    <Mono className="text-[13.5px]">{hit.address}</Mono>
                    {hit.kind === "mailbox" && (
                      <p className="text-[12.5px] text-ink-3">{hit.domain}</p>
                    )}
                  </Td>
                  <Td align="right" className="tabular">
                    {hit.limit_value}
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
    </div>
  );
}
