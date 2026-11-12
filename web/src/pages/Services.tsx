import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, Play, RotateCw, Square } from "lucide-react";
import { Fragment, useMemo, useState } from "react";
import { useFeedback } from "../components/feedback";
import { ServiceHistoryPanel } from "../components/ServiceHistoryPanel";
import type { Tone } from "../components/ui";
import { Checkbox, ErrorBlock, IconButton, LoadingBlock, Mono, PageHeader, Panel, SearchInput, Status, Table, Td, Th } from "../components/ui";
import type { Service } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";

function serviceState(service: Service): { tone: Tone; label: string } {
  if (service.active === "failed") return { tone: "bad", label: t("Hata verdi") };
  if (service.active === "active" && service.sub === "running") return { tone: "ok", label: t("Çalışıyor") };
  if (service.active === "active") return { tone: "accent", label: t("Tamamlandı") };
  if (service.active === "activating") return { tone: "warn", label: t("Başlıyor") };
  return { tone: "idle", label: t("Durdu") };
}

const actionDone: Record<string, string> = {
  start: "{name} başlatıldı",
  stop: "{name} durduruldu",
  restart: "{name} yeniden başlatıldı",
  enable: "{name} açılışta başlayacak",
  disable: "{name} açılışta başlamayacak",
};

/** systemd'nin açılış durumlarından aç/kapa yapılamayanların okunur adları. */
const enabledLabels: Record<string, string> = {
  static: "Başka servis başlatır",
  "enabled-runtime": "Bu açılışa özel açık",
  indirect: "Dolaylı",
  generated: "Sistem üretir",
  masked: "Engelli",
  alias: "Takma ad",
  transient: "Geçici",
  linked: "Bağlı",
};

export function ServicesPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [onlyRunning, setOnlyRunning] = useState(false);
  // Ölçüm geçmişi açık olan servis (aynı anda biri).
  const [expanded, setExpanded] = useState<string | null>(null);

  const services = useQuery({
    queryKey: ["services"],
    queryFn: () => api.get<Service[]>("/system/services"),
    refetchInterval: 10000,
  });

  const act = useMutation({
    mutationFn: ({ name, action }: { name: string; action: string }) =>
      api.post(`/system/services/${encodeURIComponent(name)}/${action}`),
    onSuccess: (_, { name, action }) => feedback.success(t(actionDone[action], { name })),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["services"] }),
  });

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase();
    return (services.data ?? []).filter(
      (service) =>
        (!onlyRunning || service.sub === "running") &&
        (!needle || service.name.toLowerCase().includes(needle) || service.description.toLowerCase().includes(needle)),
    );
  }, [services.data, search, onlyRunning]);

  async function stop(service: Service) {
    const confirmed = await feedback.confirm({
      title: t("{name} durdurulsun mu?", { name: service.name }),
      description: t("Servis durduğunda ona bağlı siteler ve uygulamalar çalışmayabilir."),
      confirmLabel: t("Durdur"),
      danger: true,
    });
    if (confirmed) act.mutate({ name: service.name, action: "stop" });
  }

  const failed = services.data?.filter((service) => service.active === "failed").length ?? 0;

  return (
    <>
      <PageHeader
        title={t("Servisler")}
        description={
          failed > 0
            ? t("{count} servis hata vermiş durumda.", { count: failed })
            : t("Sunucuda tanımlı systemd servisleri.")
        }
      />
      {services.isError && <ErrorBlock message={errorMessage(services.error)} onRetry={() => void services.refetch()} />}
      {services.isPending && <LoadingBlock />}
      {services.data && (
        <Panel
          flush
          title={
            <div className="flex flex-wrap items-center gap-x-5 gap-y-3 font-normal">
              <SearchInput
                placeholder={t("Servis ara")}
                aria-label={t("Servis ara")}
                value={search}
                onChange={(event) => setSearch(event.target.value)}
              />
              <Checkbox label={t("Yalnızca çalışanlar")} checked={onlyRunning} onChange={(event) => setOnlyRunning(event.target.checked)} />
            </div>
          }
          actions={<span className="tabular text-[13.5px] text-ink-3">{t("{count} servis", { count: filtered.length })}</span>}
        >
          <Table>
            <thead>
              <tr>
                <Th>{t("Servis")}</Th>
                <Th>{t("Durum")}</Th>
                <Th>{t("Açılışta başlat")}</Th>
                <Th align="right">{t("İşlemler")}</Th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((service) => {
                const state = serviceState(service);
                const running = service.active === "active" || service.active === "activating";
                const busy = act.isPending && act.variables?.name === service.name;
                const canToggle = service.enabled === "enabled" || service.enabled === "disabled";
                const open = expanded === service.name;
                return (
                  <Fragment key={service.name}>
                  <tr className="hover:bg-sunken/50">
                    <Td>
                      <button
                        type="button"
                        aria-expanded={open}
                        onClick={() => setExpanded(open ? null : service.name)}
                        className="flex w-full items-start gap-1.5 text-left"
                        title={t("Ölçüm geçmişini göster")}
                      >
                        {open ? <ChevronDown aria-hidden className="mt-0.5 size-4 shrink-0 text-ink-3" /> : <ChevronRight aria-hidden className="mt-0.5 size-4 shrink-0 text-ink-3" />}
                        <span className="min-w-0">
                          <Mono className="font-medium">{service.name.replace(/\.service$/, "")}</Mono>
                          <span className="block max-w-[60ch] truncate text-[13px] text-ink-3">{service.description}</span>
                        </span>
                      </button>
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Status tone={state.tone}>{state.label}</Status>
                    </Td>
                    <Td>
                      {canToggle ? (
                        <Checkbox
                          label={service.enabled === "enabled" ? t("Açık") : t("Kapalı")}
                          checked={service.enabled === "enabled"}
                          disabled={busy}
                          onChange={(event) => act.mutate({ name: service.name, action: event.target.checked ? "enable" : "disable" })}
                        />
                      ) : (
                        <span className="text-ink-3" title={service.enabled}>
                          {enabledLabels[service.enabled] ? t(enabledLabels[service.enabled]) : service.enabled || "—"}
                        </span>
                      )}
                    </Td>
                    <Td align="right">
                      <div className="flex justify-end gap-0.5">
                        {running ? (
                          <>
                            <IconButton label={t("Yeniden başlat")} disabled={busy} onClick={() => act.mutate({ name: service.name, action: "restart" })}>
                              <RotateCw className="size-4" />
                            </IconButton>
                            <IconButton label={t("Durdur")} disabled={busy} onClick={() => void stop(service)}>
                              <Square className="size-4" />
                            </IconButton>
                          </>
                        ) : (
                          <IconButton label={t("Başlat")} disabled={busy} onClick={() => act.mutate({ name: service.name, action: "start" })}>
                            <Play className="size-4" />
                          </IconButton>
                        )}
                      </div>
                    </Td>
                  </tr>
                  {open && (
                    <tr>
                      <td colSpan={4} className="border-b border-line bg-sunken/30 px-4 py-4 sm:px-6">
                        <ServiceHistoryPanel unit={service.name} />
                      </td>
                    </tr>
                  )}
                  </Fragment>
                );
              })}
            </tbody>
          </Table>
          {filtered.length === 0 && <p className="px-4 sm:px-6 py-8 text-ink-2">{t("Aramanıza uyan servis yok.")}</p>}
        </Panel>
      )}
    </>
  );
}
