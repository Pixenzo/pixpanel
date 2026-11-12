import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { LucideIcon } from "lucide-react";
import { ArrowLeft, ArrowRightLeft, ChartColumn, ExternalLink, FileCode2, GitBranch, Globe, HardDriveUpload, Hexagon, LayoutGrid, PenLine, Plus, Route, ShieldCheck, Signpost, SlidersHorizontal, Trash2 } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useState } from "react";
import { Link, NavLink, useNavigate, useParams } from "react-router";
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
  Textarea,
} from "../components/ui";
import type { PhpGlobal, PhpSettings, Site, SiteOptions, SiteSummary } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { FtpSection } from "./SiteFtp";
import { GitSection } from "./SiteGit";
import { NodeSection } from "./SiteNode";
import { ProxyRulesSection } from "./SiteProxyRules";
import { CloneRow } from "./SiteClone";
import { MigrateRow } from "./SiteMigrate";
import { SiteRulesSection } from "./SiteRules";
import { selectablePhpVersions, siteKindLabel, SiteToolGrid, useSiteOptions } from "./Sites";
import { UptimePanel } from "../components/UptimePanel";
import { StatsSection } from "./SiteStats";
import { WordPressSection } from "./SiteWordPress";
import { SslTab } from "./SiteSsl";

