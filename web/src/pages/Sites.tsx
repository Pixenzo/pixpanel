import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, CornerDownRight, ExternalLink, Lock, LockOpen, Plus, UserRound } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import {
  Badge,
  Button,
  EmptyState,
  ErrorBlock,
  Input,
  LoadingBlock,
  Modal,
  Meter,
  Mono,
  PageHeader,
  Panel,
  SearchInput,
  Select,
  Spinner,
} from "../components/ui";
import type { Overview, Site, SiteKind, SiteOptions, SiteSummary } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatPercent, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useJobs } from "../lib/live";
import { useMe } from "../lib/session";
import { siteToolGroups, siteTools } from "../lib/siteTools";
import { BackupImportModal } from "./BackupImport";

export const kindLabels: Record<SiteKind, string> = {
  php: "PHP",
  static: "Statik",
  proxy: "Ters vekil",
  node: "Node.js",
  python: "Python",
};

const kindChoices: { id: SiteKind; title: string; description: string }[] = [
  { id: "php", title: "PHP", description: "WordPress, Laravel ve diğer PHP uygulamaları" },
  { id: "node", title: "Node.js", description: "Express, Next.js, NestJS gibi Node.js uygulamaları" },
  { id: "python", title: "Python", description: "Flask, Django, FastAPI gibi Python uygulamaları" },
  { id: "static", title: "Statik", description: "Yalnızca HTML, CSS ve JavaScript dosyaları" },
  { id: "proxy", title: "Ters vekil", description: "Başka bir adreste ya da portta çalışan uygulamaya yönlendirir" },
];

/** Hesabın seçebileceği PHP sürümleri: kurulu sürümlerin paketin izin verdikleriyle kesişimi (yönetici için hepsi). */
export function selectablePhpVersions(options: SiteOptions): string[] {
  return options.php_allowed.length === 0 ? options.php_versions : options.php_versions.filter((version) => options.php_allowed.includes(version));
}

export function useSiteOptions() {
  return useQuery({ queryKey: ["site-options"], queryFn: () => api.get<SiteOptions>("/sites/options") });
}

