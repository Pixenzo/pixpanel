// Başka sunucudaki siteyi buraya taşıma: pencere önce bağlantı bilgilerini sorar ve karşı
// sunucuya bağlanmayı dener, bulduklarını (sunucu anahtarı, klasör, WordPress) onaya sunar,
// ardından taşıma görevinin çıktısını canlı gösterir.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { useSearchParams } from "react-router";
import { Button, Checkbox, ErrorBlock, Input, Modal, Mono, Tabs, Textarea } from "../components/ui";
import type { Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

interface Probe {
  host_key: string;
  fingerprints: string[];
  address: string;
  path_found: boolean;
  size_kb: number | null;
  wordpress: { database: string; database_host: string } | null;
  can_dump: boolean;
}

type AuthMethod = "password" | "key";

function MigrateModal({ site, onClose }: { site: Site; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [username, setUsername] = useState("");
  const [method, setMethod] = useState<AuthMethod>("password");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [path, setPath] = useState("");
  const [found, setFound] = useState<Probe | null>(null);
  const [withDatabase, setWithDatabase] = useState(true);
  // WordPress bulunamadığında veritabanının bağlantı bilgileri elle yazılır.
  const [dbName, setDbName] = useState("");
  const [dbUser, setDbUser] = useState("");
  const [dbPassword, setDbPassword] = useState("");
  const [dbHost, setDbHost] = useState("localhost");
  const [jobId, setJobId] = useState<number | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const connection = () => ({
    host: host.trim(),
    port: Number(port) || 22,
    username: username.trim(),
    path: path.trim(),
    ...(method === "password" ? { password } : { private_key: privateKey }),
  });

  const probe = useMutation({
    mutationFn: () => api.post<Probe>(`/sites/${site.id}/migrate/probe`, connection()),
    onSuccess: (response) => {
      setFound(response);
      setWithDatabase(response.wordpress !== null && response.can_dump);
      setDbName("");
    },
  });

  const start = useMutation({
    mutationFn: () =>
      api.post<{ job_id: number }>(`/sites/${site.id}/migrate`, {
        ...connection(),
        host_key: found?.host_key ?? "",
        database: withDatabase && (found?.wordpress != null || manualDatabase),
        ...(manualDatabase ? { remote_database: { name: dbName.trim(), user: dbUser.trim(), password: dbPassword, host: dbHost.trim() } } : {}),
      }),
    onSuccess: (response) => setJobId(response.job_id),
  });

  // Elle yazılan veritabanı yalnızca WordPress bulunamadıysa ve karşıda döküm aracı varsa taşınır.
  const manualDatabase = found != null && found.wordpress === null && found.can_dump && withDatabase;

  function submit(event: FormEvent) {
    event.preventDefault();
    probe.mutate();
  }

  function finished(status: string) {
    setResult(status);
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
    void queryClient.invalidateQueries({ queryKey: ["databases"] });
  }

  if (jobId !== null) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={result === "success" ? t("{domain} taşındı", { domain: site.domain }) : t("{domain} taşınıyor", { domain: site.domain })}
        footer={<Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>}
      >
        <JobLog jobId={jobId} onDone={finished} />
        {result === "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">
            {t("Site taşındı. Alan adının DNS kayıtlarını bu sunucuya çevirmeden önce siteyi denetleyin; eski sunucudaki site yerinde duruyor.")}
          </p>
        )}
        {result !== null && result !== "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">{t("Taşıma tamamlanamadı. O ana kadar gelen dosyalar sitede kalır; sorunu giderip yeniden başlatabilirsiniz.")}</p>
        )}
      </Modal>
    );
  }

  if (found) {
    const canStart = found.path_found && (!manualDatabase || (dbName.trim() !== "" && dbUser.trim() !== ""));
    return (
      <Modal
        open
        onClose={onClose}
        title={t("Bağlantı kuruldu")}
        description={t("Karşı sunucuda bulunanları denetleyin; onaylarsanız taşıma başlar.")}
        footer={
          <>
            <Button onClick={() => setFound(null)}>{t("Geri")}</Button>
            <Button variant="primary" loading={start.isPending} disabled={!canStart} onClick={() => start.mutate()}>
              {t("Taşımayı başlat")}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-5">
          <dl className="grid grid-cols-[auto_1fr] gap-x-5 gap-y-3 text-[14px]">
            <dt className="text-ink-2">{t("Sunucu")}</dt>
            <dd>
              <Mono>
                {host.trim() === found.address ? found.address : `${host.trim()} (${found.address})`}
              </Mono>
            </dd>
            <dt className="text-ink-2">{t("Sunucu anahtarı")}</dt>
            <dd className="min-w-0">
              {found.fingerprints.map((print) => (
                <Mono key={print} className="block break-all text-[12.5px]">
                  {print}
                </Mono>
              ))}
              <p className="mt-1 text-[13px] text-ink-3">{t("Taşıma yalnızca bu anahtarı gösteren sunucuya bağlanır. Emin olmak isterseniz karşı sunucunun parmak iziyle karşılaştırın.")}</p>
            </dd>
            <dt className="text-ink-2">{t("Klasör")}</dt>
            <dd>
              <Mono>{path.trim()}</Mono>
              {found.path_found ? (
                found.size_kb !== null && <span className="ml-2 text-ink-2">{formatBytes(found.size_kb * 1024)}</span>
              ) : (
                <p className="mt-1 text-[13.5px] text-bad">{t("Bu klasör karşı sunucuda bulunamadı. Geri dönüp yolu düzeltin.")}</p>
              )}
            </dd>
            <dt className="text-ink-2">{t("Hedef")}</dt>
            <dd>
              <Mono>{site.domain}</Mono>
              <p className="mt-1 text-[13px] text-ink-3">{t("Dosyalar sitenin yayın klasörüne açılır; aynı adlı dosyaların üzerine yazılır.")}</p>
            </dd>
          </dl>

          {found.path_found && found.wordpress && (
            <div className="rounded-xl border border-line bg-sunken/50 p-4">
              <p className="font-medium">{t("WordPress bulundu")}</p>
              {found.can_dump && site.kind === "php" ? (
                <div className="mt-2">
                  <Checkbox
                    label={t("Veritabanını da taşı")}
                    hint={t("{database} veritabanı dökülür, bu hesapta açılan yeni bir veritabanına yüklenir ve wp-config.php ona bağlanır.", {
                      database: found.wordpress.database,
                    })}
                    checked={withDatabase}
                    onChange={(event) => setWithDatabase(event.target.checked)}
                  />
                </div>
              ) : (
                <p className="mt-1 text-[13.5px] text-ink-2">
                  {site.kind === "php"
                    ? t("Karşı sunucuda veritabanı döküm aracı (mysqldump) bulunamadı; yalnızca dosyalar taşınır, veritabanını elle aktarmanız gerekir.")
                    : t("Bu site PHP sitesi olmadığı için yalnızca dosyalar taşınır.")}
                </p>
              )}
            </div>
          )}
          {found.path_found && !found.wordpress && found.can_dump && (
            <div className="rounded-xl border border-line bg-sunken/50 p-4">
              <Checkbox
                label={t("Veritabanını da taşı")}
                hint={t("Klasörde WordPress bulunamadı. Uygulama MySQL ya da MariaDB kullanıyorsa eski sunucudaki bağlantı bilgilerini yazın; veritabanı bu hesapta açılan yeni bir veritabanına yüklenir.")}
                checked={withDatabase}
                onChange={(event) => setWithDatabase(event.target.checked)}
              />
              {withDatabase && (
                <div className="mt-4 grid gap-3 sm:grid-cols-2">
                  <Input label={t("Veritabanı adı")} value={dbName} onChange={(event) => setDbName(event.target.value)} className="font-mono text-[13.5px]" spellCheck={false} autoComplete="off" />
                  <Input label={t("Veritabanı sunucusu")} value={dbHost} onChange={(event) => setDbHost(event.target.value)} className="font-mono text-[13.5px]" spellCheck={false} autoComplete="off" />
                  <Input label={t("Veritabanı kullanıcısı")} value={dbUser} onChange={(event) => setDbUser(event.target.value)} className="font-mono text-[13.5px]" spellCheck={false} autoComplete="off" />
                  <Input label={t("Veritabanı parolası")} type="password" value={dbPassword} onChange={(event) => setDbPassword(event.target.value)} autoComplete="off" />
                  <p className="text-[13px] text-ink-3 sm:col-span-2">
                    {t("Bu bilgiler uygulamanın ayar dosyasında yazar (ör. .env ya da config.php). Yeni veritabanının bilgileri taşıma bitince hesabın tmp klasörüne yazılır; yayın klasöründeki .env kendiliğinden güncellenir.")}
                  </p>
                </div>
              )}
            </div>
          )}
          {found.path_found && !found.wordpress && !found.can_dump && (
            <p className="text-[13.5px] text-ink-2">
              {t("Yalnızca dosyalar taşınır. Site veritabanı kullanıyorsa dökümünü Veritabanları sayfasından içe aktarın.")}
            </p>
          )}
          {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{domain} sitesine başka sunucudan taşı", { domain: site.domain })}
      description={t("Eski sunucudaki sitenin dosyaları SSH ile buraya çekilir; WordPress ise veritabanı da birlikte gelir. Eski sunucudaki siteye dokunulmaz.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="site-migrate-form" loading={probe.isPending}>
            {t("Bağlantıyı dene")}
          </Button>
        </>
      }
    >
      <form id="site-migrate-form" onSubmit={submit} className="flex flex-col gap-5">
        <div className="grid grid-cols-[1fr_6.5rem] gap-3">
          <Input
            label={t("Eski sunucunun adresi")}
            value={host}
            onChange={(event) => setHost(event.target.value)}
            placeholder="sunucu.ornek.com"
            className="font-mono text-[13.5px]"
            spellCheck={false}
            autoComplete="off"
            autoFocus
            required
          />
          <Input label={t("SSH portu")} value={port} onChange={(event) => setPort(event.target.value)} inputMode="numeric" className="font-mono text-[13.5px]" required />
        </div>
        <Input
          label={t("SSH kullanıcı adı")}
          value={username}
          onChange={(event) => setUsername(event.target.value)}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
          required
        />
        <div>
          <Tabs<AuthMethod>
            label={t("Giriş yöntemi")}
            tabs={[
              { id: "password", label: t("Parola") },
              { id: "key", label: t("Özel anahtar") },
            ]}
            value={method}
            onChange={setMethod}
          />
          {method === "password" ? (
            <Input label={t("SSH parolası")} type="password" value={password} onChange={(event) => setPassword(event.target.value)} autoComplete="off" required />
          ) : (
            <Textarea
              label={t("Özel anahtar")}
              value={privateKey}
              onChange={(event) => setPrivateKey(event.target.value)}
              rows={5}
              placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
              hint={t("Anahtar dosyasının bütün içeriğini yapıştırın. Parolayla korunan anahtarlar desteklenmez.")}
              className="font-mono text-[12.5px]"
              spellCheck={false}
              required
            />
          )}
          <p className="mt-2 text-[13px] text-ink-3">{t("Giriş bilgileri yalnızca bu taşıma için kullanılır; panelde saklanmaz.")}</p>
        </div>
        <Input
          label={t("Sitenin eski sunucudaki klasörü")}
          value={path}
          onChange={(event) => setPath(event.target.value)}
          placeholder="/var/www/vhosts/ornek.com/httpdocs"
          hint={t("Sitenin yayın klasörünün tam yolu. cPanel'de çoğunlukla ~/public_html, Plesk'te /var/www/vhosts/alanadi/httpdocs.")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
          required
        />
        {probe.isError && <ErrorBlock message={errorMessage(probe.error)} />}
      </form>
    </Modal>
  );
}

/** "Yayın durumu" panelindeki satır: açıklama ve taşıma penceresini açan düğme. */
export function MigrateRow({ site }: { site: Site }) {
  // Araç kutusundan gelindiyse (?tasi=1) pencere kendiliğinden açılır.
  const [params, setParams] = useSearchParams();
  const [open, setOpen] = useState(() => params.get("tasi") === "1" && site.status === "active");
  function close() {
    setOpen(false);
    if (params.has("tasi")) {
      params.delete("tasi");
      setParams(params, { replace: true });
    }
  }
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
      <div className="max-w-[60ch]">
        <p className="font-medium">{t("Başka sunucudan taşı")}</p>
        <p className="text-[13.5px] text-ink-2">{t("Eski sunucudaki siteyi dosyaları ve WordPress veritabanıyla birlikte buraya çeker.")}</p>
      </div>
      <Button onClick={() => setOpen(true)} disabled={site.status !== "active"}>
        {t("Taşımayı başlat")}
      </Button>
      {open && <MigrateModal site={site} onClose={close} />}
    </div>
  );
}
