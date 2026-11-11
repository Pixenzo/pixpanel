// Geri yükleme sihirbazı ve yedekteki dosyaları gezen dosya seçici.

import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronRight, File, Folder, Link2, X } from "lucide-react";
import type { ReactNode } from "react";
import { useId, useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Select } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import type { BackupSources, Entry, JobStarted, Snapshot, Target } from "./BackupShared";
import { engineLabel, formatLongDateTime, Notice } from "./BackupShared";
import { JobLog } from "./Jobs";

// ---- Panel yedeği için açıklama ----

/** Panelin kendi yedeğinin arayüzden neden geri yüklenmediğini anlatır. */
export function PanelRestoreHelp({ onClose }: { onClose: () => void }) {
  return (
    <Modal open onClose={onClose} title={t("Panel yedeği nasıl geri yüklenir?")} footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      <div className="flex flex-col gap-3">
        <p>{t("Panelin kendi yedeği arayüzden geri yüklenemez; çünkü bunun gerektiği anda panel çalışmıyor olabilir.")}</p>
        <p>
          {t("Geri yükleme sunucuda komut satırından, ")}
          <Mono>docs/infra/felaket-kurtarma.md</Mono>
          {t(" dosyasındaki adımlar izlenerek yapılır.")}
        </p>
      </div>
    </Modal>
  );
}

// ---- Dosya seçici ----

interface FilesResponse {
  root: string;
  path: string;
  entries: Entry[];
}

export interface PickedEntry {
  path: string;
  kind: Entry["kind"];
}

/** Kökten bulunulan klasöre kadar olan yol parçaları; kökün yolu boş metindir. */
function buildCrumbs(root: string, current: string): { label: string; path: string }[] {
  const base = root.replace(/\/+$/, "");
  const rootLabel = base.split("/").filter(Boolean).pop() ?? "/";
  const crumbs = [{ label: rootLabel, path: "" }];
  if (current.startsWith(base) && current.length > base.length) {
    let walked = base;
    for (const part of current.slice(base.length).split("/").filter(Boolean)) {
      walked += `/${part}`;
      crumbs.push({ label: part, path: walked });
    }
  }
  return crumbs;
}

function EntryIcon({ kind }: { kind: Entry["kind"] }) {
  if (kind === "dir") return <Folder aria-hidden className="size-4 shrink-0 text-accent" />;
  if (kind === "symlink") return <Link2 aria-hidden className="size-4 shrink-0 text-ink-3" />;
  return <File aria-hidden className="size-4 shrink-0 text-ink-3" />;
}

