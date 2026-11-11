import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Play, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, PageHeader, Panel, Select, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { JobLog } from "./Jobs";

interface CronJob {
  id: number;
  run_as: string;
  schedule: string;
  command: string;
  comment: string;
  enabled: boolean;
}

const presets: { schedule: string; label: string }[] = [
  { schedule: "* * * * *", label: "Her dakika" },
  { schedule: "*/5 * * * *", label: "5 dakikada bir" },
  { schedule: "*/15 * * * *", label: "15 dakikada bir" },
  { schedule: "0 * * * *", label: "Saat başı" },
  { schedule: "0 3 * * *", label: "Her gün 03:00" },
  { schedule: "0 3 * * 1", label: "Her pazartesi 03:00" },
  { schedule: "0 3 1 * *", label: "Her ayın ilk günü 03:00" },
  { schedule: "@reboot", label: "Sunucu açıldığında" },
];

/** Yaygın zamanlamaların okunur karşılığı; tanınmayanlar için boş döner. */
function describeSchedule(schedule: string): string {
  const preset = presets.find((item) => item.schedule === schedule);
  if (preset) return t(preset.label);
  const everyMinutes = schedule.match(/^\*\/(\d+) \* \* \* \*$/);
  if (everyMinutes) return t("{n} dakikada bir", { n: everyMinutes[1] });
  const daily = schedule.match(/^(\d{1,2}) (\d{1,2}) \* \* \*$/);
  if (daily) return t("Her gün {time}", { time: `${daily[2].padStart(2, "0")}:${daily[1].padStart(2, "0")}` });
  return "";
}

