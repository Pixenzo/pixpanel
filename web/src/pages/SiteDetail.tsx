import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, ExternalLink, Plus, Trash2 } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useFeedback } from "../components/feedback";
import {
  Badge,
  Button,
  Checkbox,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Mono,
  Panel,
  Select,
  Status,
  Tabs,
  Textarea,
} from "../components/ui";
import type { PhpSettings, Site, SiteOptions } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { kindLabels, useSiteOptions } from "./Sites";

type TabId = "general" | "php" | "proxy" | "advanced";

/** Sitenin bir bölümünü kaydeden ortak mutasyon. */
function useSaveSite(site: Site, message: string) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (changes: Record<string, unknown>) => api.patch<Site>(`/sites/${site.id}`, changes),
    onSuccess: (updated) => {
      queryClient.setQueryData(["site", String(site.id)], updated);
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      feedback.success(message);
    },
  });
}

function SectionForm({
  title,
  onSubmit,
  saving,
  error,
  children,
}: {
  title: string;
  onSubmit: () => void;
  saving: boolean;
  error: unknown;
  children: ReactNode;
}) {
  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit();
  }
  return (
    <Panel title={title}>
      <form onSubmit={submit} className="flex max-w-xl flex-col gap-4">
        {children}
        {error ? <ErrorBlock message={errorMessage(error)} /> : null}
        <div>
          <Button type="submit" variant="primary" loading={saving}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function GeneralTab({ site }: { site: Site }) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [aliases, setAliases] = useState(site.aliases.join("\n"));
  const [publicDir, setPublicDir] = useState(site.public_dir);
  const save = useSaveSite(site, t("Site ayarları kaydedildi"));
  const status = useSaveSite(site, site.status === "active" ? t("Site askıya alındı") : t("Site yeniden yayında"));

  const remove = useMutation({
    mutationFn: (deleteFiles: boolean) => api.delete(`/sites/${site.id}?delete_files=${deleteFiles}`),
    onSuccess: () => {
      feedback.success(t("{domain} silindi", { domain: site.domain }));
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      void navigate("/sites");
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(deleteFiles: boolean) {
    const confirmed = await feedback.confirm({
      title: t("{domain} silinsin mi?", { domain: site.domain }),
      description: deleteFiles
        ? t("Site yayından kalkar ve klasöründeki bütün dosyalar kalıcı olarak silinir. Bu işlem geri alınamaz.")
        : t("Site yayından kalkar. Dosyaları sunucuda kalır; dilediğinizde elle silebilirsiniz."),
      confirmLabel: deleteFiles ? t("Siteyi ve dosyalarını sil") : t("Siteyi sil"),
      danger: true,
      typeToConfirm: deleteFiles ? site.domain : undefined,
    });
    if (confirmed) remove.mutate(deleteFiles);
  }

  const canSuspend = me.role === "admin" || (me.role === "reseller" && site.user_id !== me.id);
  const root = `/home/${site.owner}/sites/${site.domain}`;

  return (
    <div className="flex flex-col gap-4">
      <Panel title={t("Özet")}>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <dt className="text-ink-2">{t("Tür")}</dt>
          <dd>{site.kind === "php" ? `PHP ${site.php_version}` : t(kindLabels[site.kind])}</dd>
          <dt className="text-ink-2">{t("Sahibi")}</dt>
          <dd>
            <Mono>{site.owner}</Mono>
          </dd>
          {site.kind !== "proxy" && (
            <>
              <dt className="text-ink-2">{t("Klasör")}</dt>
              <dd className="min-w-0 break-all">
                <Mono>{root}</Mono>
              </dd>
            </>
          )}
          <dt className="text-ink-2">{t("Oluşturulma")}</dt>
          <dd>{formatDateTime(site.created_at)}</dd>
        </dl>
      </Panel>

      <SectionForm
        title={t("Adresler")}
        saving={save.isPending}
        error={save.error}
        onSubmit={() =>
          save.mutate({
            aliases: aliases.split(/[\s,]+/).filter(Boolean),
            public_dir: site.kind === "proxy" ? undefined : publicDir,
          })
        }
      >
        <Textarea
          label={t("Takma adlar")}
          value={aliases}
          onChange={(event) => setAliases(event.target.value)}
          hint={t("Bu siteyi açan diğer alan adları; her satıra bir tane yazın.")}
          placeholder={`www.${site.domain}`}
          spellCheck={false}
        />
        {site.kind !== "proxy" && (
          <Input
            label={t("Yayın klasörü")}
            value={publicDir}
            onChange={(event) => setPublicDir(event.target.value)}
            hint={t("Site klasörünün içinde ziyaretçilere açılan alt klasör. Laravel için genellikle app/public gibi bir yol kullanılır.")}
            className="font-mono text-[13px]"
            spellCheck={false}
            required
          />
        )}
      </SectionForm>

      <Panel title={t("Yayın durumu")}>
        <div className="flex flex-col divide-y divide-line">
          {canSuspend && (
            <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
              <div className="max-w-[60ch]">
                <p className="font-medium">{site.status === "active" ? t("Siteyi askıya al") : t("Siteyi yeniden yayınla")}</p>
                <p className="text-[13px] text-ink-2">{t("Askıdaki site ziyaretçilere 503 yanıtı verir; dosyaları ve ayarları korunur.")}</p>
              </div>
              <Button loading={status.isPending} onClick={() => status.mutate({ status: site.status === "active" ? "suspended" : "active" })}>
                {site.status === "active" ? t("Askıya al") : t("Yeniden yayınla")}
              </Button>
            </div>
          )}
          <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
            <div className="max-w-[60ch]">
              <p className="font-medium">{t("Siteyi sil")}</p>
              <p className="text-[13px] text-ink-2">{t("Siteyi yayından kaldırır. Dosyaları saklamayı ya da birlikte silmeyi seçebilirsiniz.")}</p>
            </div>
            <div className="flex gap-2">
              <Button loading={remove.isPending} onClick={() => void confirmRemove(false)}>
                {t("Siteyi sil")}
              </Button>
              <Button variant="danger" loading={remove.isPending} onClick={() => void confirmRemove(true)}>
                {t("Siteyi ve dosyalarını sil")}
              </Button>
            </div>
          </div>
        </div>
      </Panel>
    </div>
  );
}

function PhpTab({ site, options }: { site: Site; options: SiteOptions }) {
  const [version, setVersion] = useState(site.php_version ?? options.php_versions[0] ?? "");
  const [settings, setSettings] = useState<PhpSettings>(site.php_settings);
  const save = useSaveSite(site, t("PHP ayarları kaydedildi"));

  const set = <K extends keyof PhpSettings>(key: K, value: PhpSettings[K]) => setSettings((current) => ({ ...current, [key]: value }));
  const setDirective = (index: number, field: "key" | "value", value: string) =>
    set(
      "custom",
      settings.custom.map((directive, i) => (i === index ? { ...directive, [field]: value } : directive)),
    );

  const versions = options.php_versions.includes(version) || !version ? options.php_versions : [version, ...options.php_versions];

  return (
    <SectionForm
      title={t("PHP")}
      saving={save.isPending}
      error={save.error}
      onSubmit={() => save.mutate({ kind: "php", php_version: version, php_settings: settings })}
    >
      <Select label={t("PHP sürümü")} value={version} onChange={(event) => setVersion(event.target.value)} hint={t("Yalnızca bu site etkilenir; diğer siteler kendi sürümüyle çalışmaya devam eder.")}>
        {versions.map((item) => (
          <option key={item} value={item}>
            PHP {item}
          </option>
        ))}
      </Select>

      <div className="grid gap-4 sm:grid-cols-2">
        <Input label={t("Bellek sınırı")} value={settings.memory_limit} onChange={(e) => set("memory_limit", e.target.value)} hint="memory_limit" className="font-mono text-[13px]" />
        <Input
          label={t("Çalışma süresi (saniye)")}
          type="number"
          min={1}
          value={settings.max_execution_time}
          onChange={(e) => set("max_execution_time", Number(e.target.value))}
          hint="max_execution_time"
        />
        <Input
          label={t("En büyük yükleme boyutu")}
          value={settings.upload_max_filesize}
          onChange={(e) => set("upload_max_filesize", e.target.value)}
          hint="upload_max_filesize"
          className="font-mono text-[13px]"
        />
        <Input label={t("En büyük POST boyutu")} value={settings.post_max_size} onChange={(e) => set("post_max_size", e.target.value)} hint="post_max_size" className="font-mono text-[13px]" />
        <Input
          label={t("En fazla form alanı")}
          type="number"
          min={100}
          value={settings.max_input_vars}
          onChange={(e) => set("max_input_vars", Number(e.target.value))}
          hint="max_input_vars"
        />
        <Input
          label={t("Eşzamanlı süreç sayısı")}
          type="number"
          min={1}
          value={settings.max_children}
          onChange={(e) => set("max_children", Number(e.target.value))}
          hint={t("Aynı anda işlenebilecek PHP isteği")}
        />
      </div>

      <div className="flex flex-col gap-3">
        <Checkbox
          label={t("Hataları sayfada göster")}
          hint={t("Geliştirme sırasında işe yarar; canlı sitede kapalı tutun.")}
          checked={settings.display_errors}
          onChange={(e) => set("display_errors", e.target.checked)}
        />
        <Checkbox
          label={t("Dosya erişimini site klasörüyle sınırla")}
          hint="open_basedir"
          checked={settings.open_basedir}
          onChange={(e) => set("open_basedir", e.target.checked)}
        />
        <Checkbox
          label={t("Komut çalıştıran işlevlere izin ver")}
          hint={t("exec, shell_exec, proc_open gibi işlevler. Yalnızca uygulamanız gerektiriyorsa açın.")}
          checked={settings.allow_exec}
          onChange={(e) => set("allow_exec", e.target.checked)}
        />
      </div>

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-2 text-[13px] font-medium text-ink-2">{t("Ek php.ini yönergeleri")}</legend>
        {settings.custom.map((directive, index) => (
          <div key={index} className="flex items-center gap-2">
            <input
              aria-label={t("Yönerge adı")}
              placeholder="opcache.enable"
              value={directive.key}
              onChange={(e) => setDirective(index, "key", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13px]"
              spellCheck={false}
            />
            <input
              aria-label={t("Değer")}
              placeholder="1"
              value={directive.value}
              onChange={(e) => setDirective(index, "value", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13px]"
              spellCheck={false}
            />
            <IconButton label={t("Yönergeyi kaldır")} onClick={() => set("custom", settings.custom.filter((_, i) => i !== index))}>
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        ))}
        <div>
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => set("custom", [...settings.custom, { key: "", value: "" }])}>
            {t("Yönerge ekle")}
          </Button>
        </div>
      </fieldset>
    </SectionForm>
  );
}

function ProxyTab({ site }: { site: Site }) {
  const [target, setTarget] = useState(site.proxy_target ?? "http://127.0.0.1:3000");
  const [websocket, setWebsocket] = useState(site.proxy_websocket);
  const save = useSaveSite(site, t("Vekil ayarları kaydedildi"));
  return (
    <SectionForm
      title={t("Ters vekil")}
      saving={save.isPending}
      error={save.error}
      onSubmit={() => save.mutate({ kind: "proxy", proxy_target: target, proxy_websocket: websocket })}
    >
      <Input
        label={t("Hedef adres")}
        value={target}
        onChange={(event) => setTarget(event.target.value)}
        hint={t("Ziyaretçi istekleri bu adrese iletilir. Örnek: http://127.0.0.1:3000")}
        className="font-mono text-[13px]"
        spellCheck={false}
        required
      />
      <Checkbox
        label={t("WebSocket bağlantılarını ilet")}
        hint={t("Canlı sohbet, bildirim gibi sürekli bağlantı kullanan uygulamalar için gerekir.")}
        checked={websocket}
        onChange={(event) => setWebsocket(event.target.checked)}
      />
    </SectionForm>
  );
}

function AdvancedTab({ site }: { site: Site }) {
  const [custom, setCustom] = useState(site.nginx_custom);
  const save = useSaveSite(site, t("nginx yönergeleri kaydedildi"));
  return (
    <SectionForm title={t("Özel nginx yönergeleri")} saving={save.isPending} error={save.error} onSubmit={() => save.mutate({ nginx_custom: custom })}>
      <Textarea
        label={t("Yönergeler")}
        value={custom}
        onChange={(event) => setCustom(event.target.value)}
        hint={t("Sitenin server bloğuna eklenir. Kaydetmeden önce sınanır; hatalıysa uygulanmaz ve site çalışmaya devam eder.")}
        placeholder={"location /eski-sayfa {\n    return 301 /yeni-sayfa;\n}"}
        className="min-h-56"
        spellCheck={false}
      />
    </SectionForm>
  );
}

export function SiteDetailPage() {
  const { id } = useParams();
  const me = useMe();
  const [tab, setTab] = useState<TabId>("general");
  const site = useQuery({ queryKey: ["site", id], queryFn: () => api.get<Site>(`/sites/${id}`) });
  const options = useSiteOptions();

  if (site.isPending || options.isPending) return <LoadingBlock />;
  if (site.isError) return <ErrorBlock message={errorMessage(site.error)} />;
  if (options.isError) return <ErrorBlock message={errorMessage(options.error)} />;

  const data = site.data;
  const tabs: { id: TabId; label: string }[] = [
    { id: "general", label: t("Genel") },
    ...(data.kind === "php" ? [{ id: "php" as const, label: "PHP" }] : []),
    ...(data.kind === "proxy" ? [{ id: "proxy" as const, label: t("Ters vekil") }] : []),
    ...(me.role === "admin" ? [{ id: "advanced" as const, label: t("Gelişmiş") }] : []),
  ];
  const activeTab = tabs.some((item) => item.id === tab) ? tab : "general";
  const scheme = data.ssl_mode === "none" ? "http" : "https";

  return (
    <>
      <Link to="/sites" className="mb-3 inline-flex items-center gap-1.5 text-[13px] font-medium text-ink-2 hover:text-ink">
        <ArrowLeft className="size-3.5" /> {t("Siteler")}
      </Link>
      <header className="mb-5 flex flex-wrap items-center gap-x-4 gap-y-2">
        <h1 className="font-mono text-[22px] leading-tight font-medium tracking-[-0.01em] break-all">{data.domain}</h1>
        {data.status === "active" ? <Status tone="ok">{t("Yayında")}</Status> : <Status tone="warn">{t("Askıda")}</Status>}
        <Badge>{data.kind === "php" ? `PHP ${data.php_version}` : t(kindLabels[data.kind])}</Badge>
        <a
          href={`${scheme}://${data.domain}`}
          target="_blank"
          rel="noreferrer"
          className="ml-auto inline-flex items-center gap-1.5 text-[13px] font-medium text-accent hover:underline"
        >
          {t("Siteyi aç")} <ExternalLink className="size-3.5" />
        </a>
      </header>
      <Tabs tabs={tabs} value={activeTab} onChange={setTab} label={t("Site bölümleri")} />
      {/* `key`, kaydetme sonrası gelen yeni değerlerle formların sıfırlanmasını sağlar. */}
      {activeTab === "general" && <GeneralTab key={JSON.stringify(data)} site={data} />}
      {activeTab === "php" && <PhpTab key={JSON.stringify(data.php_settings) + data.php_version} site={data} options={options.data} />}
      {activeTab === "proxy" && <ProxyTab key={data.proxy_target} site={data} />}
      {activeTab === "advanced" && <AdvancedTab key={data.nginx_custom} site={data} />}
    </>
  );
}
