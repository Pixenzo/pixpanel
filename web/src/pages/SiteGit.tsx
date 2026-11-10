// Site çalışma alanının "Git ile yayınlama" bölümü: depo bağlantısı, yayın
// anahtarı, web kancası, yayın betiği ve yayın geçmişi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, History, Rocket, ScrollText } from "lucide-react";
import type { FormEvent } from "react";
import { useEffect, useRef, useState } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import { Button, Checkbox, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Status, Table, Td, Textarea, Th } from "../components/ui";
import type { Deployment, GitConfig, GitView, Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDuration, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

/** Sık kullanılan yayın betikleri; kullanıcı seçtiğini düzenleyebilir. */
const scriptPresets: { id: string; label: string; script: string }[] = [
  {
    id: "laravel",
    label: "Laravel",
    script: [
      "composer install --no-dev --optimize-autoloader --no-interaction",
      "php artisan migrate --force",
      "php artisan config:cache",
      "php artisan route:cache",
      "php artisan view:cache",
      "php artisan queue:restart",
    ].join("\n"),
  },
  { id: "composer", label: "Composer (PHP)", script: "composer install --no-dev --optimize-autoloader --no-interaction" },
  { id: "node", label: "Node.js (derleme)", script: ["npm ci", "npm run build"].join("\n") },
];

const statusView: Record<Deployment["status"], { tone: Tone; label: string }> = {
  running: { tone: "accent", label: "Yayınlanıyor" },
  success: { tone: "ok", label: "Başarılı" },
  failed: { tone: "bad", label: "Başarısız" },
};

const sourceLabels: Record<Deployment["source"], string> = {
  manual: "Elle",
  webhook: "Depoya gönderim",
  rollback: "Eski sürüme dönüş",
};

function CopyField({ label, value, hint, multiline }: { label: string; value: string; hint?: string; multiline?: boolean }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Pano kapalıysa kullanıcı metni elle seçip kopyalayabilir.
    }
  }
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-[13px] font-medium text-ink-2">{label}</p>
      <div className="flex items-start gap-2">
        <code
          className={`min-w-0 flex-1 rounded-md border border-line bg-sunken/60 px-3 py-2 font-mono text-[12.5px] select-all ${multiline ? "break-all" : "truncate"}`}
        >
          {value}
        </code>
        <IconButton label={copied ? t("Kopyalandı") : t("Kopyala")} onClick={() => void copy()} className="mt-0.5">
          {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
        </IconButton>
      </div>
      {hint && <p className="max-w-[75ch] text-[12.5px] text-ink-3">{hint}</p>}
    </div>
  );
}

