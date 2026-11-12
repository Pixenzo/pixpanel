// Site çalışma alanının "Kurallar ve erişim" bölümü: yönlendirmeler, parola korumalı
// klasörler, adres engelleme, bakım kipi, WWW tercihi, görsel koruması ve hata sayfaları.
// Hepsi tek formdur; kaydedince web sunucusunun ayarı yeniden üretilir.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, IconButton, Input, Panel, Select, Switch, Textarea } from "../components/ui";
import type { ProtectedArea, Site, SiteRedirect, SiteRules } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";

const cell = "h-9 min-w-0 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]";

/** Formdaki korumalı klasör: kullanıcıların yeni parolası yalnızca yazıldıysa gönderilir. */
interface AreaDraft {
  path: string;
  label: string;
  users: { name: string; password: string; saved: boolean }[];
}

function toDrafts(areas: ProtectedArea[]): AreaDraft[] {
  return areas.map((area) => ({ path: area.path, label: area.label, users: area.users.map((user) => ({ name: user.name, password: "", saved: true })) }));
}

/** Anahtar sırasından bağımsız karşılaştırma için: sunucu alanları farklı sırada gönderir. */
function stable(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stable).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).sort(([a], [b]) => a.localeCompare(b));
    return `{${entries.map(([key, item]) => `${JSON.stringify(key)}:${stable(item)}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

/** Anahtarın yanında görünen metniyle birlikte aç/kapa düğmesi. */
function LabeledSwitch({ checked, onChange, label }: { checked: boolean; onChange: (next: boolean) => void; label: string }) {
  return (
    <div className="flex items-center gap-3">
      <Switch checked={checked} onChange={onChange} label={label} />
      <span className="font-medium">{label}</span>
    </div>
  );
}

const lines = (text: string) =>
  text
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter(Boolean);

