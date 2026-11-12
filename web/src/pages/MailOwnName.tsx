// "Kendi adıyla bağlan": posta alanı için `mail.<alan>` (ve isteğe bağlı `webmail.<alan>`) adları,
// tek bir Let's Encrypt sertifikası, Postfix/Dovecot SNI ayarı. İstemci ayarları bölümünde durur.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, Checkbox, ErrorBlock, LoadingBlock, Modal, Mono, Panel, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";
import type { MailDomain } from "./Mail";

interface NameView {
  name: string;
  role: "mail" | "autoconfig" | "autodiscover" | "webmail";
  /** Ad şu an bu sunucunun adresine çözülüyor mu? */
  resolves_here: boolean;
  in_certificate: boolean;
}

interface OwnNameStatus {
  enabled: boolean;
  webmail: boolean;
  dns: boolean;
  mail_host: string;
  webmail_host: string;
  names: NameView[];
  certificate: { issuer: string; names: string[]; not_before: number; not_after: number; self_signed: boolean } | null;
  last_error: string;
  issued_at: number | null;
  dns_managed: boolean;
  mail_installed: boolean;
  nginx_installed: boolean;
  webmail_installed: boolean;
  suggested_records: { kind: string; name: string; content: string }[];
}

interface SaveResult {
  job_id: number | null;
  dns_report: string[];
}

const roleLabel: Record<NameView["role"], string> = {
  mail: "Posta sunucusu (IMAP/SMTP)",
  autoconfig: "Thunderbird / K-9 otomatik ayarı",
  autodiscover: "Outlook otomatik ayarı",
  webmail: "Web postası",
};