function FileBrowser({
  targetId,
  snapshotId,
  picked,
  onChange,
}: {
  targetId: number;
  snapshotId: string;
  picked: PickedEntry[];
  onChange: (next: PickedEntry[]) => void;
}) {
  const [path, setPath] = useState("");
  const query = useQuery({
    queryKey: ["backup", "files", targetId, snapshotId, path],
    queryFn: () =>
      api.get<FilesResponse>(`/backup/targets/${targetId}/snapshots/${encodeURIComponent(snapshotId)}/files${path === "" ? "" : `?path=${encodeURIComponent(path)}`}`),
    placeholderData: keepPreviousData,
    retry: false,
  });

  const data = query.data;
  const crumbs = data ? buildCrumbs(data.root, data.path || data.root) : [];

  function toggle(entry: Entry, checked: boolean) {
    onChange(checked ? [...picked, { path: entry.path, kind: entry.kind }] : picked.filter((item) => item.path !== entry.path));
  }

  return (
    <div className="grid min-w-0 gap-4 lg:grid-cols-[minmax(0,1fr)_16rem]">
      <div className="flex min-w-0 flex-col gap-2">
        <nav aria-label={t("Klasör yolu")} className="flex min-h-6 flex-wrap items-center gap-1 text-[13.5px]">
          {crumbs.map((crumb, index) => (
            <span key={crumb.path} className="flex items-center gap-1">
              {index > 0 && <ChevronRight aria-hidden className="size-3.5 text-ink-3" />}
              {index === crumbs.length - 1 ? (
                <Mono className="font-medium">{crumb.label}</Mono>
              ) : (
                <button type="button" onClick={() => setPath(crumb.path)} className="text-accent hover:underline">
                  <Mono>{crumb.label}</Mono>
                </button>
              )}
            </span>
          ))}
        </nav>

        <div className="h-72 overflow-y-auto rounded-md border border-line">
          {query.isError && !data ? (
            <div className="p-3">
              <ErrorBlock message={errorMessage(query.error)} onRetry={() => void query.refetch()} />
            </div>
          ) : !data ? (
            <LoadingBlock />
          ) : data.entries.length === 0 ? (
            <p className="px-4 sm:px-6 py-6 text-ink-3">{t("Bu klasör boş.")}</p>
          ) : (
            <ul className={classNames(query.isFetching && "opacity-60 transition-opacity")}>
              {data.entries.map((entry) => (
                <li key={entry.path} className="flex items-center gap-2.5 border-b border-line px-3 py-1.5 last:border-b-0">
                  <input
                    type="checkbox"
                    aria-label={t("{name} seç", { name: entry.name })}
                    className="size-4 shrink-0 accent-(--accent)"
                    checked={picked.some((item) => item.path === entry.path)}
                    onChange={(event) => toggle(entry, event.target.checked)}
                  />
                  <EntryIcon kind={entry.kind} />
                  {entry.kind === "dir" ? (
                    <button type="button" onClick={() => setPath(entry.path)} className="min-w-0 flex-1 truncate text-left font-medium text-accent hover:underline">
                      {entry.name}
                    </button>
                  ) : (
                    <span className="min-w-0 flex-1 truncate">{entry.name}</span>
                  )}
                  <span className="w-20 shrink-0 text-right text-[13px] text-ink-3 tabular-nums">{entry.kind === "file" ? formatBytes(entry.size) : ""}</span>
                  <span className="hidden w-32 shrink-0 text-right text-[13px] text-ink-3 tabular-nums sm:block">{formatDateTime(entry.mtime)}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex min-h-6 items-center justify-between text-[13.5px] font-medium text-ink-2">
          <span>{t("Seçilenler ({n})", { n: picked.length })}</span>
          {picked.length > 0 && (
            <button type="button" onClick={() => onChange([])} className="text-accent hover:underline">
              {t("Temizle")}
            </button>
          )}
        </div>
        <ul className="h-72 overflow-y-auto rounded-md border border-line">
          {picked.length === 0 && <li className="px-3 py-3 text-ink-3">{t("Soldaki listeden dosya ya da klasör işaretleyin.")}</li>}
          {picked.map((item) => (
            <li key={item.path} className="flex items-center gap-1 border-b border-line py-0.5 pr-1 pl-3 last:border-b-0">
              <span className="min-w-0 flex-1 break-all">
                <Mono>{item.path}</Mono>
              </span>
              <IconButton label={t("{path} seçimini kaldır", { path: item.path })} onClick={() => onChange(picked.filter((other) => other.path !== item.path))}>
                <X className="size-4" />
              </IconButton>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}

// ---- Sihirbaz ----

function Choice({ name, checked, onChange, children, hint, disabled }: { name: string; checked: boolean; onChange: () => void; children: ReactNode; hint?: string; disabled?: boolean }) {
  return (
    <label className={classNames("flex items-start gap-2.5", disabled && "opacity-55")}>
      <input type="radio" name={name} className="mt-1 accent-(--accent)" checked={checked} disabled={disabled} onChange={onChange} />
      <span>
        <span className="text-ink">{children}</span>
        {hint && <span className="block text-[13px] text-ink-3">{hint}</span>}
      </span>
    </label>
  );
}

function Question({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="font-semibold">{title}</h3>
      {children}
    </section>
  );
}

type Destination = "same" | "other";

export function RestoreWizard({ target, snapshot, sources, onClose }: { target: Target; snapshot: Snapshot; sources: BackupSources; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const groupId = useId();

  const isSite = snapshot.kind === "site";
  const isDb = snapshot.kind === "db";
  const isMail = snapshot.kind === "mail";
  const hasFiles = isSite || isMail;

  // Yedeklenen şeyin şu an var olup olmadığı.
  const existingSite = isSite ? sources.sites.find((site) => site.domain === snapshot.name) : undefined;
  const existingDb = isDb ? sources.databases.find((database) => database.name === snapshot.name && database.engine === snapshot.engine) : undefined;
  const missingSite = isSite && !existingSite;
  const missingDb = isDb && !existingDb;

  const otherSites = sources.sites.filter((site) => site.domain !== snapshot.name);
  const otherDatabases = sources.databases.filter((database) => database.engine === snapshot.engine && database.id !== existingDb?.id);

  const [mode, setMode] = useState<"all" | "some">("all");
  const [picked, setPicked] = useState<PickedEntry[]>([]);
  const [destination, setDestination] = useState<Destination>("same");
  const [siteChoice, setSiteChoice] = useState<number | null>(null);
  const [databaseChoice, setDatabaseChoice] = useState<number | null>(null);
  const [deleteExtra, setDeleteExtra] = useState(false);
  const [typed, setTyped] = useState("");
  const [job, setJob] = useState<{ id: number } | null>(null);

  const when = formatLongDateTime(snapshot.time);
  const subject = isSite ? t("{name} sitesi", { name: snapshot.name }) : isDb ? t("{name} veritabanı", { name: snapshot.name }) : t("{name} posta alanı", { name: snapshot.name });

  const showBrowser = hasFiles && mode === "some" && !missingSite;
  const inPlace = destination === "same" || mode === "some" || missingSite;
  const destSiteId = otherSites.some((site) => site.id === siteChoice) ? siteChoice : (otherSites[0]?.id ?? null);
  const destDbId = otherDatabases.some((database) => database.id === databaseChoice) ? databaseChoice : (otherDatabases[0]?.id ?? null);
  const destSite = otherSites.find((site) => site.id === destSiteId);
  const destDb = otherDatabases.find((database) => database.id === destDbId);

  const restore = useMutation({
    mutationFn: () =>
      api.post<JobStarted>("/backup/restore", {
        target_id: target.id,
        snapshot: snapshot.id,
        ...(hasFiles && mode === "some" && picked.length > 0 ? { paths: picked.map((item) => item.path) } : {}),
        ...(hasFiles && deleteExtra ? { delete_extra: true } : {}),
        ...(isSite && !inPlace && destSiteId !== null ? { destination_site_id: destSiteId } : {}),
        ...(isDb && destination === "other" && destDbId !== null ? { destination_database_id: destDbId } : {}),
      }),
    onSuccess: (started) => setJob({ id: started.job_id }),
  });

  const otherChosen = isSite ? destSite !== undefined : isDb ? destDb !== undefined : true;
  const problem = (() => {
    if (showBrowser && picked.length === 0) return t("Geri yüklenecek en az bir dosya ya da klasör seçin.");
    if (destination === "other" && !otherChosen) return t("Hangisine yükleneceğini seçin.");
    return null;
  })();
  const needsTyping = inPlace && (isSite || isDb || isMail);
  const typedOk = !needsTyping || typed === snapshot.name;

  const summary = (() => {
    const tail = hasFiles ? (deleteExtra ? ` ${t("Yedekte olmayan dosyalar silinir.")}` : ` ${t("Aynı adlı dosyaların üzerine yazılır; başka dosyalara dokunulmaz.")}`) : "";
    if (isDb) {
      if (missingDb) return t("{name} veritabanı yeniden oluşturulacak ve {when} tarihli yedek yüklenecek; veritabanı kullanıcılarına yeniden yetki vermeniz gerekir.", { name: snapshot.name, when });
      if (destination === "other" && destDb) return t("{name} yedeği {other} veritabanına yüklenecek; oradaki mevcut tablolar yedektekilerle değiştirilir.", { name: snapshot.name, other: destDb.name });
      return t("{name} veritabanı {when} tarihli yedeğine döndürülecek; mevcut tablolar yedektekilerle değiştirilir.", { name: snapshot.name, when });
    }
    if (missingSite) return t("{name} sitesi yedekteki ayarlarıyla yeniden kurulacak ve dosyaları {when} tarihli yedekten geri yüklenecek.", { name: snapshot.name, when }) + tail;
    if (isSite && !inPlace && destSite) return t("{name} sitesinin {when} tarihli yedeği {other} sitesinin klasörüne yüklenecek.", { name: snapshot.name, when, other: destSite.domain }) + tail;
    if (mode === "some") return t("Seçtiğiniz {n} dosya ve klasör {when} tarihli yedekten {subject} içine geri yüklenecek.", { n: picked.length, when, subject }) + tail;
    return t("{subject} içindeki her şey {when} tarihli yedekten geri yüklenecek.", { subject, when }) + tail;
  })();

  function finish(status: string) {
    if (status === "success") feedback.success(t("Geri yükleme tamamlandı"));
    void queryClient.invalidateQueries({ queryKey: ["backup"] });
  }

  // ---- Görev günlüğü adımı ----
  if (job) {
    return (
      <Modal open onClose={onClose} wide title={t("Geri yükleniyor")} description={t("{subject} — {when} yedeği", { subject, when })} footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
        <JobLog jobId={job.id} onDone={finish} />
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      wide={!showBrowser}
      extraWide={showBrowser}
      title={t("Geri yükle")}
      description={t("{subject} — {when} yedeği", { subject, when })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={restore.isPending} disabled={problem !== null || !typedOk} onClick={() => restore.mutate()}>
            {t("Geri yükle")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        {/* Ne geri yüklensin? (site ve posta) */}
        {hasFiles && !missingSite && (
          <Question title={t("Ne geri yüklensin?")}>
            <Choice name={`${groupId}-mode`} checked={mode === "all"} onChange={() => setMode("all")}>
              {t("Her şeyi geri yükle")}
            </Choice>
            <Choice name={`${groupId}-mode`} checked={mode === "some"} onChange={() => { setMode("some"); setDestination("same"); }}>
              {t("Yalnızca seçtiğim dosya ve klasörleri")}
            </Choice>
          </Question>
        )}

        {showBrowser && <FileBrowser targetId={target.id} snapshotId={snapshot.id} picked={picked} onChange={setPicked} />}

        {/* Nereye? */}
        {isSite && missingSite && <Notice tone="accent">{t("Bu site artık yok. Yedekteki ayarlarıyla yeniden kurulacak ve dosyaları geri yüklenecek.")}</Notice>}

        {isSite && !missingSite && mode === "all" && (
          <Question title={t("Nereye?")}>
            <Choice name={`${groupId}-dest`} checked={destination === "same"} onChange={() => setDestination("same")}>
              {t("Aynı siteye")}
            </Choice>
            <Choice
              name={`${groupId}-dest`}
              checked={destination === "other"}
              disabled={otherSites.length === 0}
              hint={otherSites.length === 0 ? t("Başka site yok.") : undefined}
              onChange={() => setDestination("other")}
            >
              {t("Başka bir siteye")}
            </Choice>
            {destination === "other" && otherSites.length > 0 && (
              <div className="max-w-sm pl-6">
                <Select label={t("Hangi site?")} value={destSiteId ?? ""} onChange={(event) => setSiteChoice(Number(event.target.value))}>
                  {otherSites.map((site) => (
                    <option key={site.id} value={site.id}>
                      {site.domain}
                    </option>
                  ))}
                </Select>
              </div>
            )}
          </Question>
        )}

        {isDb && (
          <Question title={t("Nereye?")}>
            <Choice
              name={`${groupId}-dest`}
              checked={destination === "same"}
              hint={missingDb ? t("Bu veritabanı artık yok; yeniden oluşturulacak. Veritabanı kullanıcılarına yeniden yetki vermeniz gerekir.") : undefined}
              onChange={() => setDestination("same")}
            >
              {t("Aynı veritabanına")}
            </Choice>
            <Choice
              name={`${groupId}-dest`}
              checked={destination === "other"}
              disabled={otherDatabases.length === 0}
              hint={otherDatabases.length === 0 ? t("Aynı türde başka veritabanı yok.") : undefined}
              onChange={() => setDestination("other")}
            >
              {t("Başka bir veritabanına")}
            </Choice>
            {destination === "other" && otherDatabases.length > 0 && (
              <div className="max-w-sm pl-6">
                <Select label={t("Hangi veritabanı?")} value={destDbId ?? ""} onChange={(event) => setDatabaseChoice(Number(event.target.value))}>
                  {otherDatabases.map((database) => (
                    <option key={database.id} value={database.id}>
                      {database.name} ({engineLabel(database.engine)})
                    </option>
                  ))}
                </Select>
              </div>
            )}
            <Notice tone="warn">{t("Mevcut tablolar yedektekilerle değiştirilir.")}</Notice>
          </Question>
        )}

        {/* Yedekte olmayan dosyalar */}
        {hasFiles && (
          <div className="flex flex-col gap-2">
            <label className="flex items-start gap-2.5">
              <input type="checkbox" className="mt-0.5 size-4 accent-(--accent)" checked={deleteExtra} onChange={(event) => setDeleteExtra(event.target.checked)} />
              <span>{t("Yedekte olmayan dosyaları sil (klasör yedekteki hâline birebir döner)")}</span>
            </label>
            {deleteExtra && (
              <Notice tone="bad">{t("Dikkat: şu an klasörde olup yedekte bulunmayan bütün dosyalar silinir. Yedekten sonra yapılan yüklemeler ve değişiklikler kaybolur.")}</Notice>
            )}
          </div>
        )}

        {/* Özet ve onay */}
        <div className="flex flex-col gap-3 border-t border-line pt-4">
          <p className="font-medium">{summary}</p>
          {needsTyping && (
            <div className="max-w-sm">
              <Input
                label={t("Onaylamak için “{text}” yazın", { text: snapshot.name })}
                value={typed}
                onChange={(event) => setTyped(event.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
          )}
          {problem && <p className="text-[13px] text-ink-3">{problem}</p>}
          {restore.isError && <Notice tone="bad">{errorMessage(restore.error)}</Notice>}
        </div>
      </div>
    </Modal>
  );
}
