// Sitenin WordPress araçları: tek tıkla kurulum, parolasız yönetici girişi,
// güncellemeler, eklentiler/temalar ve bakım kipi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, LogIn, RefreshCw, Trash2, Wand2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Mono, Panel, Select, Status, Switch, Table, Td, Th } from "../components/ui";
import type { Site, WordPressExtension, WordPressStatus } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { generatePassword } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { JobLog } from "./Jobs";
import { SecurityPanel } from "./SiteWordPressSecurity";
import { StagingPanel } from "./SiteStaging";

const LOCALES = [
  { value: "tr_TR", label: "Türkçe" },
  { value: "en_US", label: "English" },
  { value: "de_DE", label: "Deutsch" },
  { value: "fr_FR", label: "Français" },
  { value: "es_ES", label: "Español" },
  { value: "ar", label: "العربية" },
  { value: "ru_RU", label: "Русский" },
];

const AUTO_CORE = [
  { value: "minor", label: "Yalnızca güvenlik ve bakım sürümleri" },
  { value: "all", label: "Bütün yeni sürümler" },
  { value: "off", label: "Kendiliğinden güncelleme" },
];

function InstallForm({ site, onStarted }: { site: Site; onStarted: (jobId: number, credentials: { user: string; password: string }) => void }) {
  const me = useMe();
  const feedback = useFeedback();
  const [title, setTitle] = useState(site.domain);
  const [adminUser, setAdminUser] = useState(site.owner === "admin" ? "yonetici" : site.owner);
  const [adminEmail, setAdminEmail] = useState(me.email);
  const [password, setPassword] = useState(() => generatePassword(20));
  const [locale, setLocale] = useState("tr_TR");

  const install = useMutation({
    mutationFn: () =>
      api.post<{ job_id: number }>(`/sites/${site.id}/wordpress/install`, {
        title,
        admin_user: adminUser,
        admin_email: adminEmail,
        admin_password: password,
        locale,
      }),
    onSuccess: (result) => onStarted(result.job_id, { user: adminUser, password }),
    onError: (error) => feedback.error(error),
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    install.mutate();
  }

  return (
    <Panel title={t("WordPress kur")}>
      <p className="mb-4 max-w-[70ch] text-ink-2">
        {t("Panel WordPress'in son sürümünü indirir, veritabanını ve kullanıcısını açar, ayarları yapar ve siteyi kullanıma hazırlar. Sizin yapmanız gereken tek şey aşağıdaki bilgileri doldurmak.")}
      </p>
      <form onSubmit={submit} className="grid max-w-2xl gap-4 sm:grid-cols-2">
        <div className="sm:col-span-2">
          <Input label={t("Site başlığı")} value={title} onChange={(event) => setTitle(event.target.value)} maxLength={100} required />
        </div>
        <Input
          label={t("Yönetici adı")}
          value={adminUser}
          onChange={(event) => setAdminUser(event.target.value)}
          hint={t("“admin” gibi tahmin edilebilir adlardan kaçının.")}
          autoComplete="off"
          required
        />
        <Input label={t("Yönetici e-postası")} type="email" value={adminEmail} onChange={(event) => setAdminEmail(event.target.value)} required />
        <div className="sm:col-span-2">
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1">
              <Input
                label={t("Yönetici parolası")}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                className="font-mono"
                hint={t("Bu parolayı şimdi kaydedin. Panelden parolasız da giriş yapabilirsiniz.")}
                autoComplete="off"
                required
              />
            </div>
            <Button className="mt-[25px]" icon={<Wand2 className="size-4" />} onClick={() => setPassword(generatePassword(20))}>
              {t("Üret")}
            </Button>
          </div>
        </div>
        <Select label={t("Dil")} value={locale} onChange={(event) => setLocale(event.target.value)}>
          {LOCALES.map((item) => (
            <option key={item.value} value={item.value}>
              {item.label}
            </option>
          ))}
        </Select>
        <div className="flex items-end sm:col-span-2">
          <Button type="submit" variant="primary" loading={install.isPending} className="max-sm:w-full">
            {t("WordPress'i kur")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

/** Kurulum sürerken canlı günlük; bitince giriş bilgilerini hatırlatır. */
function InstallProgress({ jobId, credentials, onClose }: { jobId: number; credentials: { user: string; password: string }; onClose: () => void }) {
  const feedback = useFeedback();
  const [status, setStatus] = useState<string | null>(null);
  return (
    <Panel title={status === "success" ? t("WordPress kuruldu") : status === "failed" ? t("Kurulum tamamlanamadı") : t("WordPress kuruluyor")}>
      <div className="flex flex-col gap-5">
        <JobLog jobId={jobId} onDone={setStatus} />
        {status === "success" && (
          <div className="rounded-lg border border-line bg-sunken/60 p-3">
            <p className="font-medium">{t("Yönetici girişi")}</p>
            <dl className="mt-2 grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-1">
              <dt className="text-ink-2">{t("Kullanıcı adı")}</dt>
              <dd>
                <Mono>{credentials.user}</Mono>
              </dd>
              <dt className="text-ink-2">{t("Parola")}</dt>
              <dd className="flex items-center gap-1">
                <Mono className="break-all">{credentials.password}</Mono>
                <IconButton
                  label={t("Parolayı kopyala")}
                  onClick={() => void navigator.clipboard.writeText(credentials.password).then(() => feedback.success(t("Parola kopyalandı")))}
                >
                  <Copy className="size-4" />
                </IconButton>
              </dd>
            </dl>
            <p className="mt-2 text-[13.5px] text-ink-2">{t("Bu sayfadan çıkınca parola bir daha gösterilmez.")}</p>
          </div>
        )}
        {status && (
          <div>
            <Button variant={status === "success" ? "primary" : "secondary"} onClick={onClose}>
              {status === "success" ? t("Siteyi yönet") : t("Geri dön")}
            </Button>
          </div>
        )}
      </div>
    </Panel>
  );
}

function ExtensionTable({
  site,
  kind,
  items,
  onChanged,
}: {
  site: Site;
  kind: "plugins" | "themes";
  items: WordPressExtension[];
  onChanged: (status: WordPressStatus) => void;
}) {
  const feedback = useFeedback();
  const action = useMutation({
    mutationFn: ({ name, action }: { name: string; action: string }) =>
      api.post<WordPressStatus>(`/sites/${site.id}/wordpress/${kind}/${encodeURIComponent(name)}`, { action }),
    onSuccess: onChanged,
    onError: (error) => feedback.error(error),
  });
  const busy = (name: string, what: string) => action.isPending && action.variables?.name === name && action.variables.action === what;

  async function remove(item: WordPressExtension) {
    const confirmed = await feedback.confirm({
      title: kind === "plugins" ? t("Eklenti silinsin mi?") : t("Tema silinsin mi?"),
      description: t("“{name}” dosyalarıyla birlikte siteden kaldırılır.", { name: item.title || item.name }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) action.mutate({ name: item.name, action: "delete" });
  }

  if (items.length === 0) {
    return <EmptyState title={kind === "plugins" ? t("Eklenti yok") : t("Tema yok")} />;
  }
  return (
    <Table>
      <thead>
        <tr>
          <Th>{kind === "plugins" ? t("Eklenti") : t("Tema")}</Th>
          <Th>{t("Durum")}</Th>
          <Th>{t("Sürüm")}</Th>
          <Th>{t("Kendiliğinden güncelle")}</Th>
          <Th align="right">{t("İşlemler")}</Th>
        </tr>
      </thead>
      <tbody>
        {items.map((item) => {
          const active = item.status === "active" || item.status === "active-network";
          const hasUpdate = item.update === "available";
          // "Olmazsa olmaz" ve "yerine geçen" eklentiler WordPress'in kendi dosyalarıdır; buradan yönetilmez.
          const managed = item.status !== "must-use" && item.status !== "dropin";
          return (
            <tr key={item.name}>
              <Td>
                <span className="font-medium">{item.title || item.name}</span>
              </Td>
              <Td>{active ? <Status tone="ok">{t("Etkin")}</Status> : <Status tone="idle">{managed ? t("Kapalı") : t("Sistem")}</Status>}</Td>
              <Td className="whitespace-nowrap">
                <Mono>{item.version}</Mono>
                {hasUpdate && (
                  <span className="ml-2">
                    <Badge tone="warn">{t("{version} var", { version: item.update_version ?? "" })}</Badge>
                  </span>
                )}
              </Td>
              <Td>
                {managed && (
                  <Switch
                    label={t("{name} kendiliğinden güncellensin", { name: item.title || item.name })}
                    checked={item.auto_update === "on"}
                    disabled={action.isPending}
                    onChange={(next) => action.mutate({ name: item.name, action: next ? "auto-on" : "auto-off" })}
                  />
                )}
              </Td>
              <Td align="right">
                {managed && (
                  <div className="flex flex-wrap items-center justify-end gap-1.5">
                    {hasUpdate && (
                      <Button size="sm" variant="primary" loading={busy(item.name, "update")} onClick={() => action.mutate({ name: item.name, action: "update" })}>
                        {t("Güncelle")}
                      </Button>
                    )}
                    {kind === "plugins" && (
                      <Button
                        size="sm"
                        loading={busy(item.name, active ? "deactivate" : "activate")}
                        onClick={() => action.mutate({ name: item.name, action: active ? "deactivate" : "activate" })}
                      >
                        {active ? t("Kapat") : t("Etkinleştir")}
                      </Button>
                    )}
                    {kind === "themes" && !active && (
                      <Button size="sm" loading={busy(item.name, "activate")} onClick={() => action.mutate({ name: item.name, action: "activate" })}>
                        {t("Etkinleştir")}
                      </Button>
                    )}
                    {!active && (
                      <IconButton label={t("{name} sil", { name: item.title || item.name })} onClick={() => void remove(item)}>
                        <Trash2 className="size-4" />
                      </IconButton>
                    )}
                  </div>
                )}
              </Td>
            </tr>
          );
        })}
      </tbody>
    </Table>
  );
}

function Manage({ site, status, onChanged }: { site: Site; status: WordPressStatus; onChanged: (status: WordPressStatus) => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [updateJob, setUpdateJob] = useState<number | null>(null);
  const pluginUpdates = status.plugins.filter((item) => item.update === "available").length;
  const themeUpdates = status.themes.filter((item) => item.update === "available").length;
  const updates = pluginUpdates + themeUpdates + (status.core_update ? 1 : 0);

  const login = useMutation({
    mutationFn: () => api.post<{ url: string }>(`/sites/${site.id}/wordpress/login`),
    onError: (error) => feedback.error(error),
  });
  const update = useMutation({
    mutationFn: () => api.post<{ job_id: number }>(`/sites/${site.id}/wordpress/update`, { what: "all" }),
    onSuccess: (result) => setUpdateJob(result.job_id),
    onError: (error) => feedback.error(error),
  });
  const settings = useMutation({
    mutationFn: (body: Record<string, unknown>) => api.put<WordPressStatus>(`/sites/${site.id}/wordpress/settings`, body),
    onSuccess: onChanged,
    onError: (error) => feedback.error(error),
  });

  function openAdmin() {
    // Sekme, tıklama anında açılır; adres sonradan verilir. Yoksa tarayıcı açılır pencereyi engeller.
    const tab = window.open("", "_blank");
    login.mutate(undefined, {
      onSuccess: (result) => {
        // Adres sunucuda doğrulanır; yine de http(s) dışındaki bir adres hiçbir koşulda açılmaz.
        if (!/^https?:\/\//i.test(result.url)) {
          tab?.close();
          feedback.error(new Error(t("Giriş adresi geçersiz")));
          return;
        }
        if (tab) {
          tab.opener = null;
          tab.location.href = result.url;
        } else {
          window.location.href = result.url;
        }
      },
      onError: () => tab?.close(),
    });
  }

  return (
    <div className="flex flex-col gap-5">
      <Panel>
        <div className="flex flex-wrap items-start gap-x-6 gap-y-4">
          <div className="min-w-0 flex-1 basis-64">
            <p className="text-[18px] leading-tight font-semibold">{status.title || site.domain}</p>
            <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-ink-2">
              <span>
                WordPress <Mono>{status.version}</Mono>
              </span>
              {status.core_update ? <Badge tone="warn">{t("{version} sürümü var", { version: status.core_update })}</Badge> : <Status tone="ok">{t("Güncel")}</Status>}
              {status.maintenance && <Badge tone="warn">{t("Bakım kipinde")}</Badge>}
            </p>
          </div>
          <div className="flex flex-wrap gap-2 max-sm:w-full max-sm:[&>*]:flex-1">
            {updates > 0 && (
              <Button loading={update.isPending} icon={<RefreshCw className="size-4" />} onClick={() => update.mutate()}>
                {updates === 1 ? t("1 güncellemeyi kur") : t("{count} güncellemeyi kur", { count: updates })}
              </Button>
            )}
            <Button variant="primary" loading={login.isPending} icon={<LogIn className="size-4" />} onClick={openAdmin}>
              {t("Yönetim paneline gir")}
            </Button>
          </div>
        </div>
        {updateJob !== null && (
          <div className="mt-4 border-t border-line pt-4">
            <JobLog
              jobId={updateJob}
              onDone={() => void queryClient.invalidateQueries({ queryKey: ["wordpress", site.id] })}
            />
          </div>
        )}
      </Panel>

      <StagingPanel site={site} />

      <SecurityPanel site={site} />

      <Panel flush title={t("Bakım ve güncelleme")}>
        <ul className="divide-y divide-line">
          <li className="flex items-start gap-3 px-4 sm:px-6 py-3">
            <div className="min-w-0 flex-1">
              <p className="font-medium">{t("Bakım kipi")}</p>
              <p className="mt-0.5 text-[13.5px] text-ink-2">{t("Ziyaretçiler “kısa süreli bakım” iletisi görür; yöneticiler siteyi kullanmayı sürdürür.")}</p>
            </div>
            <Switch label={t("Bakım kipi")} checked={status.maintenance} disabled={settings.isPending} onChange={(next) => settings.mutate({ maintenance: next })} />
          </li>
          <li className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 sm:px-6 py-3">
            <div className="min-w-0 flex-1 basis-56">
              <p className="font-medium">{t("WordPress'in kendini güncellemesi")}</p>
              <p className="mt-0.5 text-[13.5px] text-ink-2">{t("Güvenlik sürümlerinin kendiliğinden kurulması önerilir.")}</p>
            </div>
            <select
              aria-label={t("WordPress'in kendini güncellemesi")}
              value={status.auto_core}
              disabled={settings.isPending}
              onChange={(event) => settings.mutate({ auto_core: event.target.value })}
              className="h-9 rounded-md border border-line-strong bg-surface px-2.5 max-sm:h-10.5 max-sm:w-full max-sm:text-[16px]"
            >
              {AUTO_CORE.map((item) => (
                <option key={item.value} value={item.value}>
                  {item.value === "off" ? t("Kendiliğinden güncellenmesin") : t(item.label)}
                </option>
              ))}
            </select>
          </li>
          <li className="flex flex-wrap items-center gap-2 px-4 sm:px-6 py-3">
            <p className="min-w-0 flex-1 basis-56 text-ink-2">{t("Bütün eklenti ve temaların kendiliğinden güncellenmesini tek seferde açabilirsiniz.")}</p>
            <Button size="sm" loading={settings.isPending} onClick={() => settings.mutate({ auto_plugins: true, auto_themes: true })}>
              {t("Hepsinde aç")}
            </Button>
            <Button size="sm" loading={settings.isPending} onClick={() => settings.mutate({ auto_plugins: false, auto_themes: false })}>
              {t("Hepsinde kapat")}
            </Button>
          </li>
        </ul>
      </Panel>

      <Panel flush title={t("Eklentiler")}>
        <ExtensionTable site={site} kind="plugins" items={status.plugins} onChanged={onChanged} />
      </Panel>
      <Panel flush title={t("Temalar")}>
        <ExtensionTable site={site} kind="themes" items={status.themes} onChanged={onChanged} />
      </Panel>
    </div>
  );
}

export function WordPressSection({ site }: { site: Site }) {
  const queryClient = useQueryClient();
  const queryKey = ["wordpress", site.id];
  const [installing, setInstalling] = useState<{ jobId: number; credentials: { user: string; password: string } } | null>(null);
  // Durum, sitede birkaç wp-cli komutu çalıştırılarak toplanır; her değişiklikte yeniden istenmesin.
  const status = useQuery({ queryKey, queryFn: () => api.get<WordPressStatus>(`/sites/${site.id}/wordpress`), staleTime: 30000 });
  const setStatus = (next: WordPressStatus) => queryClient.setQueryData(queryKey, next);

  if (installing) {
    return (
      <InstallProgress
        jobId={installing.jobId}
        credentials={installing.credentials}
        onClose={() => {
          setInstalling(null);
          void queryClient.invalidateQueries({ queryKey });
        }}
      />
    );
  }
  if (status.isPending) return <LoadingBlock />;
  if (status.isError) return <ErrorBlock message={errorMessage(status.error)} onRetry={() => void status.refetch()} />;

  const data = status.data;
  if (data.installed && data.problem) {
    return (
      <Panel title="WordPress">
        <p className="font-medium">{t("WordPress dosyaları var ama site çalışır durumda değil")}</p>
        <p className="mt-1 text-ink-2">{data.problem}</p>
      </Panel>
    );
  }
  if (data.installed) return <Manage site={site} status={data} onChanged={setStatus} />;
  if (!data.database_ready) {
    return (
      <Panel title="WordPress">
        <p className="font-medium">{t("Önce MariaDB kurulmalı")}</p>
        <p className="mt-1 max-w-[65ch] text-ink-2">{t("WordPress verilerini MariaDB veritabanında tutar. Bileşenler sayfasından MariaDB'yi kurduktan sonra buradan tek tıkla kurabilirsiniz.")}</p>
        <Link to="/components" className="mt-3 inline-block font-medium text-accent hover:underline">
          {t("Bileşenler sayfasını aç")}
        </Link>
      </Panel>
    );
  }
  if (!data.can_install) {
    return (
      <Panel title="WordPress">
        <p className="font-medium">{t("Bu sitede WordPress bulunamadı")}</p>
        <p className="mt-1 max-w-[65ch] text-ink-2">
          {t("Site klasöründe başka dosyalar olduğu için tek tıkla kurulum yapılamıyor; var olan dosyaların üzerine yazılmaz. WordPress'i kurmak için klasörü boşaltın ya da yeni bir site açın.")}
        </p>
      </Panel>
    );
  }
  return <InstallForm site={site} onStarted={(jobId, credentials) => setInstalling({ jobId, credentials })} />;
}