/** Sitenin bir bölümünü kaydeden ortak mutasyon. */
function useSaveSite(site: Site, message: string) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (changes: Record<string, unknown>) => api.patch<Site>(`/sites/${site.id}`, changes),
    onSuccess: (updated) => {
      queryClient.setQueryData(["site", String(site.id)], updated);
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      void queryClient.invalidateQueries({ queryKey: ["sites-summary"] });
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
  const templates = SITE_TEMPLATES.filter((item) => item.kind === site.kind);
  // Site türü değişmişse eski türün şablonu kalmış olabilir; o durumda varsayılan gösterilir.
  const [template, setTemplate] = useState(templates.some((item) => item.id === site.template) ? site.template : "default");
  const options = useSiteOptions();
  const [ip, setIp] = useState(site.ip);
  // Site, seçeneklerde olmayan bir adrese bağlı olabilir (yönetici bağlamıştır); o da listede gösterilir.
  const ipChoices = [...(options.data?.ips ?? [])];
  if (site.ip !== "" && !ipChoices.some((item) => item.address === site.ip)) ipChoices.unshift({ address: site.ip, label: "", user_id: null });
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

  return (
    <div className="flex flex-col gap-5">
      <SectionForm
        title={t("Adresler")}
        saving={save.isPending}
        error={save.error}
        onSubmit={() =>
          save.mutate({
            aliases: aliases.split(/[\s,]+/).filter(Boolean),
            public_dir: site.kind === "php" || site.kind === "static" ? publicDir : undefined,
            template: templates.length > 0 ? template : undefined,
            ip: ip !== site.ip ? ip : undefined,
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
        {(site.kind === "php" || site.kind === "static") && (
          <Input
            label={t("Yayın klasörü")}
            value={publicDir}
            onChange={(event) => setPublicDir(event.target.value)}
            hint={t("Site klasörünün içinde ziyaretçilere açılan alt klasör. Laravel için genellikle app/public gibi bir yol kullanılır.")}
            className="font-mono text-[13.5px]"
            spellCheck={false}
            required
          />
        )}
        {templates.length > 0 && (
          <Select
            label={t("Uygulama şablonu")}
            value={template}
            onChange={(event) => setTemplate(event.target.value)}
            hint={t(templates.find((item) => item.id === template)?.hint ?? "")}
          >
            {templates.map((item) => (
              <option key={item.id} value={item.id}>
                {t(item.label)}
              </option>
            ))}
          </Select>
        )}
        {ipChoices.length > 0 && (
          <Select
            label={t("Sunucu adresi")}
            value={ip}
            onChange={(event) => setIp(event.target.value)}
            hint={
              ip === ""
                ? t("Site sunucunun bütün adreslerinden yanıt verir.")
                : t("Site yalnızca bu adresten yanıt verir; panelin yönettiği DNS kayıtları bu adrese çevrilir.")
            }
          >
            <option value="">{t("Bütün adresler (ortak)")}</option>
            {ipChoices.map((item) => (
              <option key={item.address} value={item.address}>
                {item.label === "" ? item.address : `${item.address} — ${item.label}`}
              </option>
            ))}
          </Select>
        )}
      </SectionForm>

      <Panel title={t("Yayın durumu")}>
        <div className="flex flex-col divide-y divide-line">
          {canSuspend && (
            <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
              <div className="max-w-[60ch]">
                <p className="font-medium">{site.status === "active" ? t("Siteyi askıya al") : t("Siteyi yeniden yayınla")}</p>
                <p className="text-[13.5px] text-ink-2">{t("Askıdaki site ziyaretçilere 503 yanıtı verir; dosyaları ve ayarları korunur.")}</p>
              </div>
              <Button loading={status.isPending} onClick={() => status.mutate({ status: site.status === "active" ? "suspended" : "active" })}>
                {site.status === "active" ? t("Askıya al") : t("Yeniden yayınla")}
              </Button>
            </div>
          )}
          <CloneRow site={site} />
          {site.kind !== "proxy" && <MigrateRow site={site} />}
          <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
            <div className="max-w-[60ch]">
              <p className="font-medium">{t("Siteyi sil")}</p>
              <p className="text-[13.5px] text-ink-2">{t("Siteyi yayından kaldırır. Dosyaları saklamayı ya da birlikte silmeyi seçebilirsiniz.")}</p>
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

/** Boş bırakılan alan genel ayarı devralır; arayüzde boş dize, API'de `null`. */
const textOrNull = (value: string) => (value.trim() === "" ? null : value.trim());
const numberOrNull = (value: string) => (value.trim() === "" ? null : Number(value));

function PhpTab({ site, options }: { site: Site; options: SiteOptions }) {
  const me = useMe();
  const selectable = selectablePhpVersions(options);
  const [version, setVersion] = useState(site.php_version ?? selectable[0] ?? "");
  const [settings, setSettings] = useState<PhpSettings>(site.php_settings);
  const save = useSaveSite(site, t("PHP ayarları kaydedildi"));

  const set = <K extends keyof PhpSettings>(key: K, value: PhpSettings[K]) => setSettings((current) => ({ ...current, [key]: value }));
  const setDirective = (index: number, field: "key" | "value", value: string) =>
    set(
      "custom",
      settings.custom.map((directive, i) => (i === index ? { ...directive, [field]: value } : directive)),
    );

  // Sitenin mevcut sürümü paketin izinli listesinde olmasa da seçenekler arasında kalır (rozetle); başka bir paket dışı sürüm seçilemez.
  const current = site.php_version;
  const versions = current && !selectable.includes(current) ? [current, ...selectable] : selectable.length === 0 && version ? [version] : selectable;
  // Seçili sürümün genel ayarları: boş bırakılan alanların alacağı değerler.
  const defaults: PhpGlobal | undefined = options.php_defaults[version];
  const inherited = (directive: string, value: string | number | undefined) =>
    value === undefined ? directive : t("{directive} · boş bırakılırsa genel ayar: {value}", { directive, value });
  const overrides = [
    settings.memory_limit,
    settings.upload_max_filesize,
    settings.post_max_size,
    settings.max_execution_time,
    settings.max_input_vars,
    settings.display_errors,
  ].filter((value) => value !== null).length;

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
            {item === current && site.php_out_of_package ? ` (${t("paket dışı")})` : ""}
          </option>
        ))}
      </Select>
      {site.php_out_of_package && (
        <p className="rounded-md bg-warn-soft px-3 py-2 text-[13.5px] text-warn">
          {t("PHP {version} hesabın paketindeki izinli sürümlerin dışında kaldı. Site çalışmaya devam eder; sürümü yalnızca paketin izin verdiği bir sürüme değiştirebilirsiniz.", { version: current ?? "" })}
        </p>
      )}

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md bg-sunken/70 px-3 py-2 text-[13.5px] text-ink-2">
        <span>
          {overrides === 0
            ? t("Bu site PHP {version} sürümünün genel ayarlarını kullanıyor. Bir alanı doldurursanız yalnızca bu sitede o değer geçerli olur.", { version })
            : t("{count} ayar bu siteye özel; boş bırakılan alanlar PHP {version} genel ayarını kullanır.", { count: overrides, version })}
        </span>
        {me.role === "admin" && (
          <Link to={`/php?surum=${encodeURIComponent(version)}`} className="font-medium text-accent hover:underline">
            {t("Genel ayarları düzenle")}
          </Link>
        )}
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        <Input
          label={t("Bellek sınırı")}
          value={settings.memory_limit ?? ""}
          placeholder={defaults?.memory_limit}
          onChange={(e) => set("memory_limit", textOrNull(e.target.value))}
          hint={inherited("memory_limit", defaults?.memory_limit)}
          className="font-mono text-[13.5px]"
        />
        <Input
          label={t("Çalışma süresi (saniye)")}
          type="number"
          min={1}
          value={settings.max_execution_time ?? ""}
          placeholder={defaults ? String(defaults.max_execution_time) : undefined}
          onChange={(e) => set("max_execution_time", numberOrNull(e.target.value))}
          hint={inherited("max_execution_time", defaults?.max_execution_time)}
        />
        <Input
          label={t("En büyük yükleme boyutu")}
          value={settings.upload_max_filesize ?? ""}
          placeholder={defaults?.upload_max_filesize}
          onChange={(e) => set("upload_max_filesize", textOrNull(e.target.value))}
          hint={inherited("upload_max_filesize", defaults?.upload_max_filesize)}
          className="font-mono text-[13.5px]"
        />
        <Input
          label={t("En büyük POST boyutu")}
          value={settings.post_max_size ?? ""}
          placeholder={defaults?.post_max_size}
          onChange={(e) => set("post_max_size", textOrNull(e.target.value))}
          hint={inherited("post_max_size", defaults?.post_max_size)}
          className="font-mono text-[13.5px]"
        />
        <Input
          label={t("En fazla form alanı")}
          type="number"
          min={100}
          value={settings.max_input_vars ?? ""}
          placeholder={defaults ? String(defaults.max_input_vars) : undefined}
          onChange={(e) => set("max_input_vars", numberOrNull(e.target.value))}
          hint={inherited("max_input_vars", defaults?.max_input_vars)}
        />
        <Select
          label={t("Hataları sayfada göster")}
          value={settings.display_errors === null ? "" : settings.display_errors ? "on" : "off"}
          onChange={(e) => set("display_errors", e.target.value === "" ? null : e.target.value === "on")}
          hint={t("display_errors · geliştirme sırasında işe yarar; canlı sitede kapalı tutun.")}
        >
          <option value="">{defaults ? t("Genel ayarı kullan ({value})", { value: defaults.display_errors ? t("açık") : t("kapalı") }) : t("Genel ayarı kullan")}</option>
          <option value="on">{t("Açık")}</option>
          <option value="off">{t("Kapalı")}</option>
        </Select>
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
        <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Ek php.ini yönergeleri")}</legend>
        {settings.custom.map((directive, index) => (
          <div key={index} className="flex items-center gap-2">
            <input
              aria-label={t("Yönerge adı")}
              placeholder="opcache.enable"
              value={directive.key}
              onChange={(e) => setDirective(index, "key", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]"
              spellCheck={false}
            />
            <input
              aria-label={t("Değer")}
              placeholder="1"
              value={directive.value}
              onChange={(e) => setDirective(index, "value", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]"
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
        className="font-mono text-[13.5px]"
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

function OverviewSection({ site, options }: { site: Site; options: SiteOptions }) {
  const summaries = useQuery({ queryKey: ["sites-summary"], queryFn: () => api.get<SiteSummary[]>("/sites/summary"), refetchInterval: 15000 });
  const summary = summaries.data?.find((item) => item.site_id === site.id);
  return (
    <div className="flex flex-col gap-5">
      <section aria-label={t("Site araçları")} className="overflow-hidden rounded-lg border border-line bg-surface">
        <SiteToolGrid site={site} summary={summary} options={options} />
      </section>
      <UptimePanel siteId={site.id} />
      <Panel title={t("Özet")}>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <dt className="text-ink-2">{t("Tür")}</dt>
          <dd>{siteKindLabel(site)}</dd>
          <dt className="text-ink-2">{t("Sahibi")}</dt>
          <dd>
            <Mono>{site.owner}</Mono>
          </dd>
          <dt className="text-ink-2">{t("Disk")}</dt>
          <dd className="tabular font-mono text-[13.5px]">{summary?.disk_bytes != null ? formatBytes(summary.disk_bytes) : "—"}</dd>
          <dt className="text-ink-2">{t("Bu ayki trafik")}</dt>
          <dd className="tabular font-mono text-[13.5px]">{summary ? formatBytes(summary.month_bytes) : "—"}</dd>
          {site.kind === "proxy" ? (
            <>
              <dt className="text-ink-2">{t("Hedef")}</dt>
              <dd className="min-w-0 break-all">
                <Mono>{site.proxy_target}</Mono>
              </dd>
            </>
          ) : (
            <>
              <dt className="text-ink-2">{t("Klasör")}</dt>
              <dd className="min-w-0 break-all">
                <Mono>
                  /home/{site.owner}/sites/{site.domain}
                </Mono>
              </dd>
              <dt className="text-ink-2">{t("Yayın klasörü")}</dt>
              <dd>
                <Mono>{site.public_dir}</Mono>
              </dd>
            </>
          )}
          {site.aliases.length > 0 && (
            <>
              <dt className="text-ink-2">{t("Takma adlar")}</dt>
              <dd className="min-w-0 break-all">
                <Mono>{site.aliases.join(", ")}</Mono>
              </dd>
            </>
          )}
          <dt className="text-ink-2">{t("Oluşturulma")}</dt>
          <dd>{formatDateTime(site.created_at)}</dd>
        </dl>
      </Panel>
    </div>
  );
}

/** Site şablonları: sunucudaki listeyle (`nginx::TEMPLATES`) aynı kimlikler. */
const SITE_TEMPLATES: { id: string; kind: string; label: string; hint: string }[] = [
  { id: "default", kind: "php", label: "Genel PHP uygulaması", hint: "Bulunamayan adresler index.php dosyasına yönlenir; çoğu PHP uygulaması için uygundur." },
  {
    id: "wordpress",
    kind: "php",
    label: "WordPress",
    hint: "Yüklenen dosyalar arasındaki PHP çalıştırılmaz, xmlrpc.php kapatılır, sürümü ele veren dosyalar gizlenir.",
  },
  { id: "laravel", kind: "php", label: "Laravel / Symfony", hint: "Uygulamanın iç klasörleri (storage, vendor, config…) ve .env dosyası hiçbir koşulda sunulmaz." },
  { id: "default", kind: "static", label: "Durağan site", hint: "Dosyalar olduğu gibi sunulur; olmayan adres 404 döner." },
  { id: "spa", kind: "static", label: "Tek sayfalık uygulama (React, Vue…)", hint: "Bilinmeyen her adres index.html dosyasına düşer; yönlendirmeyi uygulama yapar." },
  { id: "clean_urls", kind: "static", label: "Uzantısız adresler", hint: "/hakkinda adresi hakkinda.html dosyasını sunar." },
];

interface Section {
  id: string;
  label: string;
  icon: LucideIcon;
}

export function SiteDetailPage() {
  const { id, section } = useParams();
  const me = useMe();
  const site = useQuery({ queryKey: ["site", id], queryFn: () => api.get<Site>(`/sites/${id}`) });
  const options = useSiteOptions();

  if (site.isPending || options.isPending) return <LoadingBlock />;
  if (site.isError) return <ErrorBlock message={errorMessage(site.error)} />;
  if (options.isError) return <ErrorBlock message={errorMessage(options.error)} />;

  const data = site.data;
  const sections: Section[] = [
    { id: "overview", label: t("Genel bakış"), icon: LayoutGrid },
    { id: "settings", label: t("Adresler ve yayın"), icon: Globe },
    { id: "access", label: t("Kurallar ve erişim"), icon: Signpost },
    ...(data.kind === "php" ? [{ id: "php", label: t("PHP ayarları"), icon: FileCode2 }] : []),
    ...(data.kind === "php" ? [{ id: "wordpress", label: "WordPress", icon: PenLine }] : []),
    { id: "stats", label: t("İstatistikler"), icon: ChartColumn },
    ...(data.kind === "node" ? [{ id: "node", label: t("Node.js uygulaması"), icon: Hexagon }] : []),
    ...(data.kind === "python" ? [{ id: "node", label: t("Python uygulaması"), icon: Hexagon }] : []),
    ...(data.kind === "proxy" ? [{ id: "proxy", label: t("Ters vekil"), icon: ArrowRightLeft }] : []),
    ...(data.kind !== "proxy" ? [{ id: "rules", label: t("Vekil kuralları"), icon: Route }] : []),
    ...(me.role !== "member" || me.permissions.includes("ssl") ? [{ id: "ssl", label: t("SSL sertifikası"), icon: ShieldCheck }] : []),
    { id: "git", label: t("Git ile yayınlama"), icon: GitBranch },
    ...(data.kind !== "proxy" && (me.role !== "member" || me.permissions.includes("ftp")) ? [{ id: "ftp", label: t("FTP hesapları"), icon: HardDriveUpload }] : []),
    ...(me.role === "admin" ? [{ id: "advanced", label: t("nginx yönergeleri"), icon: SlidersHorizontal }] : []),
  ];
  const active = sections.some((item) => item.id === section) ? section : "overview";
  const scheme = data.ssl_mode === "none" ? "http" : "https";

  return (
    <>
      <Link to="/sites" className="mb-3 inline-flex items-center gap-1.5 text-[13.5px] font-medium text-ink-2 hover:text-ink">
        <ArrowLeft className="size-3.5" /> {t("Siteler")}
      </Link>
      <header className="mb-5 flex flex-wrap items-center gap-x-4 gap-y-2">
        <h1 className="font-mono text-[22px] leading-tight font-medium tracking-[-0.01em] break-all">{data.domain}</h1>
        {data.status === "active" ? <Status tone="ok">{t("Yayında")}</Status> : <Status tone="warn">{t("Askıda")}</Status>}
        <Badge>{siteKindLabel(data)}</Badge>
        {data.php_out_of_package && <Badge tone="warn">{t("paket dışı sürüm")}</Badge>}
        <a
          href={`${scheme}://${data.domain}`}
          target="_blank"
          rel="noreferrer"
          className="ml-auto inline-flex h-9 items-center gap-1.5 rounded-md border border-line-strong bg-surface px-3.5 font-medium hover:bg-sunken"
        >
          {t("Siteyi aç")} <ExternalLink className="size-3.5 text-ink-3" />
        </a>
      </header>

      <div className="flex flex-col gap-5 lg:flex-row lg:items-start">
        <nav aria-label={t("Site bölümleri")} className="no-scrollbar -mx-1 flex gap-1 overflow-x-auto px-1 lg:sticky lg:top-18 lg:mx-0 lg:w-52 lg:shrink-0 lg:flex-col lg:gap-px lg:overflow-visible lg:px-0">
          {sections.map((item) => (
            <NavLink
              key={item.id}
              to={item.id === "overview" ? `/sites/${data.id}` : `/sites/${data.id}/${item.id}`}
              end
              className={() =>
                classNames(
                  "flex h-9 shrink-0 items-center gap-2.5 rounded-md px-2.5 font-medium whitespace-nowrap transition-colors",
                  active === item.id ? "bg-accent-soft text-accent" : "text-ink-2 hover:bg-sunken hover:text-ink",
                )
              }
            >
              <item.icon className="size-4 shrink-0" />
              {item.label}
            </NavLink>
          ))}
        </nav>
        <div className="min-w-0 flex-1">
          {/* `key`, kaydetme sonrası gelen yeni değerlerle formların sıfırlanmasını sağlar. */}
          {active === "overview" && <OverviewSection site={data} options={options.data} />}
          {active === "settings" && <GeneralTab key={JSON.stringify(data)} site={data} />}
          {active === "php" && <PhpTab key={JSON.stringify(data.php_settings) + data.php_version} site={data} options={options.data} />}
          {active === "wordpress" && <WordPressSection site={data} />}
          {active === "stats" && <StatsSection site={data} />}
          {active === "node" && <NodeSection site={data} options={options.data} />}
          {active === "proxy" && <ProxyTab key={data.proxy_target} site={data} />}
          {active === "access" && <SiteRulesSection key={JSON.stringify(data.rules)} site={data} />}
          {active === "rules" && <ProxyRulesSection key={JSON.stringify(data.proxy_rules)} site={data} />}
          {active === "ssl" && <SslTab site={data} />}
          {active === "git" && <GitSection site={data} />}
          {active === "ftp" && <FtpSection site={data} />}
          {active === "advanced" && <AdvancedTab key={data.nginx_custom} site={data} />}
        </div>
      </div>
    </>
  );
}
