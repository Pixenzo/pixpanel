// "Planlar" sekmesi: neyin ne zaman kendiliğinden yedekleneceği.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { Pencil, Play, Plus, Trash2 } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, EmptyState, IconButton, Input, Modal, Panel, Select, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { ScopePicker } from "./BackupScope";
import type { BackupOverview, Frequency, JobRef, JobStarted, Plan, Scope, Target } from "./BackupShared";
import {
  describeScope,
  formatUpcoming,
  fullScope,
  isScopeEmpty,
  JobModal,
  minutesToClock,
  Notice,
  OVERVIEW_KEY,
  Segmented,
  SNAPSHOTS_KEY,
  Switch,
  useOverviewSetter,
  WEEKDAYS,
} from "./BackupShared";

// ---- Türler ve sabitler ----

interface PlanBody {
  name: string;
  target_id: number;
  scope: Scope;
  frequency: Frequency;
  time: string;
  weekday: number;
  month_day: number;
  keep_daily: number;
  keep_weekly: number;
  keep_monthly: number;
  enabled: boolean;
}

interface Keep {
  keep_daily: number;
  keep_weekly: number;
  keep_monthly: number;
}

type RetentionPreset = "short" | "balanced" | "long" | "custom";

const PRESETS: { id: Exclude<RetentionPreset, "custom">; title: string; description: string; keep: Keep }[] = [
  { id: "short", title: "Kısa", description: "7 günlük kopya", keep: { keep_daily: 7, keep_weekly: 0, keep_monthly: 0 } },
  { id: "balanced", title: "Dengeli", description: "7 günlük, 4 haftalık, 6 aylık kopya", keep: { keep_daily: 7, keep_weekly: 4, keep_monthly: 6 } },
  { id: "long", title: "Uzun", description: "14 günlük, 8 haftalık, 12 aylık kopya", keep: { keep_daily: 14, keep_weekly: 8, keep_monthly: 12 } },
];

const MONTH_DAYS = Array.from({ length: 28 }, (_, index) => index + 1);

// ---- Yardımcılar ----

/** Var olan bir planı sunucunun beklediği gövdeye çevirir; `changes` üstüne yazılır. */
function planToBody(plan: Plan, changes: Partial<PlanBody> = {}): PlanBody {
  return {
    name: plan.name,
    target_id: plan.target_id,
    scope: plan.scope,
    frequency: plan.frequency,
    time: minutesToClock(plan.at_minute),
    weekday: plan.weekday,
    month_day: plan.month_day,
    keep_daily: plan.keep_daily,
    keep_weekly: plan.keep_weekly,
    keep_monthly: plan.keep_monthly,
    enabled: plan.enabled,
    ...changes,
  };
}

function detectPreset(keep: Keep): RetentionPreset {
  const match = PRESETS.find(
    (preset) => preset.keep.keep_daily === keep.keep_daily && preset.keep.keep_weekly === keep.keep_weekly && preset.keep.keep_monthly === keep.keep_monthly,
  );
  return match ? match.id : "custom";
}

function describeFrequency(plan: Plan): string {
  const clock = minutesToClock(plan.at_minute);
  switch (plan.frequency) {
    case "daily":
      return t("Her gün {time}", { time: clock });
    case "weekly":
      return t("Her hafta {day} {time}", { day: t(WEEKDAYS[plan.weekday - 1] ?? WEEKDAYS[0]), time: clock });
    case "monthly":
      return t("Her ayın {n}. günü {time}", { n: plan.month_day, time: clock });
    case "manual":
      return t("Yalnızca elle");
  }
}

function describeKeep(keep: Keep): string {
  const parts: string[] = [];
  if (keep.keep_daily > 0) parts.push(t("{n} günlük", { n: keep.keep_daily }));
  if (keep.keep_weekly > 0) parts.push(t("{n} haftalık", { n: keep.keep_weekly }));
  if (keep.keep_monthly > 0) parts.push(t("{n} aylık", { n: keep.keep_monthly }));
  if (parts.length === 0) return t("Eski kopyalar silinmez");
  return t("{list} kopya saklanır", { list: parts.join(", ") });
}

function parseCount(text: string): number | null {
  return /^\d{1,3}$/.test(text.trim()) ? Number(text.trim()) : null;
}

/** Plan işlemlerinden sonra yedek listesinin de tazelenmesi için ortak yardımcı. */
function useRefreshAfterJob() {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: OVERVIEW_KEY });
    void queryClient.invalidateQueries({ queryKey: SNAPSHOTS_KEY });
  };
}

// ---- Plan penceresi ----

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="font-semibold">{title}</h3>
      {children}
    </section>
  );
}

