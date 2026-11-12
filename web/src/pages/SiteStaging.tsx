// WordPress hazırlık ortamı: asıl sitede kopyayı açan, kopyada ise değişiklikleri canlıya
// aktaran ya da kopyayı canlıdan yenileyen bölüm. Aktarım görevinin çıktısı canlı gösterilir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDownToLine, ArrowUpFromLine } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { Badge, Button, Checkbox, ErrorBlock, Input, Modal, Mono, Panel } from "../components/ui";
import type { Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

interface SiteRef {
  id: number;
  domain: string;
}

interface StagingInfo {
  /** Site bir hazırlık ortamıysa asıl sitesi. */
  live: SiteRef | null;
  /** Sitenin hazırlık ortamları. */
  copies: SiteRef[];
  suggested: string;
}

type Direction = "push" | "refresh";

function CreateModal({ site, suggested, onClose }: { site: Site; suggested: string; onClose: () => void }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [domain, setDomain] = useState(suggested);
  const [started, setStarted] = useState<{ job_id: number; site_id: number } | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const start = useMutation({
    mutationFn: () => api.post<{ job_id: number; site_id: number }>(`/sites/${site.id}/wordpress/staging`, { domain: domain.trim() }),
    onSuccess: (response) => {
      setStarted(response);
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    start.mutate();
  }

  function finished(status: string) {
    setResult(status);
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
    void queryClient.invalidateQueries({ queryKey: ["wordpress-staging"] });
    void queryClient.invalidateQueries({ queryKey: ["databases"] });
  }

  if (started) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={result === "success" ? t("Hazırlık ortamı hazır") : t("Hazırlık ortamı açılıyor")}
        footer={
          <>
            <Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>
            {result === "success" && (
              <Button variant="primary" onClick={() => void navigate(`/sites/${started.site_id}/wordpress`)}>
                {t("Hazırlık ortamına git")}
              </Button>
            )}
          </>
        }
      >
        <JobLog jobId={started.job_id} onDone={finished} />
        {result !== null && result !== "success" && <p className="mt-3 text-[13.5px] text-ink-2">{t("Hazırlık ortamı açılamadı; yarım kalan kopya kaldırıldı. Asıl siteniz etkilenmedi.")}</p>}
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("Hazırlık ortamı aç")}
      description={t("Site, dosyaları ve veritabanıyla birlikte ayrı bir adrese kopyalanır. Güncellemeleri ve değişiklikleri önce orada dener, beğenirseniz tek tıkla canlıya aktarırsınız.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="staging-create-form" loading={start.isPending}>
            {t("Hazırlık ortamını aç")}
          </Button>
        </>
      }
    >
      <form id="staging-create-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input
          label={t("Hazırlık ortamının adresi")}
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          hint={t("Bu adın DNS kaydı sunucuyu göstermelidir. Arama motorları hazırlık ortamını dizine eklemez.")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
          autoFocus
          required
        />
        {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      </form>
    </Modal>
  );
}

function SyncModal({ copy, live, direction, onClose }: { copy: Site; live: SiteRef; direction: Direction; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [files, setFiles] = useState(true);
  const [database, setDatabase] = useState(true);
  const [jobId, setJobId] = useState<number | null>(null);
  const [result, setResult] = useState<string | null>(null);
  const push = direction === "push";
  const target = push ? live.domain : copy.domain;

  const start = useMutation({
    mutationFn: () => api.post<{ job_id: number }>(`/sites/${copy.id}/wordpress/staging/sync`, { direction, files, database }),
    onSuccess: (response) => setJobId(response.job_id),
  });

  function finished(status: string) {
    setResult(status);
    void queryClient.invalidateQueries({ queryKey: ["wordpress"] });
  }

  if (jobId !== null) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={
          result === "success"
            ? t("{domain} güncellendi", { domain: target })
            : push
              ? t("Canlıya aktarılıyor")
              : t("Hazırlık ortamı yenileniyor")
        }
        footer={<Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>}
      >
        <JobLog jobId={jobId} onDone={finished} />
        {result !== null && result !== "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">
            {t("Aktarım tamamlanamadı. Veritabanı yarıda kaldıysa eski hâline döndürüldü; ayrıntı yukarıdaki günlükte.")}
          </p>
        )}
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={push ? t("{domain} sitesine aktar", { domain: live.domain }) : t("Hazırlık ortamını canlıdan yenile")}
      description={
        push
          ? t("Hazırlık ortamındaki site, canlı sitenin üzerine yazılır. Aktarım sürerken canlı site kısa süre bakım iletisi gösterir.")
          : t("Canlı sitenin bugünkü hâli hazırlık ortamının üzerine yazılır; hazırlık ortamında yaptığınız değişiklikler silinir.")
      }
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant={push ? "danger" : "primary"} loading={start.isPending} disabled={!files && !database} onClick={() => start.mutate()}>
            {push ? t("Canlıya aktar") : t("Canlıdan yenile")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Checkbox
          label={t("Dosyalar")}
          hint={t("Çekirdek, eklentiler, temalar ve yüklenen dosyalar. Hedefte yüklenmiş ortam dosyaları silinmez; wp-config.php yerinde kalır.")}
          checked={files}
          onChange={(event) => setFiles(event.target.checked)}
        />
        <Checkbox
          label={t("Veritabanı")}
          hint={
            push
              ? t("Yazılar, sayfalar, ayarlar ve kullanıcılar. Hazırlık ortamı açıldıktan sonra canlı sitede oluşan siparişler, yorumlar ve üyelikler kaybolur.")
              : t("Yazılar, sayfalar, ayarlar ve kullanıcılar; adresler hazırlık ortamının adresine çevrilir.")
          }
          checked={database}
          onChange={(event) => setDatabase(event.target.checked)}
        />
        {push && (
          <p className="rounded-lg border border-line bg-sunken/50 px-3.5 py-3 text-[13.5px] text-ink-2">
            {t("Yedek yeri tanımlıysa aktarımdan önce canlı sitenin dosyaları ve veritabanı kendiliğinden yedeklenir. Sitede satış ya da üyelik varsa yalnızca dosyaları aktarmayı düşünün.")}
          </p>
        )}
        {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      </div>
    </Modal>
  );
}

/** WordPress bölümündeki "Hazırlık ortamı" paneli. */
export function StagingPanel({ site }: { site: Site }) {
  const [creating, setCreating] = useState(false);
  const [syncing, setSyncing] = useState<Direction | null>(null);
  const info = useQuery({ queryKey: ["wordpress-staging", site.id], queryFn: () => api.get<StagingInfo>(`/sites/${site.id}/wordpress/staging`) });
  if (!info.data) return null;
  const { live, copies, suggested } = info.data;

  if (live) {
    return (
      <Panel title={t("Hazırlık ortamı")}>
        <div className="flex flex-wrap items-center gap-x-6 gap-y-4">
          <div className="min-w-0 flex-1 basis-72">
            <p>
              {t("Bu site şu sitenin hazırlık ortamı:")}{" "}
              <Link to={`/sites/${live.id}/wordpress`} className="font-medium text-accent hover:underline">
                {live.domain}
              </Link>
            </p>
            <p className="mt-1 text-[13.5px] text-ink-2">{t("Değişiklikleri burada deneyin; hazır olunca canlıya aktarın. Canlı site bu arada değiştiyse önce buradan yenileyebilirsiniz.")}</p>
          </div>
          <div className="flex flex-wrap gap-2 max-sm:w-full max-sm:[&>*]:flex-1">
            <Button icon={<ArrowDownToLine className="size-4" />} onClick={() => setSyncing("refresh")} disabled={site.status !== "active"}>
              {t("Canlıdan yenile")}
            </Button>
            <Button variant="primary" icon={<ArrowUpFromLine className="size-4" />} onClick={() => setSyncing("push")} disabled={site.status !== "active"}>
              {t("Canlıya aktar")}
            </Button>
          </div>
        </div>
        {syncing && <SyncModal copy={site} live={live} direction={syncing} onClose={() => setSyncing(null)} />}
      </Panel>
    );
  }

  return (
    <Panel title={t("Hazırlık ortamı")}>
      <div className="flex flex-wrap items-center gap-x-6 gap-y-4">
        <div className="min-w-0 flex-1 basis-72">
          {copies.length === 0 ? (
            <p className="text-ink-2">{t("Güncellemeleri ve tasarım değişikliklerini canlı siteye dokunmadan denemek için sitenin bağlı bir kopyasını açın.")}</p>
          ) : (
            <ul className="flex flex-col gap-2">
              {copies.map((copy) => (
                <li key={copy.id} className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <Link to={`/sites/${copy.id}/wordpress`} className="font-medium text-accent hover:underline">
                    <Mono>{copy.domain}</Mono>
                  </Link>
                  <Badge>{t("Hazırlık ortamı")}</Badge>
                </li>
              ))}
            </ul>
          )}
          {copies.length > 0 && <p className="mt-2 text-[13.5px] text-ink-2">{t("Canlıya aktarma ve yenileme, hazırlık ortamının kendi sayfasından yapılır.")}</p>}
        </div>
        {copies.length === 0 && (
          <Button onClick={() => setCreating(true)} disabled={site.status !== "active"}>
            {t("Hazırlık ortamı aç")}
          </Button>
        )}
      </div>
      {creating && <CreateModal site={site} suggested={suggested} onClose={() => setCreating(false)} />}
    </Panel>
  );
}
