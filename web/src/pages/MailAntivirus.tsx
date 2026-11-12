// İstenmeyen posta sekmesindeki virüs taraması kartı: ClamAV bileşeninin durumu, imza veritabanının
// tarihi ve son 7 günde yakalanan virüslü ileti sayısı.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw } from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { Badge, Button, ErrorBlock, LoadingBlock, Mono, Panel, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";

interface AntivirusStatus {
  installed: boolean;
  running: boolean;
  updater_running: boolean;
  /** Rspamd virüs taramasına bağlı mı? */
  enabled: boolean;
  engine_version: string | null;
  signature_version: number | null;
  /** İmza veritabanının tarihi (Unix saniyesi). */
  signatures_at: number | null;
  caught_7d: number | null;
  memory_mb: number | null;
  memory_low: boolean;
}

/** İmza veritabanı bu süreden eskiyse uyarılır (güncelleyici günde birkaç kez çalışır). */
const STALE_SIGNATURE_SECS = 3 * 24 * 3600;

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-ink-2">{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

export function AntivirusCard() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const status = useQuery({ queryKey: ["mail-antivirus"], queryFn: () => api.get<AntivirusStatus>("/mail/antivirus"), refetchInterval: 60_000 });
  const apply = useMutation({
    mutationFn: () => api.post<AntivirusStatus>("/mail/antivirus/apply"),
    onSuccess: (data) => {
      queryClient.setQueryData(["mail-antivirus"], data);
      feedback.success(t("Virüs taraması yapılandırması yenilendi"));
    },
    onError: (error) => feedback.error(error),
  });

  if (status.isPending) return <LoadingBlock />;
  if (status.isError) return <ErrorBlock message={errorMessage(status.error)} onRetry={() => void status.refetch()} />;
  const data = status.data;

  if (!data.installed) {
    return (
      <Panel title={t("Virüs taraması (ClamAV)")} actions={<Badge>{t("Kapalı")}</Badge>}>
        <p className="max-w-[75ch] text-ink-2">
          {t("ClamAV kurulunca gelen iletiler virüs için taranır ve virüslü olanlar reddedilir. İmza veritabanı bellekte yaklaşık 1,2 GB yer tutar; en az 2 GB belleği olan sunucularda kurun.")}
        </p>
        {data.memory_low && data.memory_mb !== null && (
          <p className="mt-3 rounded-md bg-warn-soft px-3 py-2.5 text-[13.5px] text-warn">
            {t("Bu sunucuda {mb} MB bellek var; ClamAV için yetersiz olabilir.", { mb: data.memory_mb })}
          </p>
        )}
        <p className="mt-4">
          <Link to="/components" className="font-medium text-accent hover:underline">
            {t("Bileşenler sayfasından kurun")}
          </Link>
        </p>
      </Panel>
    );
  }

  const stale = data.signatures_at !== null && Date.now() / 1000 - data.signatures_at > STALE_SIGNATURE_SECS;
  const active = data.running && data.enabled;
  return (
    <Panel
      title={t("Virüs taraması (ClamAV)")}
      actions={
        <>
          {active ? <Status tone="ok">{t("Açık")}</Status> : <Status tone="warn">{t("Çalışmıyor")}</Status>}
          {!data.enabled && (
            <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={apply.isPending} onClick={() => apply.mutate()}>
              {t("Rspamd'ye bağla")}
            </Button>
          )}
        </>
      }
    >
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
        <Row label={t("Tarayıcı (clamd)")}>{data.running ? <Status tone="ok">{t("Çalışıyor")}</Status> : <Status tone="bad">{t("Durdu")}</Status>}</Row>
        <Row label={t("İmza güncelleyici")}>{data.updater_running ? <Status tone="ok">{t("Çalışıyor")}</Status> : <Status tone="warn">{t("Durdu")}</Status>}</Row>
        <Row label={t("Rspamd bağlantısı")}>{data.enabled ? <Status tone="ok">{t("Virüslü iletiler reddediliyor")}</Status> : <Status tone="warn">{t("Bağlı değil")}</Status>}</Row>
        <Row label={t("Sürüm")}>
          <Mono>{data.engine_version ?? "—"}</Mono>
        </Row>
        <Row label={t("İmza veritabanı")}>
          {data.signatures_at === null ? (
            <span className="text-ink-3">—</span>
          ) : (
            <span className="inline-flex flex-wrap items-center gap-x-2">
              <Mono>{data.signature_version ?? ""}</Mono>
              <span className="text-ink-2">{formatDateTime(data.signatures_at)}</span>
              {stale && <Badge tone="warn">{t("Güncel değil")}</Badge>}
            </span>
          )}
        </Row>
        <Row label={t("Son 7 günde yakalanan")}>
          {data.caught_7d === null ? <span className="text-ink-3">—</span> : <span className="tabular font-medium">{data.caught_7d}</span>}
        </Row>
      </dl>
      <p className="mt-4 max-w-[75ch] text-[13px] text-ink-3">
        {t("Sayı Rspamd'nin sınırlı geçmişinden okunur. Karantina kipi açıksa virüslü iletiler de karantinaya alınır; kapalıysa SMTP aşamasında reddedilir.")}
      </p>
    </Panel>
  );
}
