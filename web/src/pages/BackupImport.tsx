// Başka panelin yedek dosyasından içe aktarma (cPanel, Plesk, HestiaCP): yol girilir, yedek
// incelenir, bulunan alan adları ve veritabanları onay kutularıyla seçilir, hedef hesap seçilip
// içe aktarılır. İnceleme ve içe aktarma arka plan görevleridir; çıktıları canlı gösterilir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useMemo, useState } from "react";
import { Badge, Button, Checkbox, ErrorBlock, Input, LoadingBlock, Modal, Mono, Select } from "../components/ui";
import type { SiteOptions } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

interface BackupDomain {
  name: string;
  kind: "main" | "addon" | "subdomain" | "domain";
  docroot_path: string | null;
  size_bytes: number | null;
  wordpress: boolean;
}

interface BackupDatabase {
  name: string;
  engine: "mysql" | "postgresql";
  dump_path: string | null;
  users: string[];
  domain: string | null;
}

interface BackupManifest {
  format: "cpanel" | "plesk" | "hestia";
  domains: BackupDomain[];
  databases: BackupDatabase[];
  mailboxes: string[];
  notes: string[];
}

interface Inspection {
  inspect_id: string;
  status: "running" | "ready" | "failed";
  error: string | null;
  job_id: number | null;
  source: string;
  created_at: number;
  manifest: BackupManifest | null;
}

const formatNames: Record<BackupManifest["format"], string> = { cpanel: "cPanel", plesk: "Plesk", hestia: "HestiaCP" };

function kindLabel(kind: BackupDomain["kind"]): string {
  switch (kind) {
    case "main":
      return t("Ana alan adı");
    case "addon":
      return t("Ek alan adı");
    case "subdomain":
      return t("Alt alan adı");
    default:
      return t("Alan adı");
  }
}

/** Hedef alan adı yazılmadıkça içe aktarılamayan (alan adı biçiminde olmayan) kaynak adı mı? */
function needsTarget(name: string): boolean {
  return !/^[a-z0-9-]+(\.[a-z0-9-]+)+$/i.test(name);
}

type Step = "path" | "inspecting" | "preview" | "importing";

