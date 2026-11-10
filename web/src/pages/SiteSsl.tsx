import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ChangeEvent, FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import { Button, Checkbox, ErrorBlock, LoadingBlock, Modal, Mono, Panel, Status, Textarea } from "../components/ui";
import type { Site, SslStatus } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

const modeLabels: Record<string, string> = {
  none: "Sertifika yok",
  selfsigned: "Kendinden imzalı",
  custom: "Yüklenen sertifika",
  letsencrypt: "Let's Encrypt",
};

/** Bitiş tarihine kalan süreye göre durum rengi ve metni. */
function expiryView(notAfter: number): { tone: Tone; label: string } {
  const days = Math.floor((notAfter - Date.now() / 1000) / 86400);
  if (days < 0) return { tone: "bad", label: t("Süresi {days} gün önce doldu", { days: -days }) };
  if (days < 15) return { tone: "warn", label: t("{days} gün sonra sona eriyor", { days }) };
  return { tone: "ok", label: t("{days} gün geçerli", { days }) };
}

function CurrentCertificate({ site, status }: { site: Site; status: SslStatus }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const certificate = status.certificate;

  const refresh = (next?: SslStatus) => {
    if (next) queryClient.setQueryData(["site-ssl", site.id], next);
    void queryClient.invalidateQueries({ queryKey: ["site", String(site.id)] });
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
  };

  const options = useMutation({
    mutationFn: (changes: { force_https?: boolean; hsts?: boolean }) => api.patch<Site>(`/sites/${site.id}`, changes),
    onSuccess: () => {
      feedback.success(t("HTTPS ayarları kaydedildi"));
      void queryClient.invalidateQueries({ queryKey: ["site-ssl", site.id] });
      refresh();
    },
    onError: (error) => feedback.error(error),
  });

  const disable = useMutation({
    mutationFn: () => api.delete<SslStatus>(`/sites/${site.id}/ssl`),
    onSuccess: (next) => {
      feedback.success(t("Sertifika kaldırıldı"));
      refresh(next);
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmDisable() {
    const confirmed = await feedback.confirm({
      title: t("Sertifika kaldırılsın mı?"),
      description: t("{domain} yalnızca HTTP üzerinden açılır. Ziyaretçilerin tarayıcıları siteyi güvensiz olarak işaretler.", { domain: site.domain }),
      confirmLabel: t("Sertifikayı kaldır"),
      danger: true,
    });
    if (confirmed) disable.mutate();
  }

  if (status.mode === "none" || !certificate) {
    return (
      <Panel title={t("Geçerli sertifika")}>
        <p className="text-ink-2">{t("Bu sitede henüz sertifika yok; yalnızca HTTP üzerinden açılıyor. Aşağıdaki yollardan biriyle sertifika ekleyin.")}</p>
      </Panel>
    );
  }

  const expiry = expiryView(certificate.not_after);
  return (
    <Panel
      title={t("Geçerli sertifika")}
      actions={
        <Button size="sm" loading={disable.isPending} onClick={() => void confirmDisable()}>
          {t("Sertifikayı kaldır")}
        </Button>
      }
    >
      <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
        <dt className="text-ink-2">{t("Tür")}</dt>
        <dd>{t(modeLabels[status.mode] ?? status.mode)}</dd>
        <dt className="text-ink-2">{t("Geçerlilik")}</dt>
        <dd className="flex flex-wrap items-center gap-x-3">
          <Status tone={expiry.tone}>{expiry.label}</Status>
          <span className="text-ink-3">{formatDateTime(certificate.not_after)}</span>
        </dd>
        <dt className="text-ink-2">{t("Alan adları")}</dt>
        <dd className="min-w-0 break-words">
          <Mono>{certificate.names.join(", ")}</Mono>
        </dd>
        <dt className="text-ink-2">{t("Veren kurum")}</dt>
        <dd className="min-w-0 break-words">{certificate.self_signed ? t("Kendinden imzalı") : certificate.issuer}</dd>
      </dl>
      {status.mode === "letsencrypt" && (
        <p className="mt-3 text-[13px] text-ink-3">{t("Bitimine 30 gün kala kendiliğinden yenilenir.")}</p>
      )}
      {status.mode === "selfsigned" && (
        <p className="mt-3 text-[13px] text-ink-3">
          {t("Tarayıcılar kendinden imzalı sertifikaya güvenmez ve uyarı gösterir. Sınama ortamları ya da Cloudflare gibi bir vekil arkasındaki siteler için uygundur.")}
        </p>
      )}
      <div className="mt-4 flex flex-col gap-3 border-t border-line pt-4">
        <Checkbox
          label={t("HTTP isteklerini HTTPS'e yönlendir")}
          checked={status.force_https}
          disabled={options.isPending}
          onChange={(event) => options.mutate({ force_https: event.target.checked })}
        />
        <Checkbox
          label={t("Tarayıcılara yalnızca HTTPS kullanmalarını bildir (HSTS)")}
          hint={t("Açtıktan sonra ziyaretçilerin tarayıcıları bir yıl boyunca siteye HTTP ile bağlanmaz. Sertifikanızın kalıcı olduğundan emin olun.")}
          checked={status.hsts}
          disabled={options.isPending}
          onChange={(event) => options.mutate({ hsts: event.target.checked })}
        />
      </div>
    </Panel>
  );
}

function LetsEncryptPanel({ site, mode }: { site: Site; mode: SslStatus["mode"] }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [includeAliases, setIncludeAliases] = useState(true);
  const [jobId, setJobId] = useState<number | null>(null);

  const issue = useMutation({
    mutationFn: () => api.post<{ job_id: number }>(`/sites/${site.id}/ssl/letsencrypt`, { include_aliases: includeAliases }),
    onSuccess: (result) => setJobId(result.job_id),
    onError: (error) => feedback.error(error),
  });

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["site-ssl", site.id] });
    void queryClient.invalidateQueries({ queryKey: ["site", String(site.id)] });
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
  }

  return (
    <Panel title="Let's Encrypt">
      <div className="flex max-w-[68ch] flex-col gap-3">
        <p className="text-ink-2">
          {t("Tarayıcıların güvendiği ücretsiz sertifika. Alan adının DNS kaydı bu sunucuyu göstermeli ve 80. port internetten erişilebilir olmalı.")}
        </p>
        {site.aliases.length > 0 && (
          <Checkbox
            label={t("Takma adları da sertifikaya ekle")}
            hint={site.aliases.join(", ")}
            checked={includeAliases}
            onChange={(event) => setIncludeAliases(event.target.checked)}
          />
        )}
        <div>
          <Button variant="primary" loading={issue.isPending} onClick={() => issue.mutate()}>
            {mode === "letsencrypt"
              ? t("Sertifikayı şimdi yenile")
              : mode === "none"
                ? t("Let's Encrypt sertifikası al")
                : t("Let's Encrypt sertifikasına geç")}
          </Button>
        </div>
      </div>
      <Modal
        open={jobId !== null}
        onClose={() => {
          setJobId(null);
          refresh();
        }}
        title={t("{domain} için sertifika alınıyor", { domain: site.domain })}
        wide
      >
        {jobId !== null && <JobLog jobId={jobId} onDone={refresh} />}
      </Modal>
    </Panel>
  );
}

function SelfSignedPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const generate = useMutation({
    mutationFn: () => api.post<SslStatus>(`/sites/${site.id}/ssl/self-signed`),
    onSuccess: (next) => {
      queryClient.setQueryData(["site-ssl", site.id], next);
      void queryClient.invalidateQueries({ queryKey: ["site", String(site.id)] });
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      feedback.success(t("Kendinden imzalı sertifika kuruldu"));
    },
    onError: (error) => feedback.error(error),
  });
  return (
    <Panel title={t("Kendinden imzalı sertifika")}>
      <div className="flex max-w-[68ch] flex-col gap-3">
        <p className="text-ink-2">{t("Hemen üretilir, bir yıl geçerlidir ve kendiliğinden yenilenir. Bağlantıyı şifreler ama tarayıcılar uyarı gösterir.")}</p>
        <div>
          <Button loading={generate.isPending} onClick={() => generate.mutate()}>
            {t("Kendinden imzalı sertifika üret")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

function CustomPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [certificate, setCertificate] = useState("");
  const [privateKey, setPrivateKey] = useState("");

  const upload = useMutation({
    mutationFn: () => api.post<SslStatus>(`/sites/${site.id}/ssl/custom`, { certificate, private_key: privateKey }),
    onSuccess: (next) => {
      queryClient.setQueryData(["site-ssl", site.id], next);
      void queryClient.invalidateQueries({ queryKey: ["site", String(site.id)] });
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      feedback.success(t("Sertifika kuruldu"));
      setCertificate("");
      setPrivateKey("");
    },
  });

  /** Seçilen dosyanın içeriğini ilgili alana aktarır; dosya sunucuya ayrıca gönderilmez. */
  const readFile = (setter: (text: string) => void) => async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) setter(await file.text());
    event.target.value = "";
  };

  function submit(event: FormEvent) {
    event.preventDefault();
    upload.mutate();
  }

  const fileInput = "text-[13px] text-ink-2 file:mr-3 file:h-7 file:rounded-md file:border file:border-line-strong file:bg-surface file:px-2.5 file:text-[13px] file:font-medium file:text-ink";

  return (
    <Panel title={t("Kendi sertifikanızı yükleyin")}>
      <form onSubmit={submit} className="flex max-w-2xl flex-col gap-4">
        <p className="text-ink-2">{t("Bir sertifika sağlayıcısından satın aldığınız sertifikayı ve özel anahtarını PEM biçiminde girin.")}</p>
        <div className="flex flex-col gap-2">
          <Textarea
            label={t("Sertifika")}
            value={certificate}
            onChange={(event) => setCertificate(event.target.value)}
            hint={t("Varsa ara sertifikaları da kendi sertifikanızın altına ekleyin.")}
            placeholder={"-----BEGIN CERTIFICATE-----"}
            spellCheck={false}
            required
          />
          <input type="file" aria-label={t("Sertifika dosyası seç")} accept=".pem,.crt,.cer,.txt" onChange={readFile(setCertificate)} className={fileInput} />
        </div>
        <div className="flex flex-col gap-2">
          <Textarea
            label={t("Özel anahtar")}
            value={privateKey}
            onChange={(event) => setPrivateKey(event.target.value)}
            placeholder={"-----BEGIN PRIVATE KEY-----"}
            spellCheck={false}
            required
          />
          <input type="file" aria-label={t("Özel anahtar dosyası seç")} accept=".pem,.key,.txt" onChange={readFile(setPrivateKey)} className={fileInput} />
        </div>
        {upload.isError && <ErrorBlock message={errorMessage(upload.error)} />}
        <div>
          <Button type="submit" loading={upload.isPending}>
            {t("Sertifikayı kur")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

export function SslTab({ site }: { site: Site }) {
  const status = useQuery({
    queryKey: ["site-ssl", site.id],
    queryFn: () => api.get<SslStatus>(`/sites/${site.id}/ssl`),
  });
  if (status.isPending) return <LoadingBlock />;
  if (status.isError) return <ErrorBlock message={errorMessage(status.error)} onRetry={() => void status.refetch()} />;
  return (
    <div className="flex flex-col gap-4">
      <CurrentCertificate site={site} status={status.data} />
      <LetsEncryptPanel site={site} mode={status.data.mode} />
      <SelfSignedPanel site={site} />
      <CustomPanel site={site} />
    </div>
  );
}