function CreateSiteModal({
  open,
  onClose,
  options,
  proxyTarget: initialTarget,
  parent,
}: {
  open: boolean;
  onClose: () => void;
  options: SiteOptions;
  /** Verilirse pencere ters vekil türü ve bu hedefle açılır. */
  proxyTarget?: string;
  /** Verilirse bu alan adının altına bir alt alan adı eklenir. */
  parent?: { domain: string; ownerId: number | null };
}) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  // Alt alan adı eklenirken yalnızca ön ek yazılır; üst alan adı sabittir.
  const [prefix, setPrefix] = useState("");
  const [fullDomain, setFullDomain] = useState("");
  const domain = parent ? (prefix.trim() ? `${prefix.trim()}.${parent.domain}` : "") : fullDomain;
  const setDomain = setFullDomain;
  const [nodeVersion, setNodeVersion] = useState(options.node_versions[0] ?? "");
  const phpChoices = selectablePhpVersions(options);
  const [kind, setKind] = useState<SiteKind>(initialTarget !== undefined ? "proxy" : phpChoices.length > 0 ? "php" : "static");
  const [phpVersion, setPhpVersion] = useState(phpChoices[0] ?? "");
  const [proxyTarget, setProxyTarget] = useState(initialTarget || "http://127.0.0.1:3000");
  const [withWww, setWithWww] = useState(true);
  const [withWordPress, setWithWordPress] = useState(false);
  const [ownerId, setOwnerId] = useState(parent?.ownerId ?? me.id);

  const isSubdomain = domain.trim().split(".").length > 2;
  const create = useMutation({
    mutationFn: () =>
      api.post<Site>("/sites", {
        domain,
        kind,
        php_version: kind === "php" ? phpVersion : undefined,
        proxy_target: kind === "proxy" ? proxyTarget : undefined,
        node_version: kind === "node" ? nodeVersion : undefined,
        aliases: withWww && !isSubdomain && domain.trim() ? [`www.${domain.trim()}`] : [],
        user_id: options.owners.length > 0 ? ownerId : undefined,
      }),
    onSuccess: (site) => {
      feedback.success(t("{domain} oluşturuldu", { domain: site.domain }));
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      void queryClient.invalidateQueries({ queryKey: ["sites-summary"] });
      onClose();
      // WordPress istenmişse doğrudan kurulum formuna geçilir.
      void navigate(kind === "php" && withWordPress ? `/sites/${site.id}/wordpress` : `/sites/${site.id}`);
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  const phpMissing = kind === "php" && phpChoices.length === 0;
  // Kurulu sürüm var ama paket hiçbirine izin vermiyorsa kurulum bağlantısı yerine paket uyarısı gösterilir.
  const phpBlockedByPackage = phpMissing && options.php_versions.length > 0;
  const nodeMissing = kind === "node" && options.node_versions.length === 0;
  const pythonMissing = kind === "python" && options.python_versions.length === 0;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={parent ? t("{domain} için alt alan adı ekle", { domain: parent.domain }) : t("Site ekle")}
      wide
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-site" loading={create.isPending} disabled={phpMissing || nodeMissing || pythonMissing}>
            {t("Siteyi ekle")}
          </Button>
        </>
      }
    >
      <form id="create-site" onSubmit={submit} className="flex flex-col gap-5">
        {parent ? (
          <div className="flex flex-col gap-1.5">
            <label htmlFor="subdomain-prefix" className="text-[13.5px] font-medium text-ink-2">
              {t("Alt alan adı")}
            </label>
            <div className="flex items-center rounded-md border border-line-strong bg-surface focus-within:border-accent">
              <input
                id="subdomain-prefix"
                value={prefix}
                onChange={(event) => setPrefix(event.target.value.toLowerCase())}
                placeholder="blog"
                className="h-9 min-w-0 flex-1 rounded-l-md bg-transparent px-3 text-right font-mono text-[13.5px] outline-none"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus
                required
              />
              <span className="shrink-0 border-l border-line bg-sunken/70 px-3 py-2 font-mono text-[13.5px] text-ink-2">.{parent.domain}</span>
            </div>
            <p className="text-[13px] text-ink-3">{t("Alt alan adı kendi klasörü, türü ve ayarları olan ayrı bir sitedir.")}</p>
          </div>
        ) : (
          <Input
            label={t("Alan adı")}
            value={domain}
            onChange={(event) => setDomain(event.target.value)}
            placeholder="ornek.com"
            hint={t("Alt alan adı da yazabilirsiniz: blog.ornek.com")}
            autoCapitalize="none"
            spellCheck={false}
            autoFocus
            required
          />
        )}
        {!isSubdomain && domain.trim() && (
          <label className="-mt-2 flex items-center gap-2.5">
            <input type="checkbox" className="size-4 accent-(--accent)" checked={withWww} onChange={(event) => setWithWww(event.target.checked)} />
            <span>
              <Mono>www.{domain.trim()}</Mono> {t("adresi de bu siteyi açsın")}
            </span>
          </label>
        )}

        <fieldset>
          <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Site türü")}</legend>
          <div className="grid gap-2 sm:grid-cols-3">
            {kindChoices.map((choice) => (
              <label
                key={choice.id}
                className={classNames(
                  "flex cursor-pointer flex-col gap-1 rounded-lg border p-3 transition-colors",
                  kind === choice.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                )}
              >
                <span className="flex items-center gap-2 font-medium">
                  <input type="radio" name="kind" className="accent-(--accent)" checked={kind === choice.id} onChange={() => setKind(choice.id)} />
                  {t(choice.title)}
                </span>
                <span className="text-[13px] text-ink-2">{t(choice.description)}</span>
              </label>
            ))}
          </div>
        </fieldset>

        {kind === "php" &&
          (phpBlockedByPackage ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">{t("Paketiniz kurulu hiçbir PHP sürümüne izin vermiyor. Paketinizin yükseltilmesini isteyin.")}</p>
          ) : phpMissing ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Henüz kurulu bir PHP sürümü yok.")}{" "}
              <Link to="/components" className="font-medium underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            </p>
          ) : (
            <>
              <Select label={t("PHP sürümü")} value={phpVersion} onChange={(event) => setPhpVersion(event.target.value)}>
                {phpChoices.map((version) => (
                  <option key={version} value={version}>
                    PHP {version}
                  </option>
                ))}
              </Select>
              <label className="-mt-1 flex items-start gap-2.5 rounded-lg border border-line-strong p-3 has-[:checked]:border-accent has-[:checked]:bg-accent-soft">
                <input
                  type="checkbox"
                  className="mt-0.5 size-4 shrink-0 accent-(--accent)"
                  checked={withWordPress}
                  onChange={(event) => setWithWordPress(event.target.checked)}
                />
                <span>
                  <span className="font-medium">{t("WordPress ile başlat")}</span>
                  <span className="block text-[13px] text-ink-2">
                    {t("Site açılınca kurulum adımına geçersiniz; veritabanını ve ayarları panel hazırlar.")}
                  </span>
                </span>
              </label>
            </>
          ))}

        {kind === "node" &&
          (nodeMissing ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Henüz kurulu bir Node.js sürümü yok.")}{" "}
              <Link to="/components" className="font-medium underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            </p>
          ) : (
            <Select
              label={t("Node.js sürümü")}
              value={nodeVersion}
              onChange={(event) => setNodeVersion(event.target.value)}
              hint={t("Site, hemen çalışan örnek bir uygulamayla açılır; başlatma komutunu ve ortam değişkenlerini sonra ayarlarsınız.")}
            >
              {options.node_versions.map((version) => (
                <option key={version} value={version}>
                  Node.js {version}
                </option>
              ))}
            </Select>
          ))}

        {kind === "python" &&
          (pythonMissing ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Python henüz kurulu değil.")}{" "}
              <Link to="/components" className="font-medium underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            </p>
          ) : (
            <p className="text-[13.5px] text-ink-2">
              {t("Site, Python {version} ve kendi sanal ortamıyla açılır; hemen çalışan örnek bir uygulama bırakılır. Başlatma komutunu ve bağımlılıkları sonra ayarlarsınız.", {
                version: options.python_versions[0],
              })}
            </p>
          ))}

        {kind === "proxy" && (
          <Input
            label={t("Hedef adres")}
            value={proxyTarget}
            onChange={(event) => setProxyTarget(event.target.value)}
            hint={t("İsteklerin iletileceği uygulamanın adresi")}
            className="font-mono text-[13.5px]"
            spellCheck={false}
            required
          />
        )}

        {options.owners.length > 1 && !parent && (
          <Select label={t("Sahibi")} value={ownerId} onChange={(event) => setOwnerId(Number(event.target.value))} hint={t("Site bu hesabın Linux kullanıcısıyla çalışır.")}>
            {options.owners.map((owner) => (
              <option key={owner.id} value={owner.id}>
                {owner.username}
              </option>
            ))}
          </Select>
        )}

        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

