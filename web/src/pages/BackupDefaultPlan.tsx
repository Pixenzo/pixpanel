// Planlar sekmesindeki "Yeni siteler için varsayılan plan" kutusu: açıksa yeni oluşturulan her site
// kendiliğinden bu plana eklenir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, Input, LoadingBlock, Panel, Select, Switch } from "../components/ui";
import type { BackupDefaultPlan } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import type { Target } from "./BackupShared";
import { minutesToClock, OVERVIEW_KEY, WEEKDAYS } from "./BackupShared";

const KEY = ["backup", "default-plan"];

const RETENTION: { id: string; label: string; keep: [number, number, number] }[] = [
  { id: "short", label: "Kısa: 7 günlük kopya", keep: [7, 0, 0] },
  { id: "balanced", label: "Dengeli: 7 günlük, 4 haftalık, 6 aylık kopya", keep: [7, 4, 6] },
  { id: "long", label: "Uzun: 14 günlük, 8 haftalık, 12 aylık kopya", keep: [14, 8, 12] },
];

function retentionOf(plan: BackupDefaultPlan): string {
  const match = RETENTION.find((item) => item.keep[0] === plan.keep_daily && item.keep[1] === plan.keep_weekly && item.keep[2] === plan.keep_monthly);
  return match?.id ?? "balanced";
}

function minutesOf(value: string): number {
  const [hours, minutes] = value.split(":").map(Number);
  return (hours || 0) * 60 + (minutes || 0);
}

export function DefaultPlanPanel({ targets }: { targets: Target[] }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const plan = useQuery({ queryKey: KEY, queryFn: () => api.get<BackupDefaultPlan>("/backup/default-plan") });
  const [draft, setDraft] = useState<BackupDefaultPlan | null>(null);

  useEffect(() => {
    if (plan.data) setDraft(plan.data);
  }, [plan.data]);

  const save = useMutation({
    mutationFn: (body: BackupDefaultPlan) => api.put<BackupDefaultPlan>("/backup/default-plan", body),
    onSuccess: (next) => {
      queryClient.setQueryData(KEY, next);
      void queryClient.invalidateQueries({ queryKey: OVERVIEW_KEY });
      feedback.success(t("Varsayılan yedek planı kaydedildi"));
    },
    onError: (error) => feedback.error(error),
  });

  if (plan.isPending || !draft) return <LoadingBlock />;
  if (plan.isError) return <ErrorBlock message={errorMessage(plan.error)} onRetry={() => void plan.refetch()} />;
  const saved = plan.data;
  const dirty = JSON.stringify(draft) !== JSON.stringify(saved);
  const targetId = draft.target_id ?? targets[0]?.id ?? null;

  function update(patch: Partial<BackupDefaultPlan>) {
    setDraft((current) => (current ? { ...current, ...patch } : current));
  }

  return (
    <Panel title={t("Yeni siteler için varsayılan plan")}>
      <div className="flex flex-col gap-4">
        <div className="flex items-start gap-3">
          <p className="min-w-0 flex-1 text-ink-2">
            {t("Açıkken yeni oluşturulan her site kendiliğinden bu plana eklenir; siteler yedeksiz kalmaz. Site zaten başka bir zamanlanmış plana giriyorsa dokunulmaz.")}
          </p>
          <Switch
            label={t("Yeni siteleri kendiliğinden plana ekle")}
            checked={draft.enabled}
            disabled={targets.length === 0}
            onChange={(next) => update({ enabled: next, target_id: targetId })}
          />
        </div>
        {targets.length === 0 ? (
          <p className="text-[13.5px] text-warn">{t("Önce Yedek yerleri sekmesinden bir yedek yeri ekleyin.")}</p>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            <Select label={t("Yedek yeri")} value={targetId ?? ""} onChange={(event) => update({ target_id: Number(event.target.value) })}>
              {targets.map((target) => (
                <option key={target.id} value={target.id}>
                  {target.name}
                </option>
              ))}
            </Select>
            <Select label={t("Sıklık")} value={draft.frequency} onChange={(event) => update({ frequency: event.target.value as BackupDefaultPlan["frequency"] })}>
              <option value="daily">{t("Her gün")}</option>
              <option value="weekly">{t("Haftada bir")}</option>
              <option value="monthly">{t("Ayda bir")}</option>
            </Select>
            <Input label={t("Saat")} type="time" value={minutesToClock(draft.at_minute)} onChange={(event) => event.target.value && update({ at_minute: minutesOf(event.target.value) })} />
            {draft.frequency === "weekly" && (
              <Select label={t("Gün")} value={draft.weekday} onChange={(event) => update({ weekday: Number(event.target.value) })}>
                {WEEKDAYS.map((name, index) => (
                  <option key={name} value={index + 1}>
                    {t(name)}
                  </option>
                ))}
              </Select>
            )}
            {draft.frequency === "monthly" && (
              <Select label={t("Ayın günü")} value={draft.month_day} onChange={(event) => update({ month_day: Number(event.target.value) })}>
                {Array.from({ length: 28 }, (_, index) => index + 1).map((day) => (
                  <option key={day} value={day}>
                    {day}
                  </option>
                ))}
              </Select>
            )}
            <Select
              label={t("Saklama")}
              value={retentionOf(draft)}
              onChange={(event) => {
                const keep = RETENTION.find((item) => item.id === event.target.value)?.keep ?? [7, 4, 6];
                update({ keep_daily: keep[0], keep_weekly: keep[1], keep_monthly: keep[2] });
              }}
            >
              {RETENTION.map((item) => (
                <option key={item.id} value={item.id}>
                  {t(item.label)}
                </option>
              ))}
            </Select>
          </div>
        )}
        <div>
          <Button variant="primary" disabled={!dirty || targets.length === 0} loading={save.isPending} onClick={() => save.mutate({ ...draft, target_id: targetId })}>
            {t("Kaydet")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}
