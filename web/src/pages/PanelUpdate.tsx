// Panelin kendi sürüm güncellemesi: yeni sürümü denetler, indirir, panel yeniden başlayana kadar izler.
// Panel yeniden başlarken kısa süre yanıt vermez; bu yüzden izleme react-query dışında, hataları yutan bir yoklamayla yapılır.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CircleAlert, CircleCheck, CircleX } from "lucide-react";
import { useEffect, useState } from "react";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import { Button, ErrorBlock, LoadingBlock, Modal, Mono, Panel, Spinner, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

interface UpdateOutcome {
  from: string;
  to: string;
  status: "restarting" | "success" | "rolled_back";
  detail: string;
  /** Unix saniyesi. */
  at: number;
}

interface UpdateView {
  current: string;
  /** Hiç denetlenmediyse boş dize. */
  latest: string;
  available: boolean;
  checked_at: number | null;
  check_error: string;
  last: UpdateOutcome | null;
  /** Sürümlerin çekildiği adres. */
  source: string;
}

const queryKey = ["panel-update"];

/** Yeniden başlatma bekleme süresi (saniye); bu süre dolarsa kullanıcıya bilgi verilir. */
const RESTART_TIMEOUT_SECONDS = 150;
const POLL_INTERVAL_SECONDS = 2;

type UpdateStep = "download" | "failed" | "restart" | "result";
type ResultKind = "success" | "rolled_back" | "timeout";

/** Süren güncellemenin durumu; pencere kapanınca temizlenir. */
interface UpdateRun {
  jobId: number;
  /** Güncellenmek istenen sürüm. */
  target: string;
  step: UpdateStep;
  result: { kind: ResultKind; outcome: UpdateOutcome | null } | null;
}

/** Üst bölümdeki sürüm durumu: renk ve metin. */
function versionStatus(view: UpdateView): { tone: Tone; label: string } {
  if (view.check_error) return { tone: "warn", label: t("Denetlenemedi") };
  if (view.available) return { tone: "accent", label: t("Yeni sürüm var: {latest}", { latest: view.latest }) };
  if (view.checked_at === null) return { tone: "idle", label: t("Henüz denetlenmedi") };
  return { tone: "ok", label: t("Güncel") };
}

/** Önceki güncellemenin sonucu için küçük durum. */
function outcomeStatus(status: UpdateOutcome["status"]): { tone: Tone; label: string } {
  if (status === "success") return { tone: "ok", label: t("Başarılı") };
  if (status === "rolled_back") return { tone: "bad", label: t("Geri alındı") };
  return { tone: "accent", label: t("Sürüyor") };
}

function StepHeader({ state, children }: { state: "active" | "done" | "failed" | "warn"; children: string }) {
  return (
    <div className="flex items-center gap-2 font-medium">
      {state === "active" && <Spinner className="text-accent" />}
      {state === "done" && <CircleCheck aria-hidden className="size-4 text-ok" />}
      {state === "failed" && <CircleX aria-hidden className="size-4 text-bad" />}
      {state === "warn" && <CircleAlert aria-hidden className="size-4 text-warn" />}
      <span>{children}</span>
    </div>
  );
}

function UpdateModal({
  run,
  onChange,
  onClose,
}: {
  run: UpdateRun;
  onChange: (next: UpdateRun) => void;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const [elapsed, setElapsed] = useState(0);
  const { step, target } = run;

  // Panel yeniden başlarken: iki saniyede bir sürüm bilgisini yokla, bağlantı hatalarını yok say.
  useEffect(() => {
    if (step !== "restart") return;
    const startedAt = Date.now();
    let lastPollAt = 0;
    let busy = false;
    let finished = false;

    const finish = (result: UpdateRun["result"]) => {
      finished = true;
      clearInterval(timer);
      onChange({ ...run, step: "result", result });
    };

    const timer = setInterval(() => {
      const seconds = Math.floor((Date.now() - startedAt) / 1000);
      setElapsed(seconds);
      if (seconds >= RESTART_TIMEOUT_SECONDS) {
        finish({ kind: "timeout", outcome: null });
        return;
      }
      if (busy || seconds - lastPollAt < POLL_INTERVAL_SECONDS) return;
      lastPollAt = seconds;
      busy = true;
      api
        .get<UpdateView>("/system/update")
        .then((view) => {
          if (finished) return;
          const last = view.last;
          if (last && last.to === target && (last.status === "success" || last.status === "rolled_back")) {
            queryClient.setQueryData(queryKey, view);
            finish({ kind: last.status, outcome: last });
          }
        })
        .catch(() => {
          // Panel o sırada kapalıdır; hata yok sayılır ve yoklama sürer.
        })
        .finally(() => {
          busy = false;
        });
    }, 1000);

    return () => {
      finished = true;
      clearInterval(timer);
    };
    // `run` ve `onChange` bilerek bağımlılık dışında: yoklama yalnızca adım ya da hedef değişince yeniden kurulur.
  }, [step, target]);

  const closable = step === "failed" || step === "result";
  const result = run.result;

  let footer = null;
  if (step === "failed") {
    footer = <Button onClick={onClose}>{t("Kapat")}</Button>;
  } else if (step === "result" && result) {
    if (result.kind === "success") {
      footer = (
        <Button variant="primary" onClick={() => window.location.reload()}>
          {t("Sayfayı yenile")}
        </Button>
      );
    } else if (result.kind === "rolled_back") {
      footer = <Button onClick={onClose}>{t("Kapat")}</Button>;
    } else {
      footer = (
        <>
          <Button onClick={onClose}>{t("Kapat")}</Button>
          <Button variant="primary" onClick={() => window.location.reload()}>
            {t("Sayfayı yenile")}
          </Button>
        </>
      );
    }
  }

  const downloadState = step === "download" ? "active" : step === "failed" ? "failed" : "done";

  return (
    <Modal
      open
      onClose={() => closable && onClose()}
      title={t("Panel {version} sürümüne güncelleniyor", { version: target })}
      wide
      footer={footer}
    >
      <div className="flex flex-col gap-5">
        <section className="flex flex-col gap-2">
          <StepHeader state={downloadState}>{t("İndiriliyor ve doğrulanıyor")}</StepHeader>
          <JobLog jobId={run.jobId} onDone={(status) => onChange({ ...run, step: status === "success" ? "restart" : "failed" })} />
          {step === "failed" && (
            <p className="text-bad">{t("Güncelleme başarısız oldu. Nedeni yukarıdaki günlükte yazıyor; panel eski sürümle çalışmayı sürdürüyor.")}</p>
          )}
        </section>

        {(step === "restart" || step === "result") && (
          <section className="flex flex-col gap-2">
            <StepHeader state={step === "restart" ? "active" : "done"}>{t("Panel yeniden başlatılıyor")}</StepHeader>
            {step === "restart" && (
              <p className="text-ink-2">
                {t("Panel kısa süre yanıt vermeyecek, bu sayfayı kapatmayın. Geçen süre: {seconds} sn", { seconds: elapsed })}
              </p>
            )}
          </section>
        )}

        {step === "result" && result && (
          <section className="flex flex-col gap-2">
            {result.kind === "success" && (
              <>
                <StepHeader state="done">{t("Panel {version} sürümüne güncellendi.", { version: result.outcome?.to ?? target })}</StepHeader>
                <p className="text-ink-2">{t("Arayüz de değiştiği için sayfayı yenilemeniz gerekir.")}</p>
              </>
            )}
            {result.kind === "rolled_back" && (
              <>
                <StepHeader state="failed">
                  {t("Yeni sürüm başlamadı; panel eski sürümle ({version}) çalışmaya devam ediyor.", {
                    version: result.outcome?.from ?? "",
                  })}
                </StepHeader>
                {result.outcome?.detail && <p className="text-ink-2">{result.outcome.detail}</p>}
              </>
            )}
            {result.kind === "timeout" && (
              <StepHeader state="warn">
                {t("Panel beklenen sürede yanıt vermedi. Birkaç dakika sonra sayfayı yenileyip sürümü denetleyin.")}
              </StepHeader>
            )}
          </section>
        )}
      </div>
    </Modal>
  );
}

function UpdateBody({ view }: { view: UpdateView }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [run, setRun] = useState<UpdateRun | null>(null);

  const check = useMutation({
    mutationFn: () => api.post<UpdateView>("/system/update/check"),
    onSuccess: (next) => queryClient.setQueryData(queryKey, next),
    onError: (error) => feedback.error(error),
  });

  const apply = useMutation({
    mutationFn: () => api.post<{ job_id: number }>("/system/update/apply"),
    onSuccess: (result) => setRun({ jobId: result.job_id, target: view.latest, step: "download", result: null }),
    onError: (error) => feedback.error(error),
  });

  async function confirmApply() {
    const confirmed = await feedback.confirm({
      title: t("Panel {latest} sürümüne güncellensin mi?", { latest: view.latest }),
      description: t(
        "Panel yaklaşık bir dakika erişilemez olacak; siteler, posta ve veritabanları bundan etkilenmez. Yeni sürüm başlamazsa eski sürüme kendiliğinden dönülür.",
      ),
      confirmLabel: t("{latest} sürümüne güncelle", { latest: view.latest }),
    });
    if (confirmed) apply.mutate();
  }

  function closeModal() {
    setRun(null);
    void queryClient.invalidateQueries({ queryKey });
  }

  const status = versionStatus(view);
  const last = view.last;
  const lastStatus = last ? outcomeStatus(last.status) : null;

  return (
    <div className="flex flex-col gap-4">
      <div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <span className="text-ink-2">{t("Kurulu sürüm")}</span>
          <Mono>{view.current}</Mono>
          <Status tone={status.tone}>{status.label}</Status>
        </div>
        {view.check_error && <p className="mt-1 text-ink-2">{view.check_error}</p>}
        {view.checked_at !== null && (
          <p className="mt-1 text-[12.5px] text-ink-3">{t("Son denetim: {relative}", { relative: formatRelative(view.checked_at) })}</p>
        )}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button loading={check.isPending} onClick={() => check.mutate()}>
          {t("Güncellemeleri denetle")}
        </Button>
        {view.available && (
          <Button variant="primary" loading={apply.isPending} onClick={() => void confirmApply()}>
            {t("{latest} sürümüne güncelle", { latest: view.latest })}
          </Button>
        )}
      </div>

      {last && lastStatus && (
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[12.5px] text-ink-3">
          <span>{t("Son güncelleme: {from} → {to}, {relative}", { from: last.from, to: last.to, relative: formatRelative(last.at) })}</span>
          <Status tone={lastStatus.tone}>{lastStatus.label}</Status>
        </p>
      )}

      <div className="flex flex-col gap-1 text-[12.5px] text-ink-3">
        <p>{t("Sürümler imzalıdır; imzası doğrulanamayan bir dosya kurulmaz.")}</p>
        {view.source && (
          <p className="flex min-w-0 items-center gap-1.5">
            <span className="shrink-0">{t("Kaynak:")}</span>
            <Mono className="truncate">
              <span title={view.source}>{view.source}</span>
            </Mono>
          </p>
        )}
      </div>

      {run && <UpdateModal run={run} onChange={setRun} onClose={closeModal} />}
    </div>
  );
}

export function PanelUpdate() {
  const state = useQuery({ queryKey, queryFn: () => api.get<UpdateView>("/system/update") });

  return (
    <Panel title={t("Panel sürümü")} className="mb-4">
      {state.isPending && <LoadingBlock />}
      {state.isError && <ErrorBlock message={errorMessage(state.error)} onRetry={() => void state.refetch()} />}
      {state.data && <UpdateBody view={state.data} />}
    </Panel>
  );
}