const EXPANDED_KEY = "pixpanel.sites.expanded";

/** Açık site kartları tarayıcıda hatırlanır; ilk gelişte (kayıt yokken) `null` döner. */
function readExpanded(): number[] | null {
  try {
    const raw = window.localStorage.getItem(EXPANDED_KEY);
    return raw ? (JSON.parse(raw) as number[]) : null;
  } catch {
    return null;
  }
}

const toneText: Record<string, string> = { warn: "text-warn", bad: "text-bad", accent: "text-accent", ok: "text-ok" };

/** Sitenin araçları, türlerine göre gruplanmış: her kutu aracın adını ve o anki durumunu gösterir. */
export function SiteToolGrid({ site, summary, options }: { site: Site; summary?: SiteSummary; options?: SiteOptions }) {
  const me = useMe();
  const tools = siteTools(site, me, summary, options);
  return (
    <div className="flex flex-col gap-5 px-3 py-4 sm:px-5 sm:py-5">
      {siteToolGroups.map((group) => {
        const items = tools.filter((tool) => tool.group === group.id);
        if (items.length === 0) return null;
        return (
          <section key={group.id} aria-label={t(group.title)}>
            <h3 className="mb-1.5 px-2 text-[13px] font-semibold text-ink-2">{t(group.title)}</h3>
            <ul className="grid grid-cols-1 gap-x-2 gap-y-0.5 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4">
              {items.map((tool) => (
                <li key={tool.id}>
                  <Link to={tool.to} className="group flex h-full items-center gap-3 rounded-xl px-2 py-2 transition-colors hover:bg-accent-soft/50">
                    <span className="grid size-10 shrink-0 place-items-center rounded-xl border border-line bg-surface text-ink-2 shadow-[0_1px_1px_rgb(21_32_43/0.04)] transition-colors group-hover:border-accent/30 group-hover:text-accent">
                      <tool.icon aria-hidden className="size-[18px]" />
                    </span>
                    <span className="min-w-0">
                      <span className="block font-medium transition-colors group-hover:text-accent">{tool.label}</span>
                      {tool.status && <span className={classNames("block truncate text-[13px]", tool.tone ? toneText[tool.tone] : "text-ink-3")}>{tool.status}</span>}
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

export function siteKindLabel(site: Site): string {
  if (site.kind === "php") return `PHP ${site.php_version}`;
  if (site.kind === "node") return `Node.js ${site.node.version}`;
  if (site.kind === "python") return `Python ${site.node.version}`;
  return t(kindLabels[site.kind]);
}

/** Liste sütunları geniş ekranda hizalı durur: aç/kapa, alan adı, durum, disk, trafik, kısayollar. */
const ROW_GRID = "flex flex-wrap items-center gap-x-4 gap-y-2 @4xl:grid @4xl:grid-cols-[2.125rem_minmax(0,1fr)_10.5rem_5rem_6.5rem_13rem]";

/** Satırdaki kısayollar: en sık açılan araçlar tek tıkla. */
const QUICK_TOOLS = ["files", "databases", "logs", "settings"];

function SiteCard({
  site,
  summary,
  options,
  expanded,
  onToggle,
  showOwner,
  nested,
}: {
  site: Site;
  summary?: SiteSummary;
  options: SiteOptions;
  expanded: boolean;
  onToggle: () => void;
  showOwner: boolean;
  /** Listede üst alan adının altında, girintili gösterilir. */
  nested: boolean;
}) {
  const me = useMe();
  const scheme = site.ssl_mode === "none" ? "http" : "https";
  const panelId = `site-tools-${site.id}`;
  // Panelin fark ettiği sorunlar: en önemlisi durum ışığının rengini belirler.
  const issues = site.status === "active" ? (summary?.issues ?? []) : [];
  const worst = issues[0]?.severity;
  const tone = site.status !== "active" ? "warn" : worst === "critical" ? "bad" : worst === "warning" ? "warn" : "ok";
  const lamp = tone === "ok" ? "bg-ok" : tone === "bad" ? "bg-bad" : "bg-warn";
  const statusText = site.status !== "active" ? t("Askıda") : issues.length > 0 ? t(issues[0].label) : t("Yayında");
  const statusTitle = issues.length > 1 ? issues.map((issue) => t(issue.label)).join(", ") : undefined;
  const quick = siteTools(site, me, summary, options).filter((tool) => QUICK_TOOLS.includes(tool.id));
  return (
    <li className={classNames("transition-colors", expanded && "bg-sunken/40")}>
      <div className={classNames(ROW_GRID, "px-3 py-3 sm:px-4")}>
        <button
          type="button"
          aria-expanded={expanded}
          aria-controls={panelId}
          aria-label={expanded ? t("{domain} araçlarını gizle", { domain: site.domain }) : t("{domain} araçlarını göster", { domain: site.domain })}
          onClick={onToggle}
          className="inline-flex size-8.5 shrink-0 items-center justify-center rounded-lg text-ink-3 transition-colors hover:bg-sunken hover:text-ink max-sm:size-9.5"
        >
          <ChevronRight className={classNames("size-4 transition-transform", expanded && "rotate-90")} />
        </button>
        <div className={classNames("min-w-0 flex-1 @max-4xl:basis-[calc(100%-4rem)]", nested && "@4xl:pl-5")}>
          <div className="flex items-center gap-2">
            {nested && <CornerDownRight aria-hidden className="size-3.5 shrink-0 text-ink-3 @max-4xl:hidden" />}
            <Link to={`/sites/${site.id}`} className="truncate font-mono text-[15.5px] font-medium tracking-[-0.01em] hover:text-accent">
              {site.domain}
            </Link>
            <a
              href={`${scheme}://${site.domain}`}
              target="_blank"
              rel="noreferrer"
              aria-label={t("{domain} adresini yeni sekmede aç", { domain: site.domain })}
              title={t("Siteyi aç")}
              className="shrink-0 text-ink-3 hover:text-accent"
            >
              <ExternalLink className="size-3.5" />
            </a>
          </div>
          <p className="mt-0.5 flex flex-wrap items-center gap-x-2.5 gap-y-0.5 text-[13px] text-ink-3">
            <span>{siteKindLabel(site)}</span>
            {site.php_out_of_package && (
              <span title={t("Bu PHP sürümü hesabın paketindeki izinli sürümlerin dışında; site çalışmaya devam eder ama sürümü yalnızca izinli bir sürüme değiştirebilirsiniz.")}>
                <Badge tone="warn">{t("paket dışı sürüm")}</Badge>
              </span>
            )}
            <span className="flex items-center gap-1">
              {site.ssl_mode === "none" ? <LockOpen aria-hidden className="size-3" /> : <Lock aria-hidden className="size-3 text-ok" />}
              {site.ssl_mode === "none" ? t("SSL yok") : "SSL"}
            </span>
            {showOwner && (
              <span className="flex items-center gap-1">
                <UserRound aria-hidden className="size-3" />
                <span className="font-mono">{site.owner}</span>
              </span>
            )}
            {site.aliases.length > 0 && <span className="truncate font-mono">{site.aliases.join(", ")}</span>}
          </p>
        </div>
        <p className="flex min-w-0 items-center gap-2 text-[13.5px]" title={statusTitle}>
          <span aria-hidden className={classNames("size-2 shrink-0 rounded-full", lamp)} />
          <span className={classNames("truncate", tone === "ok" ? "text-ink" : tone === "bad" ? "font-medium text-bad" : "font-medium text-warn")}>{statusText}</span>
          {summary?.uptime_day != null && tone === "ok" && summary.uptime_day < 99.9 && (
            <span className="tabular font-mono text-[12.5px] text-warn" title={t("Son 24 saatte erişilebilirlik")}>
              %{summary.uptime_day.toLocaleString("tr-TR", { maximumFractionDigits: 1 })}
            </span>
          )}
        </p>
        <p className="tabular font-mono text-[13px] text-ink-2 @max-4xl:hidden" title={t("Site klasörünün boyutu")}>
          {summary?.disk_bytes != null ? formatBytes(summary.disk_bytes) : "—"}
        </p>
        <p className="tabular font-mono text-[13px] text-ink-2 @max-4xl:hidden" title={t("Bu ayki trafik")}>
          {summary ? (
            <>
              {formatBytes(summary.month_bytes)}
              <span className="text-ink-3">{t("/ay")}</span>
            </>
          ) : (
            "—"
          )}
        </p>
        <div className="ml-auto flex items-center justify-end gap-0.5">
          {quick.map((tool) => (
            <Link
              key={tool.id}
              to={tool.to}
              title={tool.label}
              aria-label={`${site.domain}: ${tool.label}`}
              className="inline-flex size-8.5 items-center justify-center rounded-lg text-ink-3 transition-colors hover:bg-sunken hover:text-accent max-sm:hidden"
            >
              <tool.icon aria-hidden className="size-[17px]" />
            </Link>
          ))}
          <Link
            to={`/sites/${site.id}`}
            className="ml-1.5 inline-flex h-8.5 items-center rounded-lg border border-line-strong bg-surface px-3.5 text-[13.5px] font-medium shadow-[0_1px_1px_rgb(21_32_43/0.04)] transition-colors hover:bg-sunken"
          >
            {t("Yönet")}
          </Link>
        </div>
      </div>
      {expanded && (
        <div id={panelId} className="border-t border-line bg-surface">
          <dl className="flex flex-wrap gap-x-8 gap-y-2 border-b border-line px-5 py-3 text-[13.5px] @4xl:hidden">
            <div className="flex gap-2">
              <dt className="text-ink-3">{t("Disk")}</dt>
              <dd className="tabular font-mono">{summary?.disk_bytes != null ? formatBytes(summary.disk_bytes) : "—"}</dd>
            </div>
            <div className="flex gap-2">
              <dt className="text-ink-3">{t("Bu ayki trafik")}</dt>
              <dd className="tabular font-mono">{summary ? formatBytes(summary.month_bytes) : "—"}</dd>
            </div>
          </dl>
          <SiteToolGrid site={site} summary={summary} options={options} />
        </div>
      )}
    </li>
  );
}

/** Sağ sütun: sunucunun anlık durumu (yönetici) ve son işlemler. */
function SideRail() {
  const me = useMe();
  const overview = useQuery({
    queryKey: ["overview"],
    queryFn: () => api.get<Overview>("/system/overview"),
    refetchInterval: 5000,
    enabled: me.role === "admin",
  });
  const jobs = useJobs();
  const recent = jobs.data?.slice(0, 6) ?? [];
  const current = overview.data?.current;
  const disk = overview.data?.disks.find((item) => item.mount === "/") ?? overview.data?.disks[0];

  return (
    <aside className="grid items-start gap-5 md:grid-cols-2 2xl:flex 2xl:w-72 2xl:flex-col 2xl:items-stretch 2xl:shrink-0">
      {current && overview.data && (
        <Panel
          title={t("Sunucu")}
          actions={
            <Link to="/server" className="text-[13.5px] font-medium text-accent hover:underline">
              {t("Ayrıntı")}
            </Link>
          }
        >
          <dl className="tabular flex flex-col gap-3 text-[13.5px]">
            {[
              { label: t("İşlemci"), percent: current.cpu, detail: formatPercent(current.cpu) },
              {
                label: t("Bellek"),
                percent: current.mem_total > 0 ? (current.mem_used / current.mem_total) * 100 : 0,
                detail: `${formatBytes(current.mem_used)} / ${formatBytes(current.mem_total)}`,
              },
              ...(disk ? [{ label: t("Disk"), percent: disk.total > 0 ? (disk.used / disk.total) * 100 : 0, detail: `${formatBytes(disk.used)} / ${formatBytes(disk.total)}` }] : []),
            ].map((row) => (
              <div key={row.label}>
                <div className="mb-1 flex items-baseline justify-between gap-2">
                  <dt className="text-ink-2">{row.label}</dt>
                  <dd className="font-mono text-[13px]">{row.detail}</dd>
                </div>
                <Meter value={row.percent} label={row.label} />
              </div>
            ))}
          </dl>
          <p className="mt-3 border-t border-line pt-3 font-mono text-[13px] text-ink-2">
            {overview.data.host.hostname} · {overview.data.host.os}
          </p>
        </Panel>
      )}
      <Panel
        flush
        title={t("Son işlemler")}
        actions={
          <Link to="/jobs" className="text-[13.5px] font-medium text-accent hover:underline">
            {t("Tümü")}
          </Link>
        }
      >
        {recent.length === 0 ? (
          <p className="px-4 sm:px-6 py-4 text-[13.5px] text-ink-2">{t("Kurulum, yayınlama ve sertifika işlemleri burada listelenir.")}</p>
        ) : (
          <ul>
            {recent.map((job) => (
              <li key={job.id} className="border-b border-line last:border-b-0">
                <Link to="/jobs" className="flex items-start gap-2.5 px-4 sm:px-6 py-2.5 hover:bg-sunken/60">
                  {job.status === "running" ? (
                    <Spinner className="mt-0.5 size-3.5 shrink-0 text-accent" />
                  ) : (
                    <span aria-hidden className={classNames("mt-1.5 size-2 shrink-0 rounded-full", job.status === "success" ? "bg-ok" : "bg-bad")} />
                  )}
                  <span className="min-w-0">
                    <span className="block truncate text-[13.5px]">{job.title}</span>
                    <span className="block text-[12.5px] text-ink-3">
                      {job.status === "running" ? t("Sürüyor") : job.status === "success" ? t(formatRelative(job.finished_at)) : t("Başarısız · {when}", { when: t(formatRelative(job.finished_at)) })}
                    </span>
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </Panel>
    </aside>
  );
}

export function SitesPage() {
  const me = useMe();
  const [searchParams, setSearchParams] = useSearchParams();
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const [filter, setFilter] = useState("");
  const [expanded, setExpanded] = useState<number[] | null>(readExpanded);
  const sites = useQuery({ queryKey: ["sites"], queryFn: () => api.get<Site[]>("/sites") });
  const summaries = useQuery({ queryKey: ["sites-summary"], queryFn: () => api.get<SiteSummary[]>("/sites/summary"), refetchInterval: 15000 });
  const options = useSiteOptions();
  const showOwner = me.role === "admin" || me.role === "reseller";

  // Docker sayfasındaki "Alan adına bağla" bağlantısı oluşturma penceresini hedefle birlikte açar.
  const proxyTarget = searchParams.get("yeni") === "vekil" ? (searchParams.get("hedef") ?? "") : null;
  // Site kartındaki "Alt alan adı ekle" kutusu pencereyi üst alan adıyla açar.
  const parentDomain = searchParams.get("yeni") === "alt" ? searchParams.get("ust") : null;
  const parentSite = parentDomain ? sites.data?.find((site) => site.domain === parentDomain) : undefined;
  const closeCreate = () => {
    setCreating(false);
    if (proxyTarget !== null || parentDomain !== null) setSearchParams({}, { replace: true });
  };

  // Az site varken hepsi açık gelir; çok site varken liste kısa kalsın diye kapalı.
  const openIds = expanded ?? ((sites.data?.length ?? 0) <= 4 ? (sites.data ?? []).map((site) => site.id) : []);
  function persist(next: number[]) {
    setExpanded(next);
    try {
      window.localStorage.setItem(EXPANDED_KEY, JSON.stringify(next));
    } catch {
      // Depolama kapalıysa tercih yalnızca bu oturumda geçerli olur.
    }
  }
  const toggle = (id: number) => persist(openIds.includes(id) ? openIds.filter((item) => item !== id) : [...openIds, id]);

  const needle = filter.trim().toLocaleLowerCase("tr");
  const visible = (sites.data ?? []).filter(
    (site) => !needle || [site.domain, site.owner, ...site.aliases].some((value) => value.toLocaleLowerCase("tr").includes(needle)),
  );
  // Alt alan adları üst sitelerinin hemen altında listelenir: sıralama anahtarı
  // etiketlerin tersidir (ornek.com, blog.ornek.com → "com.ornek", "com.ornek.blog").
  const sortKey = (name: string) => name.split(".").reverse().join(".");
  visible.sort((a, b) => sortKey(a.domain).localeCompare(sortKey(b.domain)));
  const isSubdomain = (site: Site) => visible.some((other) => other.id !== site.id && site.domain.endsWith(`.${other.domain}`));
  const allOpen = visible.length > 0 && visible.every((site) => openIds.includes(site.id));

  const createButton = (
    <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)} disabled={!options.data?.nginx_installed}>
      {t("Site ekle")}
    </Button>
  );

  return (
    <div className="flex flex-col gap-6 2xl:flex-row 2xl:items-start">
      <div className="min-w-0 flex-1">
        <PageHeader
          title={t("Siteler")}
          description={t("Alan adlarınız ve her birinin araçları tek yerde. Bir aracı açmak için kutusuna tıklayın.")}
          actions={
            <>
              {me.role === "admin" && <Button onClick={() => setImporting(true)}>{t("Yedek dosyasından içe aktar")}</Button>}
              {createButton}
            </>
          }
        />
        {sites.isError && <ErrorBlock message={errorMessage(sites.error)} onRetry={() => void sites.refetch()} />}
        {(sites.isPending || options.isPending) && <LoadingBlock />}
        {options.data && !options.data.nginx_installed && (
          <div className="mb-4 rounded-lg border border-warn/40 bg-warn-soft px-4 sm:px-6 py-3 text-warn">
            {t("Site yayınlamak için nginx gerekli.")}{" "}
            {me.role === "admin" ? (
              <Link to="/components" className="font-medium underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            ) : (
              t("Sunucu yöneticinizden kurmasını isteyin.")
            )}
          </div>
        )}
        {sites.data && options.data && sites.data.length === 0 && (
          <Panel flush>
            <EmptyState
              title={t("Henüz site yok")}
              description={t("İlk sitenizi ekleyin; alan adını, türünü ve PHP sürümünü seçmeniz yeterli.")}
              action={options.data.nginx_installed ? createButton : undefined}
            />
          </Panel>
        )}
        {sites.data && options.data && sites.data.length > 0 && (
          <>
            <div className="mb-3 flex flex-wrap items-center gap-3">
              <SearchInput placeholder={t("Alan adı ya da hesap ara")} aria-label={t("Alan adı ya da hesap ara")} value={filter} onChange={(event) => setFilter(event.target.value)} />
              <p className="text-[13.5px] text-ink-2">{t("{count} site", { count: visible.length })}</p>
              <button
                type="button"
                onClick={() => persist(allOpen ? openIds.filter((id) => !visible.some((site) => site.id === id)) : [...new Set([...openIds, ...visible.map((site) => site.id)])])}
                className="ml-auto text-[13.5px] font-medium text-accent hover:underline"
              >
                {allOpen ? t("Araçları gizle") : t("Bütün araçları göster")}
              </button>
            </div>
            {visible.length === 0 ? (
              <Panel flush>
                <EmptyState title={t("Aramanıza uyan site yok")} description={t("Alan adının bir bölümünü ya da hesabın adını yazmayı deneyin.")} />
              </Panel>
            ) : (
              <div className="@container overflow-hidden rounded-2xl border border-line bg-surface shadow-card">
                <div className={classNames(ROW_GRID, "border-b border-line bg-sunken/50 px-3 py-2 text-[12.5px] font-medium text-ink-3 @max-4xl:hidden sm:px-4")}>
                  <span />
                  <span>{t("Alan adı")}</span>
                  <span>{t("Durum")}</span>
                  <span>{t("Disk")}</span>
                  <span>{t("Trafik")}</span>
                  <span />
                </div>
                <ul className="divide-y divide-line">
                {visible.map((site) => (
                  <SiteCard
                    key={site.id}
                    site={site}
                    summary={summaries.data?.find((item) => item.site_id === site.id)}
                    options={options.data}
                    expanded={openIds.includes(site.id)}
                    onToggle={() => toggle(site.id)}
                    showOwner={showOwner}
                    nested={isSubdomain(site)}
                  />
                ))}
                </ul>
              </div>
            )}
          </>
        )}
      </div>
      <SideRail />
      {importing && <BackupImportModal onClose={() => setImporting(false)} />}
      {options.data && (creating || proxyTarget !== null || parentSite) && (
        <CreateSiteModal
          key={parentSite?.domain ?? "new"}
          open
          onClose={closeCreate}
          options={options.data}
          proxyTarget={proxyTarget ?? undefined}
          parent={parentSite ? { domain: parentSite.domain, ownerId: options.data.owners.length > 0 ? parentSite.user_id : null } : undefined}
        />
      )}
    </div>
  );
}