function JobModal({ job, accounts, onClose }: { job: CronJob | null; accounts: string[]; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [schedule, setSchedule] = useState(job?.schedule ?? "0 3 * * *");
  const [command, setCommand] = useState(job?.command ?? "");
  const [comment, setComment] = useState(job?.comment ?? "");
  const [runAs, setRunAs] = useState(job?.run_as ?? accounts[0] ?? "");
  const presetValue = presets.some((item) => item.schedule === schedule) ? schedule : "custom";

  const save = useMutation({
    mutationFn: () =>
      job
        ? api.patch(`/cron/${job.id}`, { schedule, command, comment })
        : api.post("/cron", { schedule, command, comment, run_as: accounts.length > 1 ? runAs : undefined }),
    onSuccess: () => {
      feedback.success(job ? t("Görev güncellendi") : t("Görev oluşturuldu"));
      void queryClient.invalidateQueries({ queryKey: ["cron"] });
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={job ? t("Görevi düzenle") : t("Zamanlanmış görev oluştur")}
      wide
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="cron-form" loading={save.isPending}>
            {job ? t("Değişiklikleri kaydet") : t("Görevi oluştur")}
          </Button>
        </>
      }
    >
      <form id="cron-form" onSubmit={submit} className="flex flex-col gap-5">
        <div className="grid gap-4 sm:grid-cols-2">
          <Select label={t("Ne zaman")} value={presetValue} onChange={(event) => event.target.value !== "custom" && setSchedule(event.target.value)}>
            {presets.map((item) => (
              <option key={item.schedule} value={item.schedule}>
                {t(item.label)}
              </option>
            ))}
            <option value="custom">{t("Özel")}</option>
          </Select>
          <Input
            label={t("Cron ifadesi")}
            value={schedule}
            onChange={(event) => setSchedule(event.target.value)}
            hint={t("dakika saat gün ay haftanın-günü")}
            className="font-mono text-[13.5px]"
            spellCheck={false}
            required
          />
        </div>
        <Input
          label={t("Komut")}
          value={command}
          onChange={(event) => setCommand(event.target.value)}
          hint={t("Tek satır. Örnek: php /home/hesap/sites/ornek.com/artisan schedule:run")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoFocus
          required
        />
        <Input label={t("Açıklama")} value={comment} onChange={(event) => setComment(event.target.value)} hint={t("İsteğe bağlı; görevin ne işe yaradığını hatırlatır.")} maxLength={120} />
        {!job && accounts.length > 1 && (
          <Select label={t("Hangi kullanıcıyla çalışsın")} value={runAs} onChange={(event) => setRunAs(event.target.value)} hint={t("root olarak çalışan görevler sunucunun tamamına erişebilir.")}>
            {accounts.map((account) => (
              <option key={account} value={account}>
                {account}
              </option>
            ))}
          </Select>
        )}
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

export function CronPage() {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<CronJob | "new" | null>(null);
  const [running, setRunning] = useState<{ jobId: number; title: string } | null>(null);
  const isAdmin = me.role === "admin";

  const jobs = useQuery({ queryKey: ["cron"], queryFn: () => api.get<CronJob[]>("/cron") });
  const roots = useQuery({
    queryKey: ["file-roots"],
    queryFn: () => api.get<{ id: string; label: string }[]>("/files/roots"),
    enabled: isAdmin,
  });
  const accounts = isAdmin ? (roots.data ?? []).map((root) => (root.id === "root" ? "root" : root.label)) : [];
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["cron"] });

  const toggle = useMutation({
    mutationFn: (job: CronJob) => api.patch(`/cron/${job.id}`, { enabled: !job.enabled }),
    onSuccess: (_, job) => feedback.success(job.enabled ? t("Görev durduruldu") : t("Görev yeniden etkin")),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });
  const remove = useMutation({
    mutationFn: (job: CronJob) => api.delete(`/cron/${job.id}`),
    onSuccess: () => feedback.success(t("Görev silindi")),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });
  const runNow = useMutation({
    mutationFn: (job: CronJob) => api.post<{ job_id: number }>(`/cron/${job.id}/run`),
    onSuccess: (result, job) => setRunning({ jobId: result.job_id, title: job.comment || job.command }),
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(job: CronJob) {
    const confirmed = await feedback.confirm({
      title: t("Görev silinsin mi?"),
      description: job.command,
      confirmLabel: t("Görevi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(job);
  }

  const createButton = (
    <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEditing("new")}>
      {t("Görev oluştur")}
    </Button>
  );

  return (
    <>
      <PageHeader title={t("Zamanlanmış görevler")} description={t("Belirli aralıklarla kendiliğinden çalışan komutlar.")} actions={createButton} />
      {jobs.isError && <ErrorBlock message={errorMessage(jobs.error)} onRetry={() => void jobs.refetch()} />}
      {jobs.isPending && <LoadingBlock />}
      {jobs.data && (
        <Panel flush>
          {jobs.data.length === 0 ? (
            <EmptyState
              title={t("Henüz zamanlanmış görev yok")}
              description={t("Yedek alma, önbellek temizleme ya da Laravel zamanlayıcısı gibi düzenli işleri burada tanımlayın.")}
              action={createButton}
            />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Etkin")}</Th>
                  <Th>{t("Zamanlama")}</Th>
                  <Th className="w-full">{t("Komut")}</Th>
                  {isAdmin && <Th>{t("Kullanıcı")}</Th>}
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {jobs.data.map((job) => (
                  <tr key={job.id} className="hover:bg-sunken/50">
                    <Td>
                      <input
                        type="checkbox"
                        aria-label={t("Görev etkin")}
                        className="size-4 accent-(--accent)"
                        checked={job.enabled}
                        disabled={toggle.isPending}
                        onChange={() => toggle.mutate(job)}
                      />
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Mono className="font-medium">{job.schedule}</Mono>
                      {describeSchedule(job.schedule) && <p className="text-[13px] text-ink-3">{describeSchedule(job.schedule)}</p>}
                    </Td>
                    <Td className="max-w-0">
                      <span className="block truncate font-mono text-[13px]" title={job.command}>
                        {job.command}
                      </span>
                      {job.comment && <p className="truncate text-[13px] text-ink-3">{job.comment}</p>}
                    </Td>
                    {isAdmin && (
                      <Td>
                        <Mono>{job.run_as}</Mono>
                      </Td>
                    )}
                    <Td align="right" className="py-0.5">
                      <div className="flex justify-end gap-0.5">
                        <IconButton label={t("Şimdi çalıştır")} disabled={runNow.isPending} onClick={() => runNow.mutate(job)}>
                          <Play className="size-4" />
                        </IconButton>
                        <IconButton label={t("Düzenle")} onClick={() => setEditing(job)}>
                          <Pencil className="size-4" />
                        </IconButton>
                        <IconButton label={t("Sil")} onClick={() => void confirmRemove(job)}>
                          <Trash2 className="size-4" />
                        </IconButton>
                      </div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
      {editing && <JobModal job={editing === "new" ? null : editing} accounts={accounts} onClose={() => setEditing(null)} />}
      <Modal open={running !== null} onClose={() => setRunning(null)} title={running?.title ?? ""} wide>
        {running && <JobLog jobId={running.jobId} />}
      </Modal>
    </>
  );
}