export function BackupImportModal({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient();
  const [step, setStep] = useState<Step>("path");
  const [path, setPath] = useState("");
  const [inspectId, setInspectId] = useState<string | null>(null);
  const [inspectJob, setInspectJob] = useState<number | null>(null);
  const [owner, setOwner] = useState("");
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const [targets, setTargets] = useState<Record<string, string>>({});
  const [pickedDatabases, setPickedDatabases] = useState<Record<string, boolean>>({});
  const [attached, setAttached] = useState<Record<string, string>>({});
  const [importJob, setImportJob] = useState<number | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const [cleaned, setCleaned] = useState(false);

  const options = useQuery({ queryKey: ["site-options"], queryFn: () => api.get<SiteOptions>("/sites/options") });

  const inspection = useQuery({
    queryKey: ["backup-import", inspectId],
    enabled: inspectId !== null,
    queryFn: () => api.get<Inspection>(`/migrate/backup/${inspectId}`),
    refetchInterval: (query) => (query.state.data?.status === "running" ? 1500 : false),
  });
  const manifest = inspection.data?.status === "ready" ? inspection.data.manifest : null;

  const inspect = useMutation({
    mutationFn: () => api.post<{ inspect_id: string; job_id: number }>("/migrate/backup/inspect", { path: path.trim() }),
    onSuccess: (response) => {
      setInspectId(response.inspect_id);
      setInspectJob(response.job_id);
      setStep("inspecting");
    },
  });

  // İnceleme bitince varsayılan seçimler kurulur: bulunabilen her şey seçili gelir.
  const [prepared, setPrepared] = useState<string | null>(null);
  if (manifest && inspectId && prepared !== inspectId) {
    setPrepared(inspectId);
    const domains: Record<string, boolean> = {};
    const names: Record<string, string> = {};
    for (const domain of manifest.domains) {
      domains[domain.name] = domain.docroot_path !== null;
      names[domain.name] = needsTarget(domain.name) ? "" : domain.name;
    }
    const databases: Record<string, boolean> = {};
    for (const database of manifest.databases) databases[database.name] = database.engine === "mysql" && database.dump_path !== null;
    setPicked(domains);
    setTargets(names);
    setPickedDatabases(databases);
    setAttached({});
    setStep("preview");
  }

  const owners = options.data?.owners ?? [];
  const selectedDomains = useMemo(() => (manifest ? manifest.domains.filter((domain) => picked[domain.name] && domain.docroot_path !== null) : []), [manifest, picked]);

  /** Veritabanının bağlanacağı alan adı: seçilmişse kullanıcının tercihi, yoksa yedekteki ilişki, yoksa ilk seçili alan adı. */
  function attachedTo(database: BackupDatabase): string | undefined {
    const names = selectedDomains.map((domain) => domain.name);
    const chosen = attached[database.name];
    if (chosen && names.includes(chosen)) return chosen;
    if (database.domain && names.includes(database.domain)) return database.domain;
    return names[0];
  }

  const targetsMissing = selectedDomains.some((domain) => (targets[domain.name] ?? "").trim() === "" && needsTarget(domain.name));
  const canImport = selectedDomains.length > 0 && owner !== "" && !targetsMissing && !cleaned;

  const start = useMutation({
    mutationFn: () =>
      api.post<{ job_id: number }>("/migrate/backup/import", {
        inspect_id: inspectId,
        owner_user_id: Number(owner),
        items: selectedDomains.map((domain) => ({
          domain: domain.name,
          ...((targets[domain.name] ?? "").trim() !== "" && targets[domain.name].trim() !== domain.name ? { target_domain: targets[domain.name].trim() } : {}),
          databases: (manifest?.databases ?? []).filter((database) => pickedDatabases[database.name] && attachedTo(database) === domain.name).map((database) => database.name),
        })),
      }),
    onSuccess: (response) => {
      setImportJob(response.job_id);
      setStep("importing");
    },
  });

  const clean = useMutation({
    mutationFn: () => api.delete(`/migrate/backup/${inspectId}`),
    onSuccess: () => setCleaned(true),
  });

  function finished(status: string) {
    setResult(status);
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
    void queryClient.invalidateQueries({ queryKey: ["databases"] });
  }

  function submitPath(event: FormEvent) {
    event.preventDefault();
    inspect.mutate();
  }

  function backToStart() {
    setStep("path");
    setInspectId(null);
    setInspectJob(null);
    setPrepared(null);
    setCleaned(false);
    inspect.reset();
  }

  // ---- 1. Yol ----
  if (step === "path") {
    return (
      <Modal
        open
        onClose={onClose}
        title={t("Yedek dosyasından içe aktar")}
        description={t("cPanel, Plesk ya da HestiaCP yedeğindeki siteleri ve veritabanlarını bu sunucuya aktarır. Yedek önce güvenli bir geçici klasöre açılıp içindekiler size gösterilir.")}
        footer={
          <>
            <Button onClick={onClose}>{t("Vazgeç")}</Button>
            <Button variant="primary" type="submit" form="backup-import-form" loading={inspect.isPending} disabled={path.trim() === ""}>
              {t("Yedeği incele")}
            </Button>
          </>
        }
      >
        <form id="backup-import-form" onSubmit={submitPath} className="flex flex-col gap-4">
          <Input
            label={t("Yedek dosyasının sunucudaki yolu")}
            value={path}
            onChange={(event) => setPath(event.target.value)}
            placeholder="/home/hesap/cpmove-kullanici.tar.gz"
            hint={t("Yedeği önce Dosyalar sayfasından ya da SFTP ile bir hesap klasörüne yükleyin, sonra tam yolunu yazın. cpmove-*.tar.gz, backup_*.tar ve kullanici.tarih.tar dosyaları ile açılmış yedek klasörleri desteklenir.")}
            className="font-mono text-[13.5px]"
            spellCheck={false}
            autoComplete="off"
            autoFocus
            required
          />
          <p className="text-[13px] text-ink-3">{t("Zstandard ile sıkıştırılmış yedekler (HestiaCP) için sunucuda zstd paketi kurulu olmalıdır.")}</p>
          {inspect.isError && <ErrorBlock message={errorMessage(inspect.error)} />}
        </form>
      </Modal>
    );
  }

  // ---- 2. İnceleme ----
  if (step === "inspecting") {
    const failed = inspection.data?.status === "failed";
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={failed ? t("Yedek incelenemedi") : t("Yedek inceleniyor")}
        footer={
          failed ? (
            <>
              <Button onClick={onClose}>{t("Kapat")}</Button>
              <Button variant="primary" onClick={backToStart}>
                {t("Başka yol dene")}
              </Button>
            </>
          ) : (
            <Button onClick={onClose}>{t("Arka planda sürsün")}</Button>
          )
        }
      >
        {inspectJob !== null && <JobLog jobId={inspectJob} />}
        {failed && inspection.data?.error && (
          <div className="mt-3">
            <ErrorBlock message={inspection.data.error} />
          </div>
        )}
        {inspection.isError && (
          <div className="mt-3">
            <ErrorBlock message={errorMessage(inspection.error)} />
          </div>
        )}
      </Modal>
    );
  }

  // ---- 4. İçe aktarma ----
  if (step === "importing" && importJob !== null) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={result === "success" ? t("İçe aktarma tamamlandı") : t("İçe aktarılıyor")}
        footer={
          <>
            {result !== null && !cleaned && (
              <Button loading={clean.isPending} onClick={() => clean.mutate()}>
                {t("Geçici dosyaları sil")}
              </Button>
            )}
            <Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>
          </>
        }
      >
        <JobLog jobId={importJob} onDone={finished} />
        {result === "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">
            {t("Siteler ve veritabanları aktarıldı. Alan adlarının DNS kayıtlarını bu sunucuya çevirmeden önce siteleri denetleyin. Posta kutuları aktarılmadı; Posta sayfasından IMAP ile aktarabilirsiniz.")}
          </p>
        )}
        {result !== null && result !== "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">{t("İçe aktarma tamamlanamadı. Bu işlemde oluşturulan siteler ve veritabanları geri alındı; sorunu giderip yeniden başlatabilirsiniz.")}</p>
        )}
        {clean.isError && (
          <div className="mt-3">
            <ErrorBlock message={errorMessage(clean.error)} />
          </div>
        )}
        {cleaned && <p className="mt-3 text-[13.5px] text-ink-2">{t("Geçici dosyalar silindi.")}</p>}
      </Modal>
    );
  }

  // ---- 3. Önizleme ----
  return (
    <Modal
      open
      onClose={onClose}
      extraWide
      title={t("Yedeğin içeriği")}
      description={manifest ? t("{format} yedeği. İçe aktarmak istediklerinizi seçin.", { format: formatNames[manifest.format] }) : undefined}
      footer={
        <>
          <Button onClick={backToStart}>{t("Geri")}</Button>
          <Button variant="primary" loading={start.isPending} disabled={!canImport || manifest === null} onClick={() => start.mutate()}>
            {t("İçe aktar")}
          </Button>
        </>
      }
    >
      {(inspection.isPending || !manifest) && <LoadingBlock />}
      {manifest && (
        <div className="flex flex-col gap-6">
          <section className="flex flex-col gap-3">
            <h3 className="text-[15px] font-semibold">{t("Alan adları ve siteler")}</h3>
            {manifest.domains.length === 0 && <p className="text-ink-2">{t("Yedekte site bulunamadı.")}</p>}
            <ul className="flex flex-col divide-y divide-line rounded-xl border border-line">
              {manifest.domains.map((domain) => {
                const found = domain.docroot_path !== null;
                return (
                  <li key={domain.name} className="flex flex-col gap-2 px-4 py-3">
                    <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                      <Checkbox
                        label={domain.name}
                        checked={picked[domain.name] === true && found}
                        disabled={!found}
                        onChange={(event) => setPicked({ ...picked, [domain.name]: event.target.checked })}
                      />
                      <Badge tone="idle">{kindLabel(domain.kind)}</Badge>
                      {domain.wordpress && <Badge tone="accent">WordPress</Badge>}
                      {found && domain.size_bytes !== null && <span className="text-[13px] text-ink-2">{formatBytes(domain.size_bytes)}</span>}
                      {!found && <span className="text-[13px] text-bad">{t("Site içeriği yedekte bulunamadı")}</span>}
                    </div>
                    {found && picked[domain.name] && (
                      <div className="ml-6 max-w-md">
                        <Input
                          label={t("Bu sunucudaki alan adı")}
                          value={targets[domain.name] ?? ""}
                          onChange={(event) => setTargets({ ...targets, [domain.name]: event.target.value })}
                          placeholder={domain.name}
                          hint={needsTarget(domain.name) ? t("Yedekte alan adı belli değil; sitenin açılacağı alan adını yazın.") : t("Farklı bir alan adıyla açmak için değiştirin; WordPress adresleri buna göre çevrilir.")}
                          className="font-mono text-[13.5px]"
                          spellCheck={false}
                          autoComplete="off"
                        />
                      </div>
                    )}
                  </li>
                );
              })}
            </ul>
          </section>

          <section className="flex flex-col gap-3">
            <h3 className="text-[15px] font-semibold">{t("Veritabanları")}</h3>
            {manifest.databases.length === 0 && <p className="text-ink-2">{t("Yedekte veritabanı bulunamadı.")}</p>}
            {manifest.databases.length > 0 && (
              <ul className="flex flex-col divide-y divide-line rounded-xl border border-line">
                {manifest.databases.map((database) => {
                  const importable = database.engine === "mysql" && database.dump_path !== null;
                  return (
                    <li key={database.name} className="flex flex-col gap-2 px-4 py-3">
                      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
                        <Checkbox
                          label={database.name}
                          checked={pickedDatabases[database.name] === true && importable}
                          disabled={!importable}
                          onChange={(event) => setPickedDatabases({ ...pickedDatabases, [database.name]: event.target.checked })}
                        />
                        <Badge tone="idle">{database.engine === "mysql" ? "MySQL" : "PostgreSQL"}</Badge>
                        {database.engine !== "mysql" && <span className="text-[13px] text-ink-2">{t("PostgreSQL dökümleri içe aktarılmaz; atlanır")}</span>}
                        {database.engine === "mysql" && database.dump_path === null && <span className="text-[13px] text-bad">{t("Döküm yedekte bulunamadı")}</span>}
                        {database.users.length > 0 && <span className="text-[13px] text-ink-3">{t("Kullanıcı: {users}", { users: database.users.join(", ") })}</span>}
                      </div>
                      {importable && pickedDatabases[database.name] && selectedDomains.length > 1 && (
                        <div className="ml-6 max-w-xs">
                          <Select
                            label={t("Hangi siteye bağlansın")}
                            value={attachedTo(database) ?? ""}
                            onChange={(event) => setAttached({ ...attached, [database.name]: event.target.value })}
                          >
                            {selectedDomains.map((domain) => (
                              <option key={domain.name} value={domain.name}>
                                {domain.name}
                              </option>
                            ))}
                          </Select>
                        </div>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
            <p className="text-[13px] text-ink-3">
              {t("Her veritabanı hedef hesapta yeni bir veritabanı olarak açılır. WordPress sitesinin wp-config.php dosyası, diğer uygulamaların .env dosyası yeni bilgilere çevrilir; bilgiler hesabın tmp klasörüne de yazılır.")}
            </p>
          </section>

          <section className="flex flex-col gap-2">
            <h3 className="text-[15px] font-semibold">{t("Posta kutuları ve DNS")}</h3>
            <p className="text-[13.5px] text-ink-2">{t("Posta kutuları ve DNS kayıtları bu yolla içe aktarılmaz. Posta kutuları IMAP ile aktarılabilir: kutuları Posta sayfasında açın, ardından her kutunun içe aktarma düğmesini kullanın.")}</p>
            {manifest.mailboxes.length > 0 ? (
              <p className="text-[13px] text-ink-2">
                {t("Yedekte bulunan kutular ({count}):", { count: manifest.mailboxes.length })} <Mono className="break-all">{manifest.mailboxes.slice(0, 30).join(", ")}{manifest.mailboxes.length > 30 ? " …" : ""}</Mono>
              </p>
            ) : (
              <p className="text-[13px] text-ink-3">{t("Yedekte posta kutusu bulunamadı.")}</p>
            )}
          </section>

          {manifest.notes.length > 0 && (
            <section className="flex flex-col gap-1.5 rounded-xl border border-warn/40 bg-warn-soft px-4 py-3 text-[13.5px] text-warn">
              {manifest.notes.map((note) => (
                <p key={note}>{note}</p>
              ))}
            </section>
          )}

          <section className="flex flex-col gap-3">
            <Select label={t("Hedef hesap")} value={owner} onChange={(event) => setOwner(event.target.value)} hint={t("Siteler ve veritabanları bu hesapta açılır; hesabın paket sınırları geçerlidir.")}>
              <option value="">{t("Hesap seçin")}</option>
              {owners.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.username}
                </option>
              ))}
            </Select>
            {options.isError && <ErrorBlock message={errorMessage(options.error)} />}
            {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
            <p className="text-[13px] text-ink-3">{t("Geçici dosyalar 24 saat sonra kendiliğinden silinir.")}</p>
            {!cleaned ? (
              <div>
                <Button size="sm" loading={clean.isPending} onClick={() => clean.mutate()}>
                  {t("Geçici dosyaları şimdi sil")}
                </Button>
              </div>
            ) : (
              <p className="text-[13.5px] text-ink-2">{t("Geçici dosyalar silindi; içe aktarmak için yedeği yeniden inceleyin.")}</p>
            )}
          </section>
        </div>
      )}
    </Modal>
  );
}
