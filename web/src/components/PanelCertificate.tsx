// Panelin kendi sertifikası: Let's Encrypt, yüklenen ya da kendinden imzalı sertifika.
// Panel, posta servisleri (SMTP/IMAP) ve FTP aynı sertifikayı kullanır.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ChangeEvent, FormEvent } from "react";
import { useState } from "react";
import { JobLog } from "../pages/Jobs";
import type { CertInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import type { Tone } from "./ui";
import { Button, ErrorBlock, Input, LoadingBlock, Modal, Mono, Panel, Status, Textarea } from "./ui";

type CertMode = "selfsigned" | "letsencrypt" | "letsencrypt_dns" | "custom";

interface PanelCertificateState {
  /** Hiç ayarlanmadıysa boş dize. */
  hostname: string;
  mode: CertMode;
  certificate: CertInfo | null;
  nginx_installed: boolean;
  dns_managed: boolean;
}

const queryKey = ["panel-certificate"];

const modeLabels: Record<CertMode, string> = {
  selfsigned: "Kendinden imzalı",
  letsencrypt: "Let's Encrypt",
  letsencrypt_dns: "Let's Encrypt (DNS ile doğrulanmış)",
  custom: "Yüklenen sertifika",
};

/** Bitiş tarihine kalan süreye göre durum rengi ve metni. */
function expiryView(notAfter: number): { tone: Tone; label: string } {
  const days = Math.floor((notAfter - Date.now() / 1000) / 86400);
  if (days < 0) return { tone: "bad", label: t("Süresi {days} gün önce doldu", { days: -days }) };
  if (days < 15) return { tone: "warn", label: t("{days} gün sonra sona eriyor", { days }) };
  return { tone: "ok", label: t("{days} gün geçerli", { days }) };
}

const fileInput =
  "text-[13.5px] text-ink-2 file:mr-3 file:h-7 file:rounded-md file:border file:border-line-strong file:bg-surface file:px-2.5 file:text-[13.5px] file:font-medium file:text-ink";

function CertificateForm({ data }: { data: PanelCertificateState }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [hostname, setHostname] = useState(data.hostname);
  const [method, setMethod] = useState<"http" | "dns">(data.mode === "letsencrypt_dns" || !data.nginx_installed ? "dns" : "http");
  const [jobId, setJobId] = useState<number | null>(null);
  const [certificate, setCertificate] = useState("");
  const [privateKey, setPrivateKey] = useState("");

  const name = hostname.trim();
  const noHostname = name === "";
  const unchanged = name === data.hostname;
  const port = window.location.port;
  const current = data.certificate;
  const isLetsEncrypt = data.mode.startsWith("letsencrypt");
  // Kayıtlı alan adı değişmediyse DNS ile doğrulama için bölgenin panelde yönetilmesi gerekir.
  const dnsMissing = method === "dns" && unchanged && !data.dns_managed;
  // Web doğrulaması nginx yoksa kullanılamaz; bu durumda DNS yöntemi seçili sayılır.
  const effectiveMethod = data.nginx_installed ? method : "dns";

  const issue = useMutation({
    mutationFn: () => api.post<{ job_id: number }>("/system/certificate/letsencrypt", { hostname: name, dns: effectiveMethod === "dns" }),
    onSuccess: (result) => setJobId(result.job_id),
    onError: (error) => feedback.error(error),
  });

  const upload = useMutation({
    mutationFn: () => api.post<PanelCertificateState>("/system/certificate/custom", { hostname: name, certificate, private_key: privateKey }),
    onSuccess: (next) => {
      queryClient.setQueryData(queryKey, next);
      feedback.success(t("Sertifika kuruldu. Yeni sertifikayı görmek için sayfayı yenileyin."));
      setCertificate("");
      setPrivateKey("");
    },
  });

  const selfSigned = useMutation({
    mutationFn: () => api.post<PanelCertificateState>("/system/certificate/selfsigned", { hostname: name }),
    onSuccess: (next) => {
      queryClient.setQueryData(queryKey, next);
      feedback.success(t("Kendinden imzalı sertifika kuruldu. Yeni sertifikayı görmek için sayfayı yenileyin."));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmSelfSigned() {
    const confirmed = await feedback.confirm({
      title: t("Kendinden imzalı sertifikaya dönülsün mü?"),
      description: t("Panel, posta servisleri ve FTP kendinden imzalı sertifikayı kullanmaya başlar. Tarayıcılar ve posta istemcileri uyarı gösterir."),
      confirmLabel: t("Kendinden imzalıya dön"),
      danger: true,
    });
    if (confirmed) selfSigned.mutate();
  }

  /** Seçilen dosyanın içeriğini ilgili alana aktarır; dosya sunucuya ayrıca gönderilmez. */
  const readFile = (setter: (text: string) => void) => async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    if (file) setter(await file.text());
    event.target.value = "";
  };

  function refresh() {
    void queryClient.invalidateQueries({ queryKey });
  }

  function submitUpload(event: FormEvent) {
    event.preventDefault();
    upload.mutate();
  }

  const expiry = current ? expiryView(current.not_after) : null;
  const methods = [
    {
      id: "http" as const,
      title: t("Web üzerinden doğrula"),
      hint: data.nginx_installed
        ? t("Alan adının DNS kaydı bu sunucuyu göstermeli ve 80. port internetten erişilebilir olmalı.")
        : t("Bunun için nginx kurulu olmalı. Önce bir site ya da web sunucusu kurun; şimdilik DNS kaydıyla doğrulayın."),
      disabled: !data.nginx_installed,
      warn: false,
    },
    {
      id: "dns" as const,
      title: t("DNS kaydıyla doğrula"),
      hint: t("Alan adının DNS'i panelde yönetiliyor olmalı: DNS sayfasından bölgeyi ekleyin ya da Cloudflare'e bağlayın."),
      disabled: false,
      warn: dnsMissing,
    },
  ];

  return (
    <div className="flex flex-col gap-6">
      <div>
        <p className="mb-4 max-w-[70ch] text-ink-2">
          {t("Bu panel, posta servisleri ve FTP aynı sertifikayı kullanır. Panele bir alan adı verip tarayıcıların güvendiği bir sertifika alabilirsiniz.")}
        </p>
        {current && expiry ? (
          <>
            <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
              <dt className="text-ink-2">{t("Tür")}</dt>
              <dd>{t(modeLabels[data.mode] ?? data.mode)}</dd>
              <dt className="text-ink-2">{t("Alan adları")}</dt>
              <dd className="min-w-0 break-words">
                <Mono>{current.names.join(", ")}</Mono>
              </dd>
              <dt className="text-ink-2">{t("Veren kurum")}</dt>
              <dd className="min-w-0 break-words">{current.self_signed ? t("Kendinden imzalı") : current.issuer}</dd>
              <dt className="text-ink-2">{t("Geçerlilik bitişi")}</dt>
              <dd className="flex flex-wrap items-center gap-x-3">
                <span>{formatDateTime(current.not_after)}</span>
                <Status tone={expiry.tone}>{expiry.label}</Status>
              </dd>
            </dl>
            {isLetsEncrypt && <p className="mt-3 text-[13.5px] text-ink-3">{t("Bitimine 30 gün kala kendiliğinden yenilenir.")}</p>}
            {data.mode === "selfsigned" && (
              <p className="mt-3 text-[13.5px] text-ink-3">
                {t("Tarayıcılar kendinden imzalı sertifikaya güvenmez ve uyarı gösterir.")}
              </p>
            )}
          </>
        ) : (
          <p className="text-ink-2">{t("Sertifika bilgisi okunamadı.")}</p>
        )}
      </div>

      <div className="max-w-xl border-t border-line pt-5">
        <Input
          label={t("Panelin alan adı")}
          value={hostname}
          onChange={(event) => setHostname(event.target.value)}
          placeholder="panel.ornek.com"
          spellCheck={false}
          autoComplete="off"
          className="font-mono text-[13.5px]"
          hint={
            t("Bu ad DNS'te bu sunucuyu göstermelidir.") +
            " " +
            (noHostname
              ? t("Paneli https://<alan adı>{port} adresinden açarsınız.", { port: port ? `:${port}` : "" })
              : t("Paneli https://{hostname}{port} adresinden açarsınız.", { hostname: name, port: port ? `:${port}` : "" }))
          }
        />
      </div>

      <div className="flex max-w-[68ch] flex-col gap-3 border-t border-line pt-5">
        <h3 className="font-semibold">Let's Encrypt</h3>
        <fieldset className="flex flex-col gap-2">
          <legend className="sr-only">{t("Doğrulama yöntemi")}</legend>
          {methods.map((option) => (
            <label key={option.id} className={option.disabled ? "flex items-start gap-2.5 opacity-70" : "flex cursor-pointer items-start gap-2.5"}>
              <input
                type="radio"
                name="panel-le-method"
                className="mt-1 accent-(--accent)"
                checked={effectiveMethod === option.id}
                disabled={option.disabled}
                onChange={() => setMethod(option.id)}
              />
              <span>
                <span className="font-medium">{option.title}</span>
                <span className={option.warn ? "block text-[13px] text-warn" : "block text-[13px] text-ink-3"}>{option.hint}</span>
              </span>
            </label>
          ))}
        </fieldset>
        <div>
          <Button variant="primary" loading={issue.isPending} disabled={noHostname} onClick={() => issue.mutate()}>
            {isLetsEncrypt && unchanged ? t("Sertifikayı şimdi yenile") : t("Let's Encrypt sertifikası al")}
          </Button>
        </div>
      </div>

      <form onSubmit={submitUpload} className="flex max-w-2xl flex-col gap-4 border-t border-line pt-5">
        <h3 className="font-semibold">{t("Kendi sertifikanızı yükleyin")}</h3>
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
          <Button type="submit" loading={upload.isPending} disabled={noHostname}>
            {t("Sertifikayı kur")}
          </Button>
        </div>
      </form>

      <div className="flex max-w-[68ch] flex-col gap-3 border-t border-line pt-5">
        <h3 className="font-semibold">{t("Kendinden imzalı sertifika")}</h3>
        <p className="text-ink-2">{t("Hemen üretilir ve bağlantıyı şifreler ama tarayıcılar uyarı gösterir. Alan adı boş bırakılabilir.")}</p>
        <div>
          <Button loading={selfSigned.isPending} onClick={() => void confirmSelfSigned()}>
            {t("Kendinden imzalı sertifikaya dön")}
          </Button>
        </div>
      </div>

      <Modal
        open={jobId !== null}
        onClose={() => {
          setJobId(null);
          refresh();
        }}
        title={t("{domain} için sertifika alınıyor", { domain: name })}
        wide
      >
        {jobId !== null && (
          <JobLog
            jobId={jobId}
            onDone={(status) => {
              refresh();
              if (status === "success") feedback.success(t("Sertifika alındı. Yeni sertifikayı görmek için sayfayı yenileyin."));
            }}
          />
        )}
      </Modal>
    </div>
  );
}

export function PanelCertificate() {
  const state = useQuery({ queryKey, queryFn: () => api.get<PanelCertificateState>("/system/certificate") });

  return (
    <Panel title={t("Panel sertifikası")} className="mb-4">
      {state.isPending && <LoadingBlock />}
      {state.isError && <ErrorBlock message={errorMessage(state.error)} onRetry={() => void state.refetch()} />}
      {state.data && <CertificateForm key={state.data.hostname} data={state.data} />}
    </Panel>
  );
}
