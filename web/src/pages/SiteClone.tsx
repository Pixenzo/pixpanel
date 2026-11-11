// Siteyi yeni bir alan adına kopyalama: pencere alan adını sorar, ardından kopyalama
// görevinin çıktısını canlı gösterir ve bitince kopyaya götürür.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { Button, ErrorBlock, Input, Modal } from "../components/ui";
import type { Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

function CloneModal({ site, onClose }: { site: Site; onClose: () => void }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [domain, setDomain] = useState("");
  const [started, setStarted] = useState<{ job_id: number; site_id: number } | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const start = useMutation({
    mutationFn: () => api.post<{ job_id: number; site_id: number }>(`/sites/${site.id}/clone`, { domain: domain.trim() }),
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
    // Başarısız kopya geri alınır; liste her iki durumda da tazelenir.
    void queryClient.invalidateQueries({ queryKey: ["sites"] });
  }

  if (started) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={t("{domain} kopyalanıyor", { domain: site.domain })}
        footer={
          <>
            <Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>
            {result === "success" && (
              <Button variant="primary" onClick={() => void navigate(`/sites/${started.site_id}`)}>
                {t("Kopyaya git")}
              </Button>
            )}
          </>
        }
      >
        <JobLog jobId={started.job_id} onDone={finished} />
        {result !== null && result !== "success" && <p className="mt-3 text-[13.5px] text-ink-2">{t("Kopyalama tamamlanamadı; yarım kalan site kaldırıldı. Asıl siteniz etkilenmedi.")}</p>}
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{domain} sitesini kopyala", { domain: site.domain })}
      description={t("Sitenin dosyaları ve ayarları yeni bir alan adına çoğaltılır. Kopya asıl siteden bağımsızdır; üzerinde yaptığınız değişiklikler asıl siteyi etkilemez.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="site-clone-form" loading={start.isPending}>
            {t("Kopyalamayı başlat")}
          </Button>
        </>
      }
    >
      <form id="site-clone-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input
          label={t("Kopyanın alan adı")}
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          placeholder={`deneme.${site.domain.replace(/^www\./, "")}`}
          hint={t("Denemeler için çoğunlukla bir alt alan adı kullanılır.")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
          autoFocus
          required
        />
        <ul className="list-disc space-y-1 pl-5 text-[13.5px] text-ink-2">
          {site.kind === "php" && <li>{t("WordPress siteleri veritabanıyla birlikte kopyalanır ve adresleri yeni alan adına çevrilir.")}</li>}
          {site.kind === "php" && <li>{t("Başka PHP uygulamalarında kopya aynı veritabanını kullanmaya devam eder; bağımsız çalışması için yeni bir veritabanı açıp uygulamanın ayarını güncelleyin.")}</li>}
          <li>{t("Takma adlar, SSL sertifikası, Git yayınlama ayarı ve FTP hesapları kopyalanmaz.")}</li>
        </ul>
        {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      </form>
    </Modal>
  );
}

/** "Yayın durumu" panelindeki satır: açıklama ve kopyalama penceresini açan düğme. */
export function CloneRow({ site }: { site: Site }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
      <div className="max-w-[60ch]">
        <p className="font-medium">{t("Siteyi kopyala")}</p>
        <p className="text-[13.5px] text-ink-2">{t("Büyük bir değişiklikten önce denemek ya da siteyi yeni bir alan adına çoğaltmak için.")}</p>
      </div>
      <Button onClick={() => setOpen(true)} disabled={site.status !== "active"}>
        {t("Siteyi kopyala")}
      </Button>
      {open && <CloneModal site={site} onClose={() => setOpen(false)} />}
    </div>
  );
}