function PlanModal({ plan, targets, sources, onClose }: { plan: Plan | null; targets: Target[]; sources: BackupOverview["sources"]; onClose: () => void }) {
  const feedback = useFeedback();
  const setOverview = useOverviewSetter();

  const [name, setName] = useState(plan?.name ?? "");
  const [targetId, setTargetId] = useState<number>(plan?.target_id ?? targets[0]?.id ?? 0);
  const [scope, setScope] = useState<Scope>(plan?.scope ?? fullScope());
  const [frequency, setFrequency] = useState<Frequency>(plan?.frequency ?? "daily");
  const [time, setTime] = useState(plan ? minutesToClock(plan.at_minute) : "03:00");
  const [weekday, setWeekday] = useState(plan?.weekday ?? 1);
  const [monthDay, setMonthDay] = useState(plan?.month_day ?? 1);
  const [preset, setPreset] = useState<RetentionPreset>(plan ? detectPreset(plan) : "balanced");
  const [custom, setCustom] = useState({
    daily: String(plan?.keep_daily ?? 7),
    weekly: String(plan?.keep_weekly ?? 4),
    monthly: String(plan?.keep_monthly ?? 6),
  });

  const customKeep: Keep | null = (() => {
    const daily = parseCount(custom.daily);
    const weekly = parseCount(custom.weekly);
    const monthly = parseCount(custom.monthly);
    return daily === null || weekly === null || monthly === null ? null : { keep_daily: daily, keep_weekly: weekly, keep_monthly: monthly };
  })();
  const keep: Keep | null = preset === "custom" ? customKeep : (PRESETS.find((item) => item.id === preset)?.keep ?? null);

  const save = useMutation({
    mutationFn: (body: PlanBody) =>
      plan ? api.put<BackupOverview>(`/backup/plans/${plan.id}`, body) : api.post<BackupOverview>("/backup/plans", body),
    onSuccess: (next) => {
      setOverview(next);
      feedback.success(plan ? t("Plan kaydedildi") : t("Plan eklendi"));
      onClose();
    },
  });

  const scheduled = frequency !== "manual";
  const problem = (() => {
    if (name.trim() === "") return t("Plana bir ad verin.");
    if (!targets.some((target) => target.id === targetId)) return t("Yedeklerin duracağı yeri seçin.");
    if (isScopeEmpty(scope)) return t("Yedeklenecek en az bir şey seçin.");
    if (scheduled && time === "") return t("Bir saat seçin.");
    if (!keep) return t("Saklanacak kopya sayıları 0 ile 999 arasında bir sayı olmalıdır.");
    return null;
  })();

  function submit() {
    if (problem || !keep) return;
    save.mutate({
      name: name.trim(),
      target_id: targetId,
      scope,
      frequency,
      time: time === "" ? "03:00" : time,
      weekday,
      month_day: monthDay,
      ...keep,
      enabled: plan?.enabled ?? true,
    });
  }

  const frequencies: { id: Frequency; label: string }[] = [
    { id: "daily", label: t("Her gün") },
    { id: "weekly", label: t("Her hafta") },
    { id: "monthly", label: t("Her ay") },
    { id: "manual", label: t("Yalnızca elle") },
  ];

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={plan ? t("Planı düzenle") : t("Plan ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={save.isPending} disabled={problem !== null} onClick={submit}>
            {plan ? t("Kaydet") : t("Planı ekle")}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-6"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <Input label={t("Ad")} placeholder={t("Örn. Gece yedeği")} value={name} onChange={(event) => setName(event.target.value)} maxLength={80} autoFocus={!plan} />
          <Select label={t("Nereye?")} value={targetId} onChange={(event) => setTargetId(Number(event.target.value))}>
            {targets.map((target) => (
              <option key={target.id} value={target.id}>
                {target.name}
              </option>
            ))}
          </Select>
        </div>

        <Section title={t("Neler yedeklensin?")}>
          <ScopePicker value={scope} onChange={setScope} sources={sources} />
        </Section>

        <Section title={t("Ne sıklıkla?")}>
          <Segmented label={t("Sıklık")} options={frequencies} value={frequency} onChange={setFrequency} />
          {scheduled && (
            <div className="grid gap-3 sm:grid-cols-3">
              <Input label={t("Saat")} type="time" value={time} onChange={(event) => setTime(event.target.value)} />
              {frequency === "weekly" && (
                <Select label={t("Hangi gün?")} value={weekday} onChange={(event) => setWeekday(Number(event.target.value))}>
                  {WEEKDAYS.map((day, index) => (
                    <option key={day} value={index + 1}>
                      {t(day)}
                    </option>
                  ))}
                </Select>
              )}
              {frequency === "monthly" && (
                <Select label={t("Ayın kaçıncı günü?")} value={monthDay} onChange={(event) => setMonthDay(Number(event.target.value))}>
                  {MONTH_DAYS.map((day) => (
                    <option key={day} value={day}>
                      {day}
                    </option>
                  ))}
                </Select>
              )}
            </div>
          )}
          <p className="text-[12.5px] text-ink-3">
            {scheduled ? t("Saat, sunucunun yerel saatidir.") : t("Bu plan yalnızca siz “Şimdi çalıştır” dediğinizde çalışır.")}
          </p>
        </Section>

        <Section title={t("Kaç kopya saklansın?")}>
          <div role="radiogroup" aria-label={t("Saklama süresi")} className="grid gap-2 sm:grid-cols-2">
            {[...PRESETS, { id: "custom" as const, title: "Özel", description: "Sayıları kendiniz belirleyin", keep: null }].map((item) => (
              <label
                key={item.id}
                className={classNames(
                  "flex cursor-pointer flex-col gap-0.5 rounded-lg border p-3 transition-colors",
                  preset === item.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                )}
              >
                <span className="flex items-center gap-2 font-medium">
                  <input type="radio" name="retention" className="accent-(--accent)" checked={preset === item.id} onChange={() => setPreset(item.id)} />
                  {t(item.title)}
                  {item.id === "balanced" && <span className="text-[12px] font-medium text-accent">{t("önerilen")}</span>}
                </span>
                <span className="text-[12.5px] text-ink-2">{t(item.description)}</span>
              </label>
            ))}
          </div>
          {preset === "custom" && (
            <div className="grid gap-3 sm:grid-cols-3">
              <Input label={t("Günlük")} type="number" inputMode="numeric" min={0} max={999} value={custom.daily} onChange={(event) => setCustom({ ...custom, daily: event.target.value })} />
              <Input label={t("Haftalık")} type="number" inputMode="numeric" min={0} max={999} value={custom.weekly} onChange={(event) => setCustom({ ...custom, weekly: event.target.value })} />
              <Input
                label={t("Aylık")}
                type="number"
                inputMode="numeric"
                min={0}
                max={999}
                hint={t("0 = o türden kopya saklama")}
                value={custom.monthly}
                onChange={(event) => setCustom({ ...custom, monthly: event.target.value })}
              />
            </div>
          )}
          <p className="text-[12.5px] text-ink-3">{t("Eski kopyalar kendiliğinden silinir; elle alınan yedekler silinmez.")}</p>
          {keep && keep.keep_daily === 0 && keep.keep_weekly === 0 && keep.keep_monthly === 0 && (
            <Notice tone="warn">{t("Hiçbir yedek silinmez; yedeklerin kapladığı yer sürekli büyür.")}</Notice>
          )}
        </Section>

        {save.isError && <Notice tone="bad">{errorMessage(save.error)}</Notice>}
        {problem && !save.isError && name !== "" && <p className="text-[12.5px] text-ink-3">{problem}</p>}
        {/* Enter ile gönderimi etkinleştiren gizli düğme. */}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}

// ---- Plan satırı ----

function PlanStatus({ plan, onShowLog }: { plan: Plan; onShowLog: () => void }) {
  switch (plan.last_status) {
    case "success":
      return <Status tone="ok">{t("Başarılı")}</Status>;
    case "failed":
      return (
        <span className="inline-flex flex-wrap items-center gap-x-2">
          <Status tone="bad">{t("Başarısız")}</Status>
          {plan.last_job_id !== null && (
            <button type="button" onClick={onShowLog} className="text-[13px] text-accent underline-offset-2 hover:underline">
              {t("günlüğü gör")}
            </button>
          )}
        </span>
      );
    case "running":
      return <Status tone="accent">{t("Sürüyor")}</Status>;
    default:
      return <Status tone="idle">{t("Henüz çalışmadı")}</Status>;
  }
}

function PlanRow({
  plan,
  targetName,
  busy,
  onToggle,
  onRun,
  onEdit,
  onDelete,
  onShowLog,
}: {
  plan: Plan;
  targetName: string;
  busy: boolean;
  onToggle: (enabled: boolean) => void;
  onRun: () => void;
  onEdit: () => void;
  onDelete: () => void;
  onShowLog: () => void;
}) {
  const details = [describeKeep(plan)];
  if (!plan.enabled) details.push(t("Plan kapalı"));
  else if (plan.frequency !== "manual" && plan.next_run_at) details.push(t("Sıradaki: {when}", { when: formatUpcoming(plan.next_run_at) }));
  if (plan.last_run_at) details.push(t("Son: {when}", { when: formatRelative(plan.last_run_at) }));

  return (
    <li className="flex flex-wrap items-start gap-x-4 gap-y-3 border-b border-line px-4 py-3 last:border-b-0">
      <div className="pt-0.5">
        <Switch checked={plan.enabled} label={t("{name} planı açık", { name: plan.name })} disabled={busy} onChange={onToggle} />
      </div>
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5">
          <span className="font-semibold">{plan.name}</span>
          <PlanStatus plan={plan} onShowLog={onShowLog} />
        </div>
        <p className={classNames("mt-0.5", !plan.enabled && "text-ink-3")}>
          {describeFrequency(plan)} · {describeScope(plan.scope)} → {targetName}
        </p>
        <p className="text-[13px] text-ink-2">{details.join(" · ")}</p>
      </div>
      <div className="flex items-center gap-1">
        <Button size="sm" icon={<Play className="size-3.5" />} disabled={busy || plan.last_status === "running"} onClick={onRun}>
          {t("Şimdi çalıştır")}
        </Button>
        <IconButton label={t("Planı düzenle")} disabled={busy} onClick={onEdit}>
          <Pencil className="size-4" />
        </IconButton>
        <IconButton label={t("Planı sil")} disabled={busy} onClick={onDelete}>
          <Trash2 className="size-4" />
        </IconButton>
      </div>
    </li>
  );
}

// ---- Sekme ----

export function PlansTab({ overview }: { overview: BackupOverview }) {
  const feedback = useFeedback();
  const setOverview = useOverviewSetter();
  const refresh = useRefreshAfterJob();

  const [editing, setEditing] = useState<Plan | "new" | null>(null);
  const [job, setJob] = useState<JobRef | null>(null);

  const toggle = useMutation({
    mutationFn: ({ plan, enabled }: { plan: Plan; enabled: boolean }) => api.put<BackupOverview>(`/backup/plans/${plan.id}`, planToBody(plan, { enabled })),
    onSuccess: setOverview,
    onError: (error) => feedback.error(error),
  });

  const run = useMutation({
    mutationFn: (plan: Plan) => api.post<JobStarted>(`/backup/plans/${plan.id}/run`),
    onSuccess: (started, plan) => setJob({ id: started.job_id, title: t("{name} planı çalışıyor", { name: plan.name }) }),
    onError: (error) => feedback.error(error),
  });

  const remove = useMutation({
    mutationFn: (plan: Plan) => api.delete<BackupOverview>(`/backup/plans/${plan.id}`),
    onSuccess: (next) => {
      setOverview(next);
      feedback.success(t("Plan silindi"));
    },
    onError: (error) => feedback.error(error),
  });

  async function askDelete(plan: Plan) {
    const confirmed = await feedback.confirm({
      title: t("Plan silinsin mi?"),
      description: t("“{name}” planı silinecek. Bu planla daha önce alınmış yedekler silinmez.", { name: plan.name }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(plan);
  }

  const targetNames = new Map(overview.targets.map((target) => [target.id, target.name]));
  const busyId = toggle.isPending ? toggle.variables.plan.id : run.isPending ? run.variables.id : remove.isPending ? remove.variables.id : null;

  return (
    <>
      {overview.plans.length === 0 ? (
        <Panel flush>
          <EmptyState
            title={t("Henüz plan yok")}
            description={t("Plan, neyin ne zaman yedekleneceğini bir kez belirlemenizi sağlar; sonrasında panel yedekleri kendiliğinden alır ve eskilerini temizler.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEditing("new")}>
                {t("Plan ekle")}
              </Button>
            }
          />
        </Panel>
      ) : (
        <div className="flex flex-col gap-3">
          <div className="flex justify-end">
            <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEditing("new")}>
              {t("Plan ekle")}
            </Button>
          </div>
          <Panel flush>
            <ul>
              {overview.plans.map((plan) => (
                <PlanRow
                  key={plan.id}
                  plan={plan}
                  targetName={targetNames.get(plan.target_id) ?? t("silinmiş yer")}
                  busy={busyId === plan.id}
                  onToggle={(enabled) => toggle.mutate({ plan, enabled })}
                  onRun={() => run.mutate(plan)}
                  onEdit={() => setEditing(plan)}
                  onDelete={() => void askDelete(plan)}
                  onShowLog={() => plan.last_job_id !== null && setJob({ id: plan.last_job_id, title: t("{name} planının günlüğü", { name: plan.name }) })}
                />
              ))}
            </ul>
          </Panel>
        </div>
      )}

      {editing !== null && (
        <PlanModal plan={editing === "new" ? null : editing} targets={overview.targets} sources={overview.sources} onClose={() => setEditing(null)} />
      )}
      <JobModal
        job={job}
        onClose={() => {
          setJob(null);
          refresh();
        }}
        onDone={refresh}
      />
    </>
  );
}