export function MailOwnNamePanel({ domain }: { domain: MailDomain }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const key = ["mail-own-name", domain.id];
  const status = useQuery({ queryKey: key, queryFn: () => api.get<OwnNameStatus>(`/mail/domains/${domain.id}/own-name`), refetchOnWindowFocus: false });
  const [webmail, setWebmail] = useState<boolean | null>(null);
  const [dns, setDns] = useState<boolean | null>(null);
  const [addDns, setAddDns] = useState(true);
  const [job, setJob] = useState<{ id: number; report: string[] } | null>(null);

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: key });
    void queryClient.invalidateQueries({ queryKey: ["mail-client-settings", domain.id] });
    void queryClient.invalidateQueries({ queryKey: ["mail-domain", String(domain.id)] });
  }

  const save = useMutation({
    mutationFn: (body: { enabled: boolean; webmail?: boolean; dns?: boolean; add_dns?: boolean }) => api.put<SaveResult>(`/mail/domains/${domain.id}/own-name`, body),
    onSuccess: (result, body) => {
      if (result.job_id !== null) setJob({ id: result.job_id, report: result.dns_report });
      else feedback.success(body.enabled ? t("Ayarlar uygulandı") : t("Kendi adıyla bağlanma kapatıldı"));
      refresh();
    },
    onError: (error) => feedback.error(error),
  });
  const renew = useMutation({
    mutationFn: () => api.post<{ job_id: number }>(`/mail/domains/${domain.id}/own-name/renew`),
    onSuccess: (result) => setJob({ id: result.job_id, report: [] }),
    onError: (error) => feedback.error(error),
  });

  if (status.isPending) return <LoadingBlock />;
  if (status.isError) return <ErrorBlock message={errorMessage(status.error)} onRetry={() => void status.refetch()} />;
  const data = status.data;

  const wantWebmail = webmail ?? (data.enabled ? data.webmail : data.webmail_installed && data.nginx_installed);
  const wantDns = dns ?? data.dns;
  const names = data.names.filter((name) => name.role !== "webmail" || wantWebmail);
  const unresolved = names.filter((name) => !name.resolves_here);

  async function disable() {
    const confirmed = await feedback.confirm({
      title: t("{domain} için kendi adıyla bağlanma kapatılsın mı?", { domain: domain.domain }),
      description: t("mail.{domain} sertifikası silinir; posta istemcileri sunucunun genel adına dönmelidir. Kutulara ve iletilere dokunulmaz.", { domain: domain.domain }),
      confirmLabel: t("Kapat"),
      danger: true,
    });
    if (confirmed) save.mutate({ enabled: false });
  }

  return (
    <Panel
      title={t("Kendi adıyla bağlan")}
      actions={
        data.enabled ? (
          data.certificate && !data.last_error ? <Status tone="ok">{t("Açık")}</Status> : data.last_error ? <Status tone="bad">{t("Sertifika alınamadı")}</Status> : <Status tone="warn">{t("Sertifika bekleniyor")}</Status>
        ) : (
          <Badge>{t("Kapalı")}</Badge>
        )
      }
    >
      <p className="mb-4 max-w-[75ch] text-ink-2">
        {t("Posta istemcileri sunucunun genel adı yerine {host} adına bağlanır; adın kendi Let's Encrypt sertifikası olur. Aynı sertifika autoconfig ve autodiscover adlarını da kapsar, istenirse web postası {webmail} adresinden açılır.", {
          host: data.mail_host,
          webmail: data.webmail_host,
        })}
      </p>
      {!data.mail_installed && <ErrorBlock message={t("Posta sunucusu kurulu değil.")} />}

      <ul className="mb-4 flex flex-col divide-y divide-line">
        {names.map((name) => (
          <li key={name.name} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2.5 first:pt-0 last:pb-0">
            <Mono className="text-[13.5px]">{name.name}</Mono>
            <span className="text-[13px] text-ink-3">{t(roleLabel[name.role])}</span>
            <span className="ml-auto flex items-center gap-2">
              {name.resolves_here ? <Status tone="ok">{t("DNS bu sunucuyu gösteriyor")}</Status> : <Status tone="warn">{t("DNS bu sunucuyu göstermiyor")}</Status>}
              {data.enabled && (name.in_certificate ? <Badge tone="ok">{t("Sertifikada")}</Badge> : <Badge>{t("Sertifikada yok")}</Badge>)}
            </span>
          </li>
        ))}
      </ul>

      {unresolved.length > 0 && !data.dns_managed && (
        <div className="mb-4 rounded-md bg-sunken/70 px-3 py-2.5 text-[13.5px] text-ink-2">
          <p className="mb-2">{t("Sertifikanın alınabilmesi için bu adların DNS sağlayıcınızda sunucuyu göstermesi gerekir. Şu kayıtları ekleyin:")}</p>
          <ul className="flex flex-col gap-0.5">
            {data.suggested_records.map((record) => (
              <li key={record.name}>
                <Mono>
                  {record.kind} {record.name} → {record.content}
                </Mono>
              </li>
            ))}
          </ul>
        </div>
      )}

      {data.certificate && (
        <dl className="mb-4 grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <dt className="text-ink-2">{t("Veren")}</dt>
          <dd className="break-all">{data.certificate.issuer}</dd>
          <dt className="text-ink-2">{t("Geçerlilik")}</dt>
          <dd>{t("{date} tarihine kadar", { date: formatDateTime(data.certificate.not_after) })}</dd>
        </dl>
      )}
      {data.last_error && <p className="mb-4 rounded-md bg-bad-soft px-3 py-2.5 text-[13.5px] text-bad">{data.last_error}</p>}

      <div className="flex flex-col gap-3">
        <Checkbox
          label={t("webmail.{domain} adresini de sun", { domain: domain.domain })}
          hint={data.webmail_installed ? t("Web postası bu adresin kökünden açılır (yalnızca HTTPS).") : t("Web postası (Roundcube) kurulu değil; Bileşenler sayfasından kurun.")}
          checked={wantWebmail}
          disabled={!data.webmail_installed || !data.nginx_installed}
          onChange={(event) => setWebmail(event.target.checked)}
        />
        {data.dns_managed ? (
          <>
            <Checkbox
              label={t("Doğrulamayı DNS kaydıyla yap")}
              hint={t("Alan adının DNS'i panelde yönetildiği için 80. porta gerek kalmaz.")}
              checked={wantDns}
              onChange={(event) => setDns(event.target.checked)}
            />
            <Checkbox label={t("Eksik adres kayıtlarını (A) panelin DNS'ine ekle")} checked={addDns} onChange={(event) => setAddDns(event.target.checked)} />
          </>
        ) : !data.nginx_installed ? (
          <p className="text-[13.5px] text-warn">{t("Web üzerinden doğrulama için nginx gerekir; Bileşenler sayfasından kurun ya da alan adının DNS'ini panele ekleyin.")}</p>
        ) : null}
        <div className="flex flex-wrap gap-2">
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={!data.mail_installed || (!wantDns && !data.nginx_installed)}
            onClick={() => save.mutate({ enabled: true, webmail: wantWebmail, dns: wantDns && data.dns_managed, add_dns: addDns })}
          >
            {data.enabled ? t("Uygula") : t("Etkinleştir ve sertifika al")}
          </Button>
          {data.enabled && (
            <>
              <Button loading={renew.isPending} onClick={() => renew.mutate()}>
                {t("Sertifikayı şimdi yenile")}
              </Button>
              <Button variant="ghost" onClick={() => void disable()}>
                {t("Kapat")}
              </Button>
            </>
          )}
        </div>
      </div>

      <Modal
        open={job !== null}
        onClose={() => {
          setJob(null);
          refresh();
        }}
        title={t("Posta sertifikası")}
        wide
      >
        {job && (
          <div className="flex flex-col gap-3">
            {job.report.length > 0 && (
              <ul className="rounded-md border border-line px-3 py-2 font-mono text-[13px] text-ink-2">
                {job.report.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            )}
            <JobLog jobId={job.id} onDone={refresh} />
          </div>
        )}
      </Modal>
    </Panel>
  );
}