function DeploymentLog({ siteId, deploymentId, onClose }: { siteId: number; deploymentId: number; onClose: () => void }) {
  const box = useRef<HTMLPreElement>(null);
  const detail = useQuery({
    queryKey: ["deployment", siteId, deploymentId],
    queryFn: () => api.get<{ deployment: Deployment; log: string }>(`/sites/${siteId}/git/deployments/${deploymentId}`),
    refetchInterval: (query) => (query.state.data?.deployment.status === "running" ? 1500 : false),
  });
  // Yayın sürerken yeni satırlar geldikçe en alta inilir.
  useEffect(() => {
    if (box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [detail.data?.log]);

  const deployment = detail.data?.deployment;
  return (
    <Modal open onClose={onClose} title={t("Yayın günlüğü")} wide>
      {detail.isPending && <LoadingBlock />}
      {detail.isError && <ErrorBlock message={errorMessage(detail.error)} />}
      {deployment && (
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
            <Status tone={statusView[deployment.status].tone}>{t(statusView[deployment.status].label)}</Status>
            {deployment.commit_hash && (
              <span className="min-w-0 truncate">
                <Mono className="text-ink-2">{deployment.commit_hash.slice(0, 8)}</Mono> {deployment.commit_subject}
              </span>
            )}
          </div>
          <pre
            ref={box}
            tabIndex={0}
            className="max-h-[52vh] overflow-auto rounded-md border border-line bg-sunken/60 px-3 py-2 font-mono text-[12.5px] leading-relaxed break-all whitespace-pre-wrap"
          >
            {detail.data?.log || t("Günlük henüz boş.")}
          </pre>
        </div>
      )}
    </Modal>
  );
}

function SettingsForm({ site, config, onSaved }: { site: Site; config: GitConfig | null; onSaved: (view: GitView) => void }) {
  const feedback = useFeedback();
  const [repoUrl, setRepoUrl] = useState(config?.repo_url ?? "");
  const [branch, setBranch] = useState(config?.branch ?? "main");
  const [autoDeploy, setAutoDeploy] = useState(config?.auto_deploy ?? true);
  const [script, setScript] = useState(config?.script ?? "");

  const save = useMutation({
    mutationFn: () => api.put<GitView>(`/sites/${site.id}/git`, { repo_url: repoUrl, branch, auto_deploy: autoDeploy, script }),
    onSuccess: (view) => {
      feedback.success(config ? t("Yayın ayarları kaydedildi") : t("Depo bağlandı"));
      onSaved(view);
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <form onSubmit={submit} className="flex max-w-2xl flex-col gap-4">
      <Input
        label={t("Depo adresi")}
        value={repoUrl}
        onChange={(event) => setRepoUrl(event.target.value)}
        placeholder="git@github.com:kullanici/depo.git"
        hint={t("Özel depolar için SSH adresini kullanın; herkese açık depolar için https adresi de olur.")}
        className="font-mono text-[13px]"
        autoCapitalize="none"
        spellCheck={false}
        required
      />
      <Input
        label={t("Dal")}
        value={branch}
        onChange={(event) => setBranch(event.target.value)}
        hint={t("Yalnızca bu dala yapılan gönderimler yayınlanır.")}
        className="max-w-xs font-mono text-[13px]"
        autoCapitalize="none"
        spellCheck={false}
        required
      />
      <Checkbox
        label={t("Depoya gönderim yapıldığında kendiliğinden yayınla")}
        hint={t("Web kancası çağrıldığında yayın başlar. Kapalıyken yalnızca “Şimdi yayınla” düğmesiyle yayınlanır.")}
        checked={autoDeploy}
        onChange={(event) => setAutoDeploy(event.target.checked)}
      />
      <div className="flex flex-col gap-2">
        <Textarea
          label={t("Yayın betiği")}
          value={script}
          onChange={(event) => setScript(event.target.value)}
          hint={t(
            "Dosyalar güncellendikten sonra site klasöründe, sitenin kendi kullanıcısıyla çalışır. Bir komut başarısız olursa yayın orada durur ve başarısız sayılır. “php” komutu sitenin PHP sürümünü kullanır.",
          )}
          placeholder={"composer install --no-dev\nphp artisan migrate --force"}
          className="min-h-40"
          spellCheck={false}
        />
        <div className="flex flex-wrap items-center gap-2 text-[13px]">
          <span className="text-ink-2">{t("Hazır betik:")}</span>
          {scriptPresets.map((preset) => (
            <Button key={preset.id} size="sm" onClick={() => setScript(preset.script)}>
              {t(preset.label)}
            </Button>
          ))}
        </div>
      </div>
      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div>
        <Button type="submit" variant="primary" loading={save.isPending}>
          {config ? t("Değişiklikleri kaydet") : t("Depoyu bağla")}
        </Button>
      </div>
    </form>
  );
}

export function GitSection({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [openLog, setOpenLog] = useState<number | null>(null);
  const queryKey = ["site-git", site.id];
  const git = useQuery({
    queryKey,
    queryFn: () => api.get<GitView>(`/sites/${site.id}/git`),
    refetchInterval: (query) => (query.state.data?.running ? 2000 : 15000),
  });

  const apply = (view: GitView) => {
    queryClient.setQueryData(queryKey, view);
    void queryClient.invalidateQueries({ queryKey: ["sites-summary"] });
  };

  const deploy = useMutation({
    mutationFn: (commit?: string) => api.post<{ deployment?: number; queued?: boolean }>(`/sites/${site.id}/git/deploy`, commit ? { commit } : {}),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
      if (result.deployment) setOpenLog(result.deployment);
      else feedback.success(t("Süren yayın bitince yenisi başlayacak"));
    },
    onError: (error) => feedback.error(error),
  });

  const rotate = useMutation({
    mutationFn: () => api.post<GitView>(`/sites/${site.id}/git/webhook-token`),
    onSuccess: (view) => {
      apply(view);
      feedback.success(t("Web kancası adresi yenilendi"));
    },
    onError: (error) => feedback.error(error),
  });

  const disconnect = useMutation({
    mutationFn: () => api.delete<GitView>(`/sites/${site.id}/git`),
    onSuccess: (view) => {
      apply(view);
      feedback.success(t("Depo bağlantısı kaldırıldı"));
    },
    onError: (error) => feedback.error(error),
  });

  if (git.isPending) return <LoadingBlock />;
  if (git.isError) return <ErrorBlock message={errorMessage(git.error)} onRetry={() => void git.refetch()} />;

  const { config, deployments, running, webhook_path: webhookPath } = git.data;

  if (!config) {
    return (
      <Panel title={t("Bir Git deposu bağlayın")}>
        <p className="mb-5 max-w-[75ch] text-ink-2">
          {t(
            "Depoyu bağladığınızda site klasörü deponun çalışma kopyası olur. Depoya her gönderimde panel değişiklikleri çeker ve yayın betiğinizi çalıştırır; sorun çıkarsa e-posta ya da Telegram ile haber verir.",
          )}
        </p>
        <SettingsForm site={site} config={null} onSaved={apply} />
      </Panel>
    );
  }

  async function confirmRotate() {
    const confirmed = await feedback.confirm({
      title: t("Web kancası adresi yenilensin mi?"),
      description: t("Eski adres çalışmaz olur; depo sağlayıcınızdaki kanca ayarını yeni adresle güncellemeniz gerekir."),
      confirmLabel: t("Adresi yenile"),
      danger: true,
    });
    if (confirmed) rotate.mutate();
  }

  async function confirmDisconnect() {
    const confirmed = await feedback.confirm({
      title: t("Depo bağlantısı kaldırılsın mı?"),
      description: t("Kendiliğinden yayınlama durur, yayın anahtarı ve web kancası silinir. Sitenin dosyaları yerinde kalır."),
      confirmLabel: t("Bağlantıyı kaldır"),
      danger: true,
    });
    if (confirmed) disconnect.mutate();
  }

  async function confirmRollback(deployment: Deployment) {
    const confirmed = await feedback.confirm({
      title: t("Bu sürüme dönülsün mü?"),
      description: t("Site dosyaları “{subject}” sürümüne getirilir ve yayın betiği yeniden çalışır. Veritabanı değişiklikleri geri alınmaz.", {
        subject: deployment.commit_subject || deployment.commit_hash.slice(0, 8),
      }),
      confirmLabel: t("Bu sürüme dön"),
    });
    if (confirmed) deploy.mutate(deployment.commit_hash);
  }

  const latest = deployments[0];
  const liveCommit = deployments.find((item) => item.status === "success")?.commit_hash;
  const webhookUrl = `${window.location.origin}${webhookPath ?? ""}`;

  return (
    <div className="flex flex-col gap-4">
      <Panel>
        <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
          <div className="min-w-0 flex-1">
            <p className="truncate font-mono text-[13.5px] font-medium">{config.repo_url}</p>
            <p className="mt-0.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-ink-2">
              <span>
                {t("Dal")}: <Mono>{config.branch}</Mono>
              </span>
              {latest ? (
                <>
                  <Status tone={statusView[latest.status].tone}>{t(statusView[latest.status].label)}</Status>
                  <span>{t(formatRelative(latest.finished_at ?? latest.started_at))}</span>
                </>
              ) : (
                <span>{t("Henüz yayınlanmadı")}</span>
              )}
            </p>
          </div>
          <Button variant="primary" icon={<Rocket className="size-4" />} loading={deploy.isPending || running} onClick={() => deploy.mutate(undefined)}>
            {running ? t("Yayınlanıyor") : t("Şimdi yayınla")}
          </Button>
        </div>
      </Panel>

      <Panel flush title={t("Yayın geçmişi")}>
        {deployments.length === 0 ? (
          <EmptyState
            title={t("Henüz yayın yapılmadı")}
            description={t("İlk yayını “Şimdi yayınla” ile başlatın; özel depolarda önce aşağıdaki yayın anahtarını deponuza ekleyin.")}
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Durum")}</Th>
                <Th>{t("Sürüm")}</Th>
                <Th>{t("Tetikleyen")}</Th>
                <Th>{t("Zaman")}</Th>
                <Th align="right">{t("İşlemler")}</Th>
              </tr>
            </thead>
            <tbody>
              {deployments.map((deployment) => (
                <tr key={deployment.id} className="hover:bg-sunken/50">
                  <Td className="whitespace-nowrap">
                    <Status tone={statusView[deployment.status].tone}>{t(statusView[deployment.status].label)}</Status>
                  </Td>
                  <Td className="max-w-80">
                    {deployment.commit_hash ? (
                      <>
                        <p className="truncate">{deployment.commit_subject}</p>
                        <p className="truncate text-[12.5px] text-ink-3">
                          <Mono>{deployment.commit_hash.slice(0, 8)}</Mono> · {deployment.commit_author}
                        </p>
                      </>
                    ) : (
                      <span className="text-ink-3">{t("Sürüm alınamadı")}</span>
                    )}
                  </Td>
                  <Td className="whitespace-nowrap text-ink-2">{t(sourceLabels[deployment.source])}</Td>
                  <Td className="whitespace-nowrap text-ink-2">
                    {t(formatRelative(deployment.started_at))}
                    {deployment.finished_at && (
                      <span className="text-ink-3"> · {formatDuration(Math.max(deployment.finished_at - deployment.started_at, 1))}</span>
                    )}
                  </Td>
                  <Td align="right" className="whitespace-nowrap">
                    <div className="flex justify-end gap-1">
                      {deployment.status === "success" && deployment.commit_hash && deployment.commit_hash !== liveCommit && (
                        <IconButton label={t("Bu sürüme dön")} disabled={running} onClick={() => void confirmRollback(deployment)}>
                          <History className="size-4" />
                        </IconButton>
                      )}
                      <IconButton label={t("Günlüğü göster")} onClick={() => setOpenLog(deployment.id)}>
                        <ScrollText className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      <Panel title={t("Depo erişimi")}>
        <div className="flex flex-col gap-5">
          <CopyField
            label={t("Yayın anahtarı")}
            value={config.public_key}
            multiline
            hint={t(
              "Özel depolarda bu açık anahtarı deponuza salt okunur erişimle ekleyin (GitHub: Settings → Deploy keys, GitLab: Settings → Repository → Deploy keys). Anahtar yalnızca bu siteye özeldir.",
            )}
          />
          <div className="flex flex-col gap-2">
            <CopyField
              label={t("Web kancası adresi")}
              value={webhookUrl}
              hint={t(
                "Depo sağlayıcınızda gönderim (push) olayı için bu adresi tanımlayın (GitHub: Settings → Webhooks, içerik türü application/json). Adresi bilen herkes yayın başlatabilir; gizli tutun.",
              )}
            />
            <div>
              <Button size="sm" loading={rotate.isPending} onClick={() => void confirmRotate()}>
                {t("Adresi yenile")}
              </Button>
            </div>
          </div>
        </div>
      </Panel>

      <Panel title={t("Yayın ayarları")}>
        <SettingsForm key={JSON.stringify(config)} site={site} config={config} onSaved={apply} />
      </Panel>

      <Panel title={t("Bildirimler ve bağlantı")}>
        <div className="flex flex-col divide-y divide-line">
          <div className="flex flex-wrap items-center justify-between gap-3 pb-3">
            <p className="max-w-[60ch] text-ink-2">{t("Yayın başarısız olduğunda e-posta ya da Telegram ile haber almak için bildirim ayarlarınızı yapın.")}</p>
            <Link to="/account" className="inline-flex h-9 items-center rounded-md border border-line-strong px-3.5 font-medium hover:bg-sunken">
              {t("Bildirim ayarları")}
            </Link>
          </div>
          <div className="flex flex-wrap items-center justify-between gap-3 pt-3">
            <div className="max-w-[60ch]">
              <p className="font-medium">{t("Depo bağlantısını kaldır")}</p>
              <p className="text-[13px] text-ink-2">{t("Kendiliğinden yayınlama durur. Sitenin dosyaları ve yayın geçmişi silinmez.")}</p>
            </div>
            <Button loading={disconnect.isPending} disabled={running} onClick={() => void confirmDisconnect()}>
              {t("Bağlantıyı kaldır")}
            </Button>
          </div>
        </div>
      </Panel>

      {openLog !== null && <DeploymentLog siteId={site.id} deploymentId={openLog} onClose={() => setOpenLog(null)} />}
    </div>
  );
}
