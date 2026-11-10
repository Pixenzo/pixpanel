import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import type { Tone } from "../components/ui";
import { EmptyState, ErrorBlock, LoadingBlock, Modal, PageHeader, Panel, Status, Table, Td, Th } from "../components/ui";
import type { JobInfo } from "../lib/api";
import { errorMessage } from "../lib/api";
import { formatDateTime, formatDuration } from "../lib/format";
import { t } from "../lib/i18n";
import { useJobs } from "../lib/live";

const statusView: Record<JobInfo["status"], { tone: Tone; label: string }> = {
  running: { tone: "accent", label: "Çalışıyor" },
  success: { tone: "ok", label: "Tamamlandı" },
  failed: { tone: "bad", label: "Başarısız" },
};

/** Görev çıktısını canlı izler; görev bitince akış kendiliğinden kapanır. */
export function JobLog({ jobId, onDone }: { jobId: number; onDone?: (status: string) => void }) {
  const [lines, setLines] = useState<string[]>([]);
  const [status, setStatus] = useState<string | null>(null);
  const box = useRef<HTMLPreElement>(null);
  const pinned = useRef(true);
  const onDoneRef = useRef(onDone);
  onDoneRef.current = onDone;

  useEffect(() => {
    setLines([]);
    setStatus(null);
    const source = new EventSource(`/api/v1/jobs/${jobId}/stream`);
    source.onmessage = (message) => {
      const event = JSON.parse(message.data) as { type: "log"; line: string } | { type: "done"; status: string };
      if (event.type === "log") {
        setLines((current) => [...current, event.line]);
      } else {
        setStatus(event.status);
        onDoneRef.current?.(event.status);
        source.close();
      }
    };
    source.onerror = () => source.close();
    return () => source.close();
  }, [jobId]);

  // Kullanıcı yukarı kaydırmadıysa çıktı en alta sabitlenir.
  useEffect(() => {
    const element = box.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [lines]);

  return (
    <div className="flex flex-col gap-3">
      <pre
        ref={box}
        tabIndex={0}
        aria-label={t("Görev çıktısı")}
        onScroll={(event) => {
          const element = event.currentTarget;
          pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
        }}
        className="h-[46vh] overflow-auto rounded-md border border-line bg-sunken p-3 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap"
      >
        {lines.length === 0 ? t("Çıktı bekleniyor") : lines.join("\n")}
      </pre>
      {status ? (
        <Status tone={status === "success" ? "ok" : "bad"}>{status === "success" ? t("Görev tamamlandı") : t("Görev başarısız oldu")}</Status>
      ) : (
        <Status tone="accent">{t("Çalışıyor")}</Status>
      )}
    </div>
  );
}

export function JobsPage() {
  const queryClient = useQueryClient();
  const [openJob, setOpenJob] = useState<JobInfo | null>(null);
  const jobs = useJobs();

  return (
    <>
      <PageHeader
        title={t("Görevler")}
        description={t("Paket kurulumu ya da yedekleme gibi uzun süren işlemler burada izlenir.")}
      />
      {jobs.isError && <ErrorBlock message={errorMessage(jobs.error)} onRetry={() => void jobs.refetch()} />}
      {jobs.isPending && <LoadingBlock />}
      {jobs.data && (
        <Panel flush>
          {jobs.data.length === 0 ? (
            <EmptyState title={t("Henüz görev çalışmadı")} description={t("Uzun süren bir işlem başlattığınızda çıktısını buradan izleyebilirsiniz.")} />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Görev")}</Th>
                  <Th>{t("Durum")}</Th>
                  <Th>{t("Başlangıç")}</Th>
                  <Th align="right">{t("Süre")}</Th>
                </tr>
              </thead>
              <tbody>
                {jobs.data.map((job) => {
                  const view = statusView[job.status];
                  const elapsed = (job.finished_at ?? Math.floor(Date.now() / 1000)) - job.created_at;
                  return (
                    <tr key={job.id} className="hover:bg-sunken/50">
                      <Td>
                        <button type="button" className="font-medium text-accent hover:underline" onClick={() => setOpenJob(job)}>
                          {job.title}
                        </button>
                      </Td>
                      <Td>
                        <Status tone={view.tone}>{t(view.label)}</Status>
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2">{formatDateTime(job.created_at)}</Td>
                      <Td align="right" className="whitespace-nowrap text-ink-2">
                        {formatDuration(elapsed)}
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
      <Modal open={openJob !== null} onClose={() => setOpenJob(null)} title={openJob?.title ?? ""} wide>
        {openJob && <JobLog jobId={openJob.id} onDone={() => void queryClient.invalidateQueries({ queryKey: ["jobs"] })} />}
      </Modal>
    </>
  );
}
