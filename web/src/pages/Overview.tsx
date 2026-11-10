// Genel bakış: sunucunun sağlık durumu. Panel denetimleri sürekli yapar, güvenli
// olanları kendisi düzeltir; yöneticiye kalanlar burada tek tıkla düzeltilecek
// biçimde listelenir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowRight, Bot, CircleAlert, CircleCheck, Info, RefreshCw, TriangleAlert, UserRound, Wrench } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import { Button, ErrorBlock, LoadingBlock, Panel, Switch } from "../components/ui";
import type { HealthAction, HealthFinding, HealthSettings, HealthSeverity, HealthView } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

const severityView: Record<HealthSeverity, { icon: LucideIcon; tone: Tone; label: string }> = {
  critical: { icon: CircleAlert, tone: "bad", label: "Kritik" },
  warning: { icon: TriangleAlert, tone: "warn", label: "Uyarı" },
  info: { icon: Info, tone: "idle", label: "Öneri" },
};

const toneText: Record<Tone, string> = { ok: "text-ok", warn: "text-warn", bad: "text-bad", idle: "text-ink-3", accent: "text-accent" };
const toneSoft: Record<Tone, string> = { ok: "bg-ok-soft", warn: "bg-warn-soft", bad: "bg-bad-soft", idle: "bg-sunken", accent: "bg-accent-soft" };

/** Kendiliğinden yapılan işler; her biri ayrı ayrı açılıp kapatılır. */
const automations: { key: keyof HealthSettings; label: string; hint: string }[] = [
  {
    key: "restart_services",
    label: "Çöken servisleri yeniden başlat",
    hint: "Web sunucusu, veritabanı ya da posta servisi hata verip durursa panel yeniden başlatır.",
  },
  {
    key: "issue_certificates",
    label: "Sertifikasız sitelere sertifika al",
    hint: "Alan adı bu sunucuyu gösteren sitelere ücretsiz Let's Encrypt sertifikası alınır.",
  },
  {
    key: "check_sites",
    label: "Siteleri dakikada bir yokla",
    hint: "Yanıt vermeyen ya da sunucu hatası döndüren site hemen bildirilir.",
  },
  {
    key: "security_updates",
    label: "Güvenlik güncellemelerini kur",
    hint: "İşletim sisteminin güvenlik yamaları her gün kendiliğinden kurulur; öteki güncellemeler size bırakılır.",
  },
  {
    key: "update_wordpress",
    label: "WordPress sitelerini gece güncelle",
    hint: "Her gece 02:00–05:00 arasında bekleyen çekirdek, eklenti ve tema güncellemeleri kurulur. Önce sitenin yedeği alınır; güncelleme siteyi bozarsa haber verilir.",
  },
  {
    key: "notify",
    label: "Sorunları ve yapılanları bildir",
    hint: "Kritik sorunlar ve panelin yaptığı düzeltmeler e-posta ya da Telegram ile gönderilir (Hesabım → Bildirimler).",
  },
];