export function SiteRulesSection({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const initial = site.rules;
  const [redirects, setRedirects] = useState<SiteRedirect[]>(initial.redirects);
  const [areas, setAreas] = useState<AreaDraft[]>(() => toDrafts(initial.protected));
  const [blocked, setBlocked] = useState(initial.blocked.join("\n"));
  const [allowed, setAllowed] = useState(initial.allowed.join("\n"));
  const [maintenance, setMaintenance] = useState(initial.maintenance);
  const [maintenanceAllowed, setMaintenanceAllowed] = useState(initial.maintenance_allowed.join("\n"));
  const [www, setWww] = useState(initial.www);
  const [hotlink, setHotlink] = useState(initial.hotlink_protection);
  const [errorPages, setErrorPages] = useState(initial.error_pages);
  const [browserCache, setBrowserCache] = useState(initial.browser_cache ?? "");
  const [serverCache, setServerCache] = useState(initial.server_cache ?? { enabled: false, ttl: "10m" });
  const [cloudflareOnly, setCloudflareOnly] = useState(initial.cloudflare_only ?? false);

  const servesFiles = site.kind === "php" || site.kind === "static";
  // Sunucu önbelleği yalnızca PHP (FastCGI) ve vekil (Node.js, Python dâhil) sitelerinde vardır.
  const canServerCache = site.kind !== "static";
  const names = [site.domain, ...site.aliases];
  const hasWwwPair = names.some((name) => names.includes(`www.${name}`));

  const body: SiteRules = {
    www,
    redirects,
    protected: areas.map((area) => ({
      path: area.path,
      label: area.label,
      users: area.users.map((user) => (user.password !== "" ? { name: user.name, password: user.password } : { name: user.name })),
    })),
    blocked: lines(blocked),
    allowed: lines(allowed),
    maintenance,
    maintenance_allowed: lines(maintenanceAllowed),
    hotlink_protection: hotlink,
    error_pages: errorPages,
    browser_cache: browserCache,
    server_cache: { enabled: canServerCache && serverCache.enabled, ttl: serverCache.ttl || "10m" },
    cloudflare_only: cloudflareOnly,
  };
  // Eski kayıtlarda yeni alanlar yoktur; karşılaştırma için varsayılanlarla tamamlanır.
  const dirty =
    stable(body) !==
    stable({ ...initial, browser_cache: initial.browser_cache ?? "", server_cache: { enabled: initial.server_cache?.enabled ?? false, ttl: initial.server_cache?.ttl || "10m" }, cloudflare_only: initial.cloudflare_only ?? false });

  const purge = useMutation({
    mutationFn: () => api.post(`/sites/${site.id}/cache/purge`),
    onSuccess: () => feedback.success(t("Önbellek temizlendi")),
    onError: (error) => feedback.error(error),
  });

  const save = useMutation({
    mutationFn: () => api.patch<Site>(`/sites/${site.id}`, { rules: body }),
    onSuccess: (updated) => {
      // Bölüm, kuralların yeni hâliyle baştan kurulur (üst bileşendeki `key`).
      queryClient.setQueryData(["site", String(site.id)], updated);
      feedback.success(t("Kurallar kaydedildi"));
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  const updateRedirect = (index: number, changes: Partial<SiteRedirect>) => setRedirects((current) => current.map((item, i) => (i === index ? { ...item, ...changes } : item)));
  const updateArea = (index: number, changes: Partial<AreaDraft>) => setAreas((current) => current.map((item, i) => (i === index ? { ...item, ...changes } : item)));
  const updateUser = (areaIndex: number, userIndex: number, changes: Partial<AreaDraft["users"][number]>) =>
    setAreas((current) => current.map((area, i) => (i === areaIndex ? { ...area, users: area.users.map((user, j) => (j === userIndex ? { ...user, ...changes } : user)) } : area)));

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <Panel
        title={t("Yönlendirmeler")}
        actions={
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setRedirects((current) => [...current, { from: "", to: "", code: 301, exact: true }])}>
            {t("Yönlendirme ekle")}
          </Button>
        }
      >
        {redirects.length === 0 ? (
          <p className="max-w-[70ch] text-ink-2">{t("Taşınan ya da kaldırılan sayfaları yeni adreslerine yönlendirin; ziyaretçiler ve arama motorları eski bağlantılardan yeni sayfaya ulaşır.")}</p>
        ) : (
          <ul className="flex flex-col gap-3">
            {redirects.map((item, index) => (
              <li key={index} className="grid items-center gap-2 border-b border-line pb-3 last:border-b-0 last:pb-0 lg:grid-cols-[1fr_1fr_auto_auto_auto]">
                <input
                  className={cell}
                  value={item.from}
                  onChange={(event) => updateRedirect(index, { from: event.target.value })}
                  placeholder="/eski-sayfa"
                  aria-label={t("Eski adres")}
                  spellCheck={false}
                  required
                />
                <input
                  className={cell}
                  value={item.to}
                  onChange={(event) => updateRedirect(index, { to: event.target.value })}
                  placeholder={t("/yeni-sayfa ya da https://…")}
                  aria-label={t("Yeni adres")}
                  spellCheck={false}
                  required
                />
                <select className={cell} value={item.exact ? "exact" : "prefix"} onChange={(event) => updateRedirect(index, { exact: event.target.value === "exact" })} aria-label={t("Kapsam")}>
                  <option value="exact">{t("Yalnızca bu adres")}</option>
                  <option value="prefix">{t("Altındaki adreslerle birlikte")}</option>
                </select>
                <select className={cell} value={item.code} onChange={(event) => updateRedirect(index, { code: Number(event.target.value) })} aria-label={t("Tür")}>
                  <option value={301}>{t("Kalıcı (301)")}</option>
                  <option value={302}>{t("Geçici (302)")}</option>
                </select>
                <IconButton label={t("Yönlendirmeyi kaldır")} onClick={() => setRedirects((current) => current.filter((_, i) => i !== index))}>
                  <Trash2 className="size-4" />
                </IconButton>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel
        title={t("Parola korumalı klasörler")}
        actions={
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setAreas((current) => [...current, { path: "", label: "", users: [{ name: "", password: "", saved: false }] }])}>
            {t("Klasör ekle")}
          </Button>
        }
      >
        {areas.length === 0 ? (
          <p className="max-w-[70ch] text-ink-2">
            {t("Bir klasörü (örneğin yönetim sayfanızı ya da yapım aşamasındaki bir bölümü) tarayıcının soracağı kullanıcı adı ve parolayla koruyun. Sitenin tamamı için yol olarak / yazın.")}
          </p>
        ) : (
          <ul className="flex flex-col gap-5">
            {areas.map((area, areaIndex) => (
              <li key={areaIndex} className="flex flex-col gap-3 border-b border-line pb-5 last:border-b-0 last:pb-0">
                <div className="grid items-end gap-3 sm:grid-cols-[1fr_1fr_auto]">
                  <Input
                    label={t("Klasör yolu")}
                    value={area.path}
                    onChange={(event) => updateArea(areaIndex, { path: event.target.value })}
                    placeholder="/yonetim"
                    className="font-mono text-[13.5px]"
                    spellCheck={false}
                    required
                  />
                  <Input label={t("Parola penceresindeki ad")} value={area.label} onChange={(event) => updateArea(areaIndex, { label: event.target.value })} placeholder={t("Korumalı alan")} maxLength={60} />
                  <IconButton label={t("Korumayı kaldır")} onClick={() => setAreas((current) => current.filter((_, i) => i !== areaIndex))} className="mb-0.5">
                    <Trash2 className="size-4" />
                  </IconButton>
                </div>
                <ul className="flex flex-col gap-2">
                  {area.users.map((user, userIndex) => (
                    <li key={userIndex} className="grid items-center gap-2 sm:grid-cols-[1fr_1fr_auto]">
                      <input
                        className={cell}
                        value={user.name}
                        onChange={(event) => updateUser(areaIndex, userIndex, { name: event.target.value, saved: false })}
                        placeholder={t("Kullanıcı adı")}
                        aria-label={t("Kullanıcı adı")}
                        autoComplete="off"
                        spellCheck={false}
                        required
                      />
                      <input
                        className={cell}
                        type="password"
                        value={user.password}
                        onChange={(event) => updateUser(areaIndex, userIndex, { password: event.target.value })}
                        placeholder={user.saved ? t("Kayıtlı parola korunuyor") : t("Parola (en az 8 karakter)")}
                        aria-label={t("Parola")}
                        autoComplete="new-password"
                        minLength={8}
                        required={!user.saved}
                      />
                      <IconButton
                        label={t("Kullanıcıyı kaldır")}
                        disabled={area.users.length === 1}
                        onClick={() => updateArea(areaIndex, { users: area.users.filter((_, i) => i !== userIndex) })}
                      >
                        <Trash2 className="size-4" />
                      </IconButton>
                    </li>
                  ))}
                </ul>
                <div>
                  <Button size="sm" variant="ghost" icon={<Plus className="size-3.5" />} onClick={() => updateArea(areaIndex, { users: [...area.users, { name: "", password: "", saved: false }] })}>
                    {t("Kullanıcı ekle")}
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Panel>

      <Panel title={t("Bakım kipi")}>
        <div className="flex flex-col gap-5">
          <LabeledSwitch checked={maintenance} onChange={setMaintenance} label={t("Ziyaretçilere bakım sayfası göster")} />
          <p className="max-w-[70ch] text-ink-2">
            {t("Açıkken ziyaretçiler sitenin yerine kısa bir bakım sayfası görür; arama motorlarına geçici olduğu bildirilir. Çalışmanızı sürdürebilmeniz için kendi adresinizi aşağıya yazın.")}
          </p>
          {maintenance && (
            <Textarea
              label={t("Siteyi görmeye devam edecek adresler")}
              value={maintenanceAllowed}
              onChange={(event) => setMaintenanceAllowed(event.target.value)}
              placeholder="203.0.113.5"
              hint={t("Her satıra bir IP adresi ya da ağ.")}
              className="min-h-20"
              spellCheck={false}
            />
          )}
        </div>
      </Panel>

      <Panel title={t("Adres kısıtlaması")}>
        <div className="grid gap-4 lg:grid-cols-2">
          <Textarea
            label={t("Engellenen adresler")}
            value={blocked}
            onChange={(event) => setBlocked(event.target.value)}
            placeholder={"203.0.113.5\n198.51.100.0/24"}
            hint={t("Bu adresler siteye erişemez. Her satıra bir IP adresi ya da ağ.")}
            className="min-h-24"
            spellCheck={false}
          />
          <Textarea
            label={t("Yalnızca şu adresler erişsin")}
            value={allowed}
            onChange={(event) => setAllowed(event.target.value)}
            placeholder="203.0.113.5"
            hint={t("Doluysa siteye bu adresler dışında kimse erişemez. Herkese açık siteler için boş bırakın.")}
            className="min-h-24"
            spellCheck={false}
          />
        </div>
      </Panel>

      <Panel title={t("Tarayıcı önbelleği")}>
        <div className="flex max-w-md flex-col gap-3">
          <Select
            label={t("Durağan dosyalar tarayıcıda şu kadar saklansın")}
            value={browserCache}
            onChange={(event) => setBrowserCache(event.target.value)}
            hint={
              servesFiles
                ? t("Resim, stil, betik ve yazı tipi dosyalarının ziyaretçinin tarayıcısında ne kadar saklanacağı. Uzun süre sayfaları hızlandırır; ama dosyayı değiştirdiğinizde ziyaretçiler eskisini görmeye devam edebilir.")
                : t("Resim, stil, betik ve yazı tipi dosyaları için uygulamanın kendi önbellek başlıkları yerine bu süre yazılır. Seçmezseniz uygulamanın başlıklarına dokunulmaz.")
            }
          >
            <option value="">{servesFiles ? t("Varsayılan (30 gün)") : t("Uygulamanın kendi başlıkları")}</option>
            {servesFiles && <option value="off">{t("Kapalı (süre bildirilmez)")}</option>}
            <option value="1h">{t("1 saat")}</option>
            <option value="1d">{t("1 gün")}</option>
            <option value="7d">{t("7 gün")}</option>
            <option value="30d">{t("30 gün")}</option>
            <option value="1y">{t("1 yıl")}</option>
          </Select>
        </div>
      </Panel>

      {canServerCache && (
        <Panel
          title={t("Sunucu önbelleği")}
          actions={
            initial.server_cache?.enabled ? (
              <Button size="sm" loading={purge.isPending} onClick={() => purge.mutate()}>
                {t("Önbelleği temizle")}
              </Button>
            ) : undefined
          }
        >
          <div className="flex flex-col gap-5">
            <LabeledSwitch
              checked={serverCache.enabled}
              onChange={(next) => setServerCache((current) => ({ ...current, enabled: next }))}
              label={site.kind === "php" ? t("Sayfaları sunucuda önbelleğe al (FastCGI)") : t("Yanıtları sunucuda önbelleğe al")}
            />
            <p className="max-w-[70ch] text-ink-2">
              {t("Ziyaretçilere aynı sayfa tekrar tekrar hazırlanmak yerine saklanan kopyadan verilir; site belirgin biçimde hızlanır ve sunucu daha az yorulur. Giriş yapmış ziyaretçilerin, yönetim sayfalarının, form gönderimlerinin ve çerez ayarlayan yanıtların kopyası saklanmaz.")}
            </p>
            {serverCache.enabled && (
              <div className="max-w-xs">
                <Select
                  label={t("Kopya ne kadar geçerli sayılsın")}
                  value={serverCache.ttl || "10m"}
                  onChange={(event) => setServerCache((current) => ({ ...current, ttl: event.target.value }))}
                  hint={t("Süre dolunca sayfa yeniden hazırlanır. Sitede değişiklik yaptıktan sonra hemen görünmesini istiyorsanız önbelleği temizleyin.")}
                >
                  <option value="1m">{t("1 dakika")}</option>
                  <option value="5m">{t("5 dakika")}</option>
                  <option value="10m">{t("10 dakika")}</option>
                  <option value="30m">{t("30 dakika")}</option>
                  <option value="1h">{t("1 saat")}</option>
                  <option value="6h">{t("6 saat")}</option>
                  <option value="1d">{t("1 gün")}</option>
                </Select>
              </div>
            )}
            {!initial.server_cache?.enabled && <p className="max-w-[70ch] text-[13px] text-ink-3">{t("Önbelleği temizleme düğmesi, ayarı kaydettikten sonra görünür.")}</p>}
          </div>
        </Panel>
      )}

      <Panel title={t("Yalnızca Cloudflare")}>
        <div className="flex flex-col gap-3">
          <LabeledSwitch checked={cloudflareOnly} onChange={setCloudflareOnly} label={t("Siteye yalnızca Cloudflare üzerinden erişilsin")} />
          <p className="max-w-[70ch] text-ink-2">
            {t("Açıkken sunucunun adresine doğrudan gelen bağlantılar reddedilir; yalnızca Cloudflare'in adres aralıklarından gelen istekler kabul edilir. Böylece Cloudflare'in güvenlik duvarını atlayarak saldırı yapılamaz.")}
          </p>
          {cloudflareOnly && (
            <p role="alert" className="max-w-[70ch] rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Dikkat: alan adınızın DNS kaydı Cloudflare üzerinden vekillenmiyorsa (turuncu bulut) site herkese kapanır. Önce DNS sayfasından kaydı kontrol edin. Sertifika doğrulaması her durumda çalışır.")}
            </p>
          )}
        </div>
      </Panel>

      {(hasWwwPair || servesFiles) && (
        <Panel title={t("Diğer")}>
          <div className="flex flex-col gap-5">
            {hasWwwPair && (
              <div className="max-w-md">
                <Select
                  label={t("Asıl alan adı")}
                  value={www}
                  onChange={(event) => setWww(event.target.value)}
                  hint={t("Arama motorlarının siteyi tek adreste görmesi için öteki ad buna yönlendirilir.")}
                >
                  <option value="">{t("İkisi de ayrı ayrı çalışsın")}</option>
                  <option value="apex">{site.domain.replace(/^www\./, "")}</option>
                  <option value="www">{`www.${site.domain.replace(/^www\./, "")}`}</option>
                </Select>
              </div>
            )}
            {servesFiles && (
              <>
                <div className="flex flex-col gap-2">
                  <LabeledSwitch checked={hotlink} onChange={setHotlink} label={t("Görsellerin başka sitelerde gösterilmesini engelle")} />
                  <p className="max-w-[70ch] text-[13.5px] text-ink-2">{t("Başka siteler görsellerinizi kendi sayfalarına gömerek bant genişliğinizi kullanamaz. Görseli doğrudan açanlar ve sitenizin kendi sayfaları etkilenmez.")}</p>
                </div>
                <div className="grid gap-4 border-t border-line pt-4 sm:grid-cols-3">
                  <Input
                    label={t("Bulunamadı sayfası (404)")}
                    value={errorPages.not_found}
                    onChange={(event) => setErrorPages((current) => ({ ...current, not_found: event.target.value }))}
                    placeholder="/404.html"
                    className="font-mono text-[13.5px]"
                    spellCheck={false}
                  />
                  <Input
                    label={t("Erişim yok sayfası (403)")}
                    value={errorPages.forbidden}
                    onChange={(event) => setErrorPages((current) => ({ ...current, forbidden: event.target.value }))}
                    placeholder="/403.html"
                    className="font-mono text-[13.5px]"
                    spellCheck={false}
                  />
                  <Input
                    label={t("Sunucu hatası sayfası (5xx)")}
                    value={errorPages.server_error}
                    onChange={(event) => setErrorPages((current) => ({ ...current, server_error: event.target.value }))}
                    placeholder="/hata.html"
                    className="font-mono text-[13.5px]"
                    spellCheck={false}
                  />
                </div>
                <p className="max-w-[70ch] text-[13px] text-ink-3">{t("Hata sayfaları sitenizin yayın klasöründeki dosyalardır; boş bıraktığınızda web sunucusunun kendi sayfası gösterilir.")}</p>
              </>
            )}
          </div>
        </Panel>
      )}

      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div className="sticky bottom-16 z-10 flex items-center gap-3 lg:bottom-4">
        <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty} className="shadow-pop">
          {t("Kuralları kaydet")}
        </Button>
        {dirty && <span className="rounded-md bg-surface px-2 py-1 text-[13.5px] text-ink-2 shadow-card">{t("Kaydedilmemiş değişiklikler var")}</span>}
      </div>
    </form>
  );
}
