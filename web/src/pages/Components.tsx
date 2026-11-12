import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, RefreshCw, Trash2 } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, ErrorBlock, LoadingBlock, Modal, Mono, PageHeader, Panel, Status } from "../components/ui";
import type { ComponentStatus, NodeVersionInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

/** Sunucunun eklediği kaldırma bilgisi. */
type ComponentRow = ComponentStatus & { removable?: boolean; in_use?: string | null; warning?: string | null };

const categories: { id: ComponentStatus["category"]; title: string }[] = [
  { id: "web", title: "Web sunucusu" },
  { id: "php", title: "PHP sürümleri" },
  { id: "node", title: "Node.js sürümleri" },
  { id: "database", title: "Veritabanı ve önbellek" },
  { id: "container", title: "Konteyner" },
  { id: "mail", title: "Posta" },
];

export function ComponentsPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [watching, setWatching] = useState<{ jobId: number; title: string } | null>(null);

  const components = useQuery({
    queryKey: ["components"],
    queryFn: () => api.get<ComponentRow[]>("/components"),
  });

  // Kurulu Node.js sürümlerinin yama güncellemeleri (sürüm listesine ulaşılamazsa sessizce boş kalır).
  const nodeVersions = useQuery({
    queryKey: ["node-versions"],
    queryFn: () => api.get<NodeVersionInfo[]>("/system/node/versions"),
    staleTime: 60000,
  });

  const updateNode = useMutation({
    mutationFn: (info: NodeVersionInfo) => api.post<{ job_id: number }>(`/system/node/versions/${info.major}/update`),
    onSuccess: (result, info) => {
      setWatching({ jobId: result.job_id, title: t("Node.js {major} güncellemesi", { major: info.major }) });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmNodeUpdate(info: NodeVersionInfo) {
    const confirmed = await feedback.confirm({
      title: t("Node.js {major} güncellensin mi?", { major: info.major }),
      description:
        (info.sites > 0
          ? t("{count} site bu sürümü kullanıyor; servisleri güncellemeden sonra sırayla yeniden başlatılır ve kısa süre yanıt vermeyebilir.", { count: info.sites })
          : t("Bu sürümü kullanan site yok.")) +
        " " +
        t("Bir servis yeni sürümle ayağa kalkmazsa eski sürüme dönülür."),
      confirmLabel: t("Güncelle"),
    });
    if (confirmed) updateNode.mutate(info);
  }

  const install = useMutation({
    // `force`: kurulum uyarısı (ör. yetersiz bellek) gösterildi ve onaylandı.
    mutationFn: ({ component, force }: { component: ComponentStatus; force: boolean }) =>
      api.post<{ job_id: number }>(`/components/${component.id}/install${force ? "?force=true" : ""}`),
    onSuccess: (result, { component }) => {
      setWatching({ jobId: result.job_id, title: t("{name} kurulumu", { name: component.name }) });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });

  const remove = useMutation({
    // uv ile kurulan Python sürümleri paket değildir; doğrudan silinir, görev başlamaz.
    mutationFn: async (component: ComponentRow) => {
      if (component.id.startsWith("python-")) {
        await api.delete(`/components/${component.id}`);
        return { job_id: null };
      }
      return api.post<{ job_id: number | null }>(`/components/${component.id}/remove`);
    },
    onSuccess: (result, component) => {
      if (result.job_id === null) {
        feedback.success(t("{name} kaldırıldı", { name: component.name }));
        void queryClient.invalidateQueries({ queryKey: ["components"] });
        void queryClient.invalidateQueries({ queryKey: ["site-options"] });
        return;
      }
      setWatching({ jobId: result.job_id, title: t("{name} kaldırma", { name: component.name }) });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(component: ComponentRow) {
    const confirmed = await feedback.confirm({
      title: t("{name} kaldırılsın mı?", { name: component.name }),
      description: component.id.startsWith("python-")
        ? t("Bu sürümü kullanan site varsa kaldırılamaz; önce sitelerin Python sürümünü değiştirin.")
        : t("Paketler kaldırılır, yapılandırma dosyaları ve veriler silinmez; yeniden kurarsanız yerinde bulunur."),
      confirmLabel: t("Kaldır"),
      danger: true,
    });
    if (confirmed) remove.mutate(component);
  }

  async function startInstall(component: ComponentRow) {
    if (component.warning) {
      const confirmed = await feedback.confirm({
        title: t("{name} yine de kurulsun mu?", { name: component.name }),
        description: component.warning,
        confirmLabel: t("Yine de kur"),
        danger: true,
      });
      if (!confirmed) return;
    }
    install.mutate({ component, force: Boolean(component.warning) });
  }

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["components"] });
    void queryClient.invalidateQueries({ queryKey: ["site-options"] });
    void queryClient.invalidateQueries({ queryKey: ["node-versions"] });
  }

  return (
    <>
      <PageHeader
        title={t("Bileşenler")}
        description={t("Sunucuda çalışan yazılımlar. İhtiyacınız olanı buradan kurun; kurulum çıktısını canlı izleyebilirsiniz.")}
      />
      {components.isError && <ErrorBlock message={errorMessage(components.error)} onRetry={() => void components.refetch()} />}
      {components.isPending && <LoadingBlock />}
      <div className="flex flex-col gap-5">
        {components.data &&
          categories.map((category) => {
            const items = components.data.filter((component) => component.category === category.id);
            if (items.length === 0) return null;
            return (
              <Panel key={category.id} flush title={t(category.title)}>
                <ul>
                  {items.map((component) => {
                    const nodeInfo = component.id.startsWith("node-") ? nodeVersions.data?.find((info) => `node-${info.major}` === component.id) : undefined;
                    return (
                      <li key={component.id} className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-line px-4 sm:px-6 py-3 last:border-b-0">
                        <div className="min-w-48 flex-1">
                          <p className="font-medium">{component.name}</p>
                          <p className="text-[13.5px] text-ink-2">{t(component.description)}</p>
                          {component.installed && component.in_use && <p className="mt-1 text-[13.5px] text-warn">{component.in_use}</p>}
                        </div>
                        {component.installed ? (
                          <>
                            <Mono className="text-ink-2">{component.version}</Mono>
                            {nodeInfo?.update_available && (
                              <Badge tone="warn">
                                {nodeInfo.security ? t("Güvenlik güncellemesi: {version}", { version: nodeInfo.latest ?? "" }) : t("Güncelleme var: {version}", { version: nodeInfo.latest ?? "" })}
                              </Badge>
                            )}
                            <div className="w-28">
                              {/* Servisi olmayan bileşen (araç, kitaplık) "durmuş" değildir; yalnızca kuruludur. */}
                              {component.running ? (
                                <Status tone="ok">{t("Çalışıyor")}</Status>
                              ) : component.service ? (
                                <Status tone="warn">{t("Durdu")}</Status>
                              ) : (
                                <Status tone="ok">{t("Kurulu")}</Status>
                              )}
                            </div>
                            {nodeInfo?.update_available && (
                              <Button
                                size="sm"
                                variant="primary"
                                icon={<RefreshCw className="size-3.5" />}
                                loading={updateNode.isPending && updateNode.variables?.major === nodeInfo.major}
                                onClick={() => void confirmNodeUpdate(nodeInfo)}
                              >
                                {t("Güncelle")}
                              </Button>
                            )}
                            {(component.removable || (component.id.startsWith("python-") && component.installed)) && (
                              <Button
                                size="sm"
                                variant="ghost"
                                icon={<Trash2 className="size-3.5" />}
                                loading={remove.isPending && remove.variables?.id === component.id}
                                disabled={Boolean(component.in_use)}
                                title={component.in_use ?? undefined}
                                onClick={() => void confirmRemove(component)}
                              >
                                {t("Kaldır")}
                              </Button>
                            )}
                          </>
                        ) : (
                          <Button
                            size="sm"
                            icon={<Download className="size-3.5" />}
                            loading={install.isPending && install.variables?.component.id === component.id}
                            onClick={() => void startInstall(component)}
                          >
                            {t("Kur")}
                          </Button>
                        )}
                      </li>
                    );
                  })}
                </ul>
              </Panel>
            );
          })}
      </div>
      <Modal
        open={watching !== null}
        onClose={() => {
          setWatching(null);
          refresh();
        }}
        title={watching?.title ?? ""}
        wide
      >
        {watching && <JobLog jobId={watching.jobId} onDone={refresh} />}
      </Modal>
    </>
  );
}