/** Bir saniyede bir yeniden çizdirir; "12 sn önce" gibi göreli süreler güncel kalsın diye. */
function useNow(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

function headline(findings: HealthFinding[]): { tone: Tone; icon: LucideIcon; title: string; subtitle: string } {
  const critical = findings.filter((finding) => finding.severity === "critical").length;
  const warning = findings.filter((finding) => finding.severity === "warning").length;
  if (critical > 0) {
    return {
      tone: "bad",
      icon: CircleAlert,
      title: critical === 1 ? t("1 kritik sorun var") : t("{count} kritik sorun var", { count: critical }),
      subtitle: t("Siteleriniz ya da sunucunuz etkileniyor olabilir; aşağıdaki sorunlara hemen bakın."),
    };
  }
  if (warning > 0) {
    return {
      tone: "warn",
      icon: TriangleAlert,
      title: warning === 1 ? t("1 konu ilginizi bekliyor") : t("{count} konu ilginizi bekliyor", { count: warning }),
      subtitle: t("Sunucu çalışıyor; aşağıdakiler düzeltilmezse ileride sorun çıkarabilir."),
    };
  }
  return {
    tone: "ok",
    icon: CircleCheck,
    title: t("Her şey yolunda"),
    subtitle: findings.length > 0 ? t("Acil bir şey yok; aşağıda birkaç öneri var.") : t("Sunucunuzda dikkat gerektiren bir şey bulunmadı."),
  };
}

function FindingRow({ finding, onFix, fixing }: { finding: HealthFinding; onFix: () => void; fixing: boolean }) {
  const view = severityView[finding.severity];
  return (
    <li className="flex gap-3 px-4 py-3.5">
      <span className={classNames("mt-0.5 flex size-7 shrink-0 items-center justify-center rounded-full", toneSoft[view.tone], toneText[view.tone])}>
        <view.icon aria-hidden className="size-4" />
      </span>
      <div className="min-w-0 flex-1">
        <p className="font-medium">
          <span className="sr-only">{t(view.label)}: </span>
          {finding.title}
        </p>
        <p className="mt-0.5 max-w-[75ch] text-ink-2">{finding.detail}</p>
        <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-2">
          {finding.fix && (
            <Button size="sm" variant={finding.severity === "info" ? "secondary" : "primary"} loading={fixing} icon={<Wrench className="size-3.5" />} onClick={onFix}>
              {t(finding.fix)}
            </Button>
          )}
          {finding.link && (
            <Link to={finding.link} className="inline-flex items-center gap-1 text-[13px] font-medium text-accent hover:underline">
              {finding.fix ? t("Ayrıntı") : t("İlgili sayfayı aç")} <ArrowRight aria-hidden className="size-3.5" />
            </Link>
          )}
          {finding.auto && (
            <span className="inline-flex items-center gap-1 text-[13px] text-ink-3">
              <Bot aria-hidden className="size-3.5" /> {t("Panel bunu kendisi düzeltmeyi deniyor")}
            </span>
          )}
          <span className="text-[13px] text-ink-3">{t("{when} fark edildi", { when: formatRelative(finding.since) })}</span>
        </div>
      </div>
    </li>
  );
}

function ActionRow({ action }: { action: HealthAction }) {
  const Icon = action.automatic ? Bot : UserRound;
  return (
    <li className="flex gap-2.5 px-4 py-2.5">
      <Icon aria-hidden className={classNames("mt-0.5 size-4 shrink-0", action.success ? "text-ok" : "text-bad")} />
      <div className="min-w-0 flex-1">
        <p className="text-[13.5px] font-medium break-words">{action.summary}</p>
        <p className="text-[12.5px] break-words text-ink-2">{action.detail}</p>
        <p className="text-[12.5px] text-ink-3">
          {action.automatic ? t("Panel kendiliğinden yaptı") : t("Elle başlatıldı")} · {formatRelative(action.created_at)}
          {!action.success && <span className="text-bad"> · {t("başarısız")}</span>}
        </p>
      </div>
    </li>
  );
}

export function OverviewPage() {
  const queryClient = useQueryClient();
  const feedback = useFeedback();
  const now = useNow();
  // Bulgular değişince sunucu canlı akıştan haber verir; yoklama yalnızca "son denetim" süresini taze tutar.
  const health = useQuery({ queryKey: ["health"], queryFn: () => api.get<HealthView>("/health"), refetchInterval: 60000 });

  const recheck = useMutation({
    mutationFn: () => api.post<HealthView>("/health/check"),
    onSuccess: (view) => queryClient.setQueryData(["health"], view),
    onError: (error) => feedback.error(error),
  });
  const fix = useMutation({
    mutationFn: (key: string) => api.post<{ message: string }>("/health/fix", { key }),
    onSuccess: (result) => {
      feedback.success(result.message);
      void queryClient.invalidateQueries({ queryKey: ["health"] });
    },
    onError: (error) => feedback.error(error),
  });
  const saveSettings = useMutation({
    mutationFn: (settings: HealthSettings) => api.put<HealthView>("/health/settings", settings),
    onSuccess: (view) => queryClient.setQueryData(["health"], view),
    onError: (error) => feedback.error(error),
  });

  if (health.isPending) return <LoadingBlock />;
  if (health.isError) return <ErrorBlock message={errorMessage(health.error)} onRetry={() => void health.refetch()} />;

  const { report, settings, actions } = health.data;
  const state = headline(report.findings);
  const waiting = report.checked_at === 0;
  const age = Math.max(0, now - report.checked_at);

  return (
    <div className="flex flex-col gap-4">
      <section className="flex flex-wrap items-center gap-x-5 gap-y-4 rounded-xl border border-line bg-surface px-4 py-5 shadow-card sm:px-6">
        <span className={classNames("flex size-12 shrink-0 items-center justify-center rounded-full", toneSoft[state.tone], toneText[state.tone])}>
          <state.icon aria-hidden className="size-6" />
        </span>
        <div className="min-w-0 flex-1 basis-64">
          <h1 className="text-[22px] leading-tight font-semibold tracking-[-0.015em] sm:text-[26px]">{waiting ? t("İlk denetim yapılıyor") : state.title}</h1>
          <p className="mt-1 text-ink-2">{waiting ? t("Panel yeni başladı; sonuçlar birkaç saniye içinde burada olacak.") : state.subtitle}</p>
          {!waiting && (
            <p className="tabular mt-2 text-[13px] text-ink-3">
              {t("{count} denetim yapıldı", { count: report.checks })} · {age < 5 ? t("az önce") : t("{seconds} sn önce", { seconds: age })} ·{" "}
              {t("dakikada bir yinelenir")}
            </p>
          )}
        </div>
        <Button loading={recheck.isPending} icon={<RefreshCw className="size-4" />} onClick={() => recheck.mutate()} className="max-sm:w-full">
          {t("Şimdi denetle")}
        </Button>
      </section>

      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_22rem]">
        <Panel flush title={report.findings.length > 0 ? t("Bulgular") : undefined}>
          {report.findings.length === 0 ? (
            <div className="px-4 py-10 sm:text-center">
              <p className="font-semibold">{t("Bekleyen bir şey yok")}</p>
              <p className="mx-auto mt-1 max-w-[52ch] text-ink-2">
                {t("Panel servisleri, diskleri, sertifikaları, yedekleri ve sitelerinizi izlemeyi sürdürüyor; bir sorun çıkarsa burada görünür.")}
              </p>
            </div>
          ) : (
            <ul className="divide-y divide-line">
              {report.findings.map((finding) => (
                <FindingRow
                  key={finding.key}
                  finding={finding}
                  fixing={fix.isPending && fix.variables === finding.key}
                  onFix={() => fix.mutate(finding.key)}
                />
              ))}
            </ul>
          )}
        </Panel>

        <div className="flex flex-col gap-4">
          <Panel flush title={t("Panelin kendiliğinden yaptıkları")}>
            <ul className="divide-y divide-line">
              {automations.map((item) => (
                <li key={item.key} className="flex items-start gap-3 px-4 py-3">
                  <div className="min-w-0 flex-1">
                    <p className="font-medium">{t(item.label)}</p>
                    <p className="mt-0.5 text-[13px] text-ink-2">{t(item.hint)}</p>
                  </div>
                  <Switch
                    label={t(item.label)}
                    checked={settings[item.key]}
                    disabled={saveSettings.isPending}
                    onChange={(next) => saveSettings.mutate({ ...settings, [item.key]: next })}
                  />
                </li>
              ))}
            </ul>
          </Panel>

          <Panel flush title={t("Son yapılanlar")}>
            {actions.length === 0 ? (
              <p className="px-4 py-6 text-ink-2">{t("Henüz bir düzeltme yapılmadı. Panel bir sorunu giderdiğinde kaydı burada görünür.")}</p>
            ) : (
              <ul className="max-h-96 divide-y divide-line overflow-y-auto">
                {actions.map((action) => (
                  <ActionRow key={action.id} action={action} />
                ))}
              </ul>
            )}
          </Panel>
        </div>
      </div>
    </div>
  );
}
