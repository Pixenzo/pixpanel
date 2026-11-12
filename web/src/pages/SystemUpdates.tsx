import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { PackageCheck, Power, RefreshCw, ShieldAlert } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, ErrorBlock, LoadingBlock, Modal, Mono, PageHeader, Panel, SearchInput, Switch, Table, Td, Th } from "../components/ui";
import type { HealthSettings, HealthView } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

interface PendingPackage {
  name: string;
  installed: string;
  candidate: string;
  security: boolean;
}

interface UpdatesView {
  checked_at: number;
  total: number;
  security: number;
  packages: PendingPackage[];
  reboot: { required: boolean; packages: string[]; scheduled_at: number | null; scheduled_mode: string | null };
  kernel: { running: string; latest: string | null; outdated: boolean };
  unattended: { installed: boolean; configured: boolean; last_run: number | null; last_install: number | null };
  services_to_restart: string[] | null;
  busy: boolean;
  refresh_error: string | null;
}

/** Tabloda en çok bu kadar satır çizilir; sunucuda yüzlerce paket bekleyebilir. */
const ROW_LIMIT = 300;

function Stat({ label, value, hint, tone }: { label: string; value: string; hint?: string; tone?: "warn" | "ok" }) {
  return (
    <div className="rounded-xl border border-line bg-surface px-4 py-3.5 shadow-card">
      <p className="text-[13px] text-ink-2">{label}</p>
      <p className={`tabular mt-1 text-[22px] font-semibold tracking-[-0.02em] ${tone === "warn" ? "text-warn" : tone === "ok" ? "text-ok" : ""}`}>{value}</p>
      {hint && <p className="mt-0.5 text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}

export function SystemUpdatesPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [watching, setWatching] = useState<{ jobId: number; title: string } | null>(null);
  // Zamanlanmış yeniden başlatmanın geri sayımı için saniyede bir yenilenen saat.
  const [clock, setClock] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const timer = window.setInterval(() => setClock(Math.floor(Date.now() / 1000)), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const updates = useQuery({
    queryKey: ["system-updates"],
    queryFn: () => api.get<UpdatesView>("/system/updates"),
    refetchInterval: 30000,
  });
  const health = useQuery({ queryKey: ["health"], queryFn: () => api.get<HealthView>("/health") });

  const refresh = useMutation({
    mutationFn: () => api.post<UpdatesView>("/system/updates/refresh"),
    onSuccess: (view) => {
      queryClient.setQueryData(["system-updates"], view);
      void queryClient.invalidateQueries({ queryKey: ["health"] });
      if (view.refresh_error) feedback.error(new Error(t("Paket listesi yenilenemedi: {reason}", { reason: view.refresh_error })));
      else feedback.success(t("Paket listesi yenilendi"));
    },
    onError: (error) => feedback.error(error),
  });

  const install = useMutation({
    mutationFn: (scope: "security" | "all") => api.post<{ job_id: number }>("/system/updates/install", { scope }),
    onSuccess: (result, scope) => {
      setWatching({ jobId: result.job_id, title: scope === "security" ? t("Güvenlik güncellemeleri") : t("Paket güncellemeleri") });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
      void queryClient.invalidateQueries({ queryKey: ["system-updates"] });
    },
    onError: (error) => feedback.error(error),
  });

  const reboot = useMutation({
    mutationFn: () => api.post("/system/reboot"),
    onSuccess: () => {
      feedback.success(t("Sunucu 1 dakika içinde yeniden başlatılacak"));
      void queryClient.invalidateQueries({ queryKey: ["system-updates"] });
    },
    onError: (error) => feedback.error(error),
  });

  const cancelReboot = useMutation({
    mutationFn: () => api.post("/system/reboot/cancel"),
    onSuccess: () => {
      feedback.success(t("Yeniden başlatma iptal edildi"));
      void queryClient.invalidateQueries({ queryKey: ["system-updates"] });
    },
    onError: (error) => feedback.error(error),
  });

  const saveSettings = useMutation({
    mutationFn: (settings: HealthSettings) => api.put<HealthView>("/health/settings", settings),
    onSuccess: (view) => {
      queryClient.setQueryData(["health"], view);
      void queryClient.invalidateQueries({ queryKey: ["system-updates"] });
    },
    onError: (error) => feedback.error(error),
  });

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return (updates.data?.packages ?? []).filter((item) => !needle || item.name.toLowerCase().includes(needle));
  }, [updates.data, search]);

  function afterJob() {
    void queryClient.invalidateQueries({ queryKey: ["system-updates"] });
    void queryClient.invalidateQueries({ queryKey: ["health"] });
  }

  async function confirmInstall(scope: "security" | "all") {
    if (!updates.data) return;
    const confirmed = await feedback.confirm({
      title: scope === "security" ? t("Güvenlik güncellemeleri kurulsun mu?") : t("Bütün güncellemeler kurulsun mu?"),
      description:
        scope === "security"
          ? t("{count} güvenlik güncellemesi kurulacak. Çıktıyı canlı izleyebilirsiniz.", { count: updates.data.security })
          : t("{count} paket güncellenecek. Bazı servisler kısa süre yeniden başlayabilir.", { count: updates.data.total }),
      confirmLabel: t("Kur"),
    });
    if (confirmed) install.mutate(scope);
  }

  async function confirmReboot() {
    const confirmed = await feedback.confirm({
      title: t("Sunucu yeniden başlatılsın mı?"),
      description: t("Sunucu 1 dakika içinde yeniden başlar. Siteler, posta ve panel birkaç dakika erişilemez olur. İptal etmek için bu sayfayı kullanabilirsiniz."),
      confirmLabel: t("Yeniden başlat"),
      danger: true,
    });
    if (confirmed) reboot.mutate();
  }

  if (updates.isPending) return <LoadingBlock />;
  if (updates.isError) return <ErrorBlock message={errorMessage(updates.error)} onRetry={() => void updates.refetch()} />;

  const data = updates.data;
  const busy = data.busy || install.isPending;
  const settings = health.data?.settings;
  const secondsLeft = data.reboot.scheduled_at ? Math.max(0, data.reboot.scheduled_at - clock) : null;

  return (
    <>
      <PageHeader
        title={t("Güncellemeler")}
        description={t("İşletim sistemi paketlerinin güncellemeleri. Güvenlik güncellemelerini geciktirmeyin.")}
        actions={
          <Button icon={<RefreshCw className="size-4" />} loading={refresh.isPending} disabled={busy} onClick={() => refresh.mutate()}>
            {t("Listeyi yenile")}
          </Button>
        }
      />

      {secondsLeft !== null && (
        <div role="alert" className="mb-5 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-line bg-bad-soft px-4 py-3.5 sm:px-6">
          <Power aria-hidden className="size-5 text-bad" />
          <p className="min-w-0 flex-1 font-medium text-bad">
            {t("Sunucu {seconds} saniye sonra yeniden başlatılacak.", { seconds: secondsLeft })}
          </p>
          <Button size="sm" loading={cancelReboot.isPending} onClick={() => cancelReboot.mutate()}>
            {t("İptal et")}
          </Button>
        </div>
      )}

      {data.reboot.required && secondsLeft === null && (
        <div className="mb-5 flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-line bg-warn-soft px-4 py-3.5 sm:px-6">
          <ShieldAlert aria-hidden className="size-5 text-warn" />
          <div className="min-w-0 flex-1">
            <p className="font-medium text-warn">{t("Sunucunun yeniden başlatılması gerekiyor")}</p>
            <p className="text-[13.5px] text-ink-2">
              {t("Kurulan güncellemeler (çoğunlukla çekirdek) yeniden başlatılana kadar devreye girmez.")}
              {data.reboot.packages.length > 0 && <> {data.reboot.packages.join(", ")}</>}
            </p>
          </div>
          <Button size="sm" variant="danger" icon={<Power className="size-3.5" />} loading={reboot.isPending} disabled={busy} onClick={() => void confirmReboot()}>
            {t("Yeniden başlat")}
          </Button>
        </div>
      )}

      <div className="mb-5 grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Stat label={t("Bekleyen paket")} value={String(data.total)} tone={data.total === 0 ? "ok" : undefined} hint={data.total === 0 ? t("Sistem güncel") : undefined} />
        <Stat label={t("Güvenlik güncellemesi")} value={String(data.security)} tone={data.security > 0 ? "warn" : "ok"} />
        <Stat
          label={t("Çekirdek")}
          value={data.kernel.running || "—"}
          tone={data.kernel.outdated ? "warn" : undefined}
          hint={data.kernel.outdated && data.kernel.latest ? t("Kurulu en yeni: {version}", { version: data.kernel.latest }) : undefined}
        />
        <Stat label={t("Son denetim")} value={formatRelative(data.checked_at)} hint={formatDateTime(data.checked_at)} />
      </div>

      {data.refresh_error && (
        <div className="mb-5">
          <ErrorBlock message={t("Paket listesi yenilenemedi, eldeki liste gösteriliyor: {reason}", { reason: data.refresh_error })} />
        </div>
      )}

      <div className="flex flex-col gap-5">
        <Panel
          flush
          title={
            <div className="flex flex-wrap items-center gap-x-5 gap-y-3 font-normal">
              <SearchInput placeholder={t("Paket ara")} aria-label={t("Paket ara")} value={search} onChange={(event) => setSearch(event.target.value)} />
            </div>
          }
          actions={
            <>
              <Button
                size="sm"
                variant="primary"
                icon={<ShieldAlert className="size-3.5" />}
                disabled={busy || data.security === 0}
                onClick={() => void confirmInstall("security")}
              >
                {t("Güvenlik güncellemelerini kur")}
              </Button>
              <Button size="sm" icon={<PackageCheck className="size-3.5" />} disabled={busy || data.total === 0} onClick={() => void confirmInstall("all")}>
                {t("Hepsini kur")}
              </Button>
            </>
          }
        >
          {data.busy && (
            <p className="border-b border-line px-4 py-3 text-[13.5px] text-ink-2 sm:px-6">{t("Bir güncelleme görevi çalışıyor; bitince liste yenilenir.")}</p>
          )}
          {data.packages.length === 0 ? (
            <p className="px-4 py-8 text-ink-2 sm:px-6">{t("Bekleyen paket güncellemesi yok. Sistem güncel.")}</p>
          ) : (
            <>
              <Table>
                <thead>
                  <tr>
                    <Th>{t("Paket")}</Th>
                    <Th>{t("Kurulu sürüm")}</Th>
                    <Th>{t("Yeni sürüm")}</Th>
                    <Th>{t("Tür")}</Th>
                  </tr>
                </thead>
                <tbody>
                  {filtered.slice(0, ROW_LIMIT).map((item) => (
                    <tr key={item.name} className="hover:bg-sunken/50">
                      <Td>
                        <Mono className="font-medium">{item.name}</Mono>
                      </Td>
                      <Td>
                        <Mono className="text-ink-2">{item.installed || "—"}</Mono>
                      </Td>
                      <Td>
                        <Mono>{item.candidate}</Mono>
                      </Td>
                      <Td>{item.security ? <Badge tone="warn">{t("Güvenlik")}</Badge> : <Badge>{t("Normal")}</Badge>}</Td>
                    </tr>
                  ))}
                </tbody>
              </Table>
              {filtered.length === 0 && <p className="px-4 py-8 text-ink-2 sm:px-6">{t("Aramanıza uyan paket yok.")}</p>}
              {filtered.length > ROW_LIMIT && (
                <p className="px-4 py-3 text-[13.5px] text-ink-3 sm:px-6">{t("İlk {count} paket gösteriliyor; aramayla daraltın.", { count: ROW_LIMIT })}</p>
              )}
            </>
          )}
        </Panel>

        {data.services_to_restart && data.services_to_restart.length > 0 && (
          <Panel title={t("Yeniden başlatılması gereken servisler")}>
            <p className="mb-3 text-ink-2">{t("Bu servisler güncellenmiş kitaplıkların eski sürümünü kullanıyor. Servisler sayfasından yeniden başlatabilirsiniz.")}</p>
            <div className="flex flex-wrap gap-2">
              {data.services_to_restart.map((unit) => (
                <Badge key={unit} tone="warn">
                  {unit}
                </Badge>
              ))}
            </div>
          </Panel>
        )}

        <Panel title={t("Kendiliğinden güvenlik güncellemesi")}>
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1">
              <p className="font-medium">{t("Güvenlik güncellemelerini her gün kendiliğinden kur")}</p>
              <p className="mt-0.5 text-[13.5px] text-ink-2">
                {data.unattended.installed && data.unattended.configured
                  ? t("Açık.")
                  : data.unattended.installed
                    ? t("unattended-upgrades kurulu ama kapalı.")
                    : t("Açıldığında unattended-upgrades paketi kurulur.")}{" "}
                {data.unattended.last_install
                  ? t("Son kurulum: {time}.", { time: formatDateTime(data.unattended.last_install) })
                  : data.unattended.last_run
                    ? t("Son çalışma: {time}.", { time: formatDateTime(data.unattended.last_run) })
                    : t("Henüz çalışmadı.")}
              </p>
            </div>
            <Switch
              label={t("Güvenlik güncellemelerini her gün kendiliğinden kur")}
              checked={settings?.security_updates ?? false}
              disabled={!settings || saveSettings.isPending}
              onChange={(next) => settings && saveSettings.mutate({ ...settings, security_updates: next })}
            />
          </div>
        </Panel>

        {!data.reboot.required && secondsLeft === null && (
          <Panel title={t("Sunucuyu yeniden başlat")}>
            <div className="flex flex-wrap items-center gap-4">
              <p className="min-w-0 flex-1 text-ink-2">{t("Yeniden başlatma gerekmiyor. Yine de başlatmak isterseniz 1 dakika sonrasına zamanlanır; iptal edebilirsiniz.")}</p>
              <Button icon={<Power className="size-4" />} loading={reboot.isPending} disabled={busy} onClick={() => void confirmReboot()}>
                {t("Yeniden başlat")}
              </Button>
            </div>
          </Panel>
        )}
      </div>

      <Modal
        open={watching !== null}
        onClose={() => {
          setWatching(null);
          afterJob();
        }}
        title={watching?.title ?? ""}
        wide
      >
        {watching && <JobLog jobId={watching.jobId} onDone={afterJob} />}
      </Modal>
    </>
  );
}
