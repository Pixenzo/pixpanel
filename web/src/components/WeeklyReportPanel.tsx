// Genel bakış'taki "Haftalık özet" kutusu: aç/kapa, gün ve saat, ek alıcı, önizleme ve şimdi gönder.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, Send } from "lucide-react";
import { useEffect, useState } from "react";
import type { WeeklyReportDelivery, WeeklyReportSettings, WeeklyReportView } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Button, ErrorBlock, Input, LoadingBlock, Modal, Panel, Select, Switch } from "./ui";

const WEEKDAYS = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];
const KEY = ["weekly-report"];

function clock(minutes: number): string {
  return `${String(Math.floor(minutes / 60)).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
}

function minutesOf(value: string): number {
  const [hours, minutes] = value.split(":").map(Number);
  return (hours || 0) * 60 + (minutes || 0);
}

export function WeeklyReportPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const view = useQuery({ queryKey: KEY, queryFn: () => api.get<WeeklyReportView>("/health/weekly-report") });
  const [draft, setDraft] = useState<WeeklyReportSettings | null>(null);
  const [preview, setPreview] = useState<WeeklyReportDelivery | null>(null);

  // Sunucudaki ayar değişince (kaydedince) taslak ona eşitlenir.
  useEffect(() => {
    if (view.data) setDraft(view.data.settings);
  }, [view.data]);

  const save = useMutation({
    mutationFn: (settings: WeeklyReportSettings) => api.put<WeeklyReportView>("/health/weekly-report", settings),
    onSuccess: (next) => {
      queryClient.setQueryData(KEY, next);
      feedback.success(t("Haftalık özet ayarı kaydedildi"));
    },
    onError: (error) => feedback.error(error),
  });
  const show = useMutation({
    mutationFn: () => api.post<WeeklyReportDelivery>("/health/weekly-report/send", { preview: true }),
    onSuccess: setPreview,
    onError: (error) => feedback.error(error),
  });
  const send = useMutation({
    mutationFn: () => api.post<WeeklyReportDelivery>("/health/weekly-report/send", { preview: false }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: KEY });
      const sent = result.emailed.length + result.telegram;
      if (sent > 0) feedback.success(t("Haftalık özet gönderildi ({count} alıcı)", { count: sent }));
      if (result.errors.length > 0) feedback.error(new Error(result.errors.join("\n")));
    },
    onError: (error) => feedback.error(error),
  });

  if (view.isPending || !draft) return <LoadingBlock />;
  if (view.isError) return <ErrorBlock message={errorMessage(view.error)} onRetry={() => void view.refetch()} />;
  const { next_run_at, last, smtp_configured, settings } = view.data;
  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);

  return (
    <Panel flush title={t("Haftalık özet")}>
      <div className="flex flex-col gap-4 px-4 py-4 sm:px-6">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <p className="font-medium">{t("Haftalık özeti gönder")}</p>
            <p className="mt-0.5 text-[13.5px] text-ink-2">
              {t("Haftada bir yapılan denetimler, düzeltmeler, bekleyen sorunlar, yedekler, disk büyümesi ve en çok ziyaret alan siteler yöneticilerin bildirim kanallarına gönderilir.")}
            </p>
          </div>
          <Switch
            label={t("Haftalık özeti gönder")}
            checked={settings.enabled}
            disabled={save.isPending}
            onChange={(next) => save.mutate({ ...settings, enabled: next })}
          />
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <Select label={t("Gün")} value={draft.weekday} onChange={(event) => setDraft({ ...draft, weekday: Number(event.target.value) })}>
            {WEEKDAYS.map((name, index) => (
              <option key={name} value={index + 1}>
                {t(name)}
              </option>
            ))}
          </Select>
          <Input
            label={t("Saat (sunucu saati)")}
            type="time"
            value={clock(draft.at_minute)}
            onChange={(event) => event.target.value && setDraft({ ...draft, at_minute: minutesOf(event.target.value) })}
          />
        </div>
        <Input
          label={t("Ek alıcı (isteğe bağlı)")}
          type="email"
          placeholder="yonetici@ornek.com"
          hint={t("Yöneticilerin Bildirimler ayarındaki e-posta ve Telegram kanallarına ek olarak bu adrese de gönderilir.")}
          value={draft.email_to}
          onChange={(event) => setDraft({ ...draft, email_to: event.target.value })}
        />
        {!smtp_configured && (
          <p className="text-[13.5px] text-warn">{t("E-posta gönderimi için Ayarlar'da SMTP sunucusu tanımlı değil; yalnızca Telegram ve web kancaları çalışır.")}</p>
        )}
        <p className="tabular text-[13.5px] text-ink-3">
          {settings.enabled && next_run_at ? t("Sıradaki gönderim: {when}", { when: formatDateTime(next_run_at) }) : t("Haftalık özet kapalı.")}
          {last.at > 0 && ` · ${t("Son gönderim: {when}", { when: formatDateTime(last.at) })}`}
        </p>
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" disabled={!dirty} loading={save.isPending} onClick={() => save.mutate(draft)}>
            {t("Kaydet")}
          </Button>
          <Button icon={<Eye className="size-4" />} loading={show.isPending} onClick={() => show.mutate()}>
            {t("Önizle")}
          </Button>
          <Button icon={<Send className="size-4" />} loading={send.isPending} onClick={() => send.mutate()}>
            {t("Şimdi gönder")}
          </Button>
        </div>
      </div>
      <Modal open={preview !== null} onClose={() => setPreview(null)} title={preview?.subject ?? ""} wide>
        <pre className="max-h-[60vh] overflow-auto text-[13px] leading-relaxed whitespace-pre-wrap">{preview?.text}</pre>
      </Modal>
    </Panel>
  );
}
