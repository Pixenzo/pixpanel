import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Trash2 } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, IconButton, LoadingBlock, Modal, Mono, PageHeader, Panel, Status } from "../components/ui";
import type { ComponentStatus } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

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
    queryFn: () => api.get<ComponentStatus[]>("/components"),
  });

  const install = useMutation({
    mutationFn: (component: ComponentStatus) => api.post<{ job_id: number }>(`/components/${component.id}/install`),
    onSuccess: (result, component) => {
      setWatching({ jobId: result.job_id, title: t("{name} kurulumu", { name: component.name }) });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });

  // Yalnız uv ile kurulan Python sürümleri kaldırılabilir; kullanan site varsa sunucu reddeder.
  const uninstall = useMutation({
    mutationFn: (component: ComponentStatus) => api.delete(`/components/${component.id}`),
    onSuccess: (_result, component) => {
      feedback.success(t("{name} kaldırıldı", { name: component.name }));
      refresh();
    },
    onError: (error) => feedback.error(error),
  });

  async function removeVersion(component: ComponentStatus) {
    const confirmed = await feedback.confirm({
      title: t("{name} kaldırılsın mı?", { name: component.name }),
      description: t("Bu sürümü kullanan site varsa kaldırılamaz; önce sitelerin Python sürümünü değiştirin."),
      confirmLabel: t("Kaldır"),
      danger: true,
    });
    if (confirmed) uninstall.mutate(component);
  }

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["components"] });
    void queryClient.invalidateQueries({ queryKey: ["site-options"] });
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
                  {items.map((component) => (
                    <li key={component.id} className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-line px-4 sm:px-6 py-3 last:border-b-0">
                      <div className="min-w-48 flex-1">
                        <p className="font-medium">{component.name}</p>
                        <p className="text-[13.5px] text-ink-2">{t(component.description)}</p>
                      </div>
                      {component.installed ? (
                        <>
                          <Mono className="text-ink-2">{component.version}</Mono>
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
                          {component.id.startsWith("python-") && (
                            <IconButton
                              label={t("{name} sürümünü kaldır", { name: component.name })}
                              onClick={() => void removeVersion(component)}
                            >
                              <Trash2 className="size-4" />
                            </IconButton>
                          )}
                        </>
                      ) : (
                        <Button
                          size="sm"
                          icon={<Download className="size-3.5" />}
                          loading={install.isPending && install.variables?.id === component.id}
                          onClick={() => install.mutate(component)}
                        >
                          {t("Kur")}
                        </Button>
                      )}
                    </li>
                  ))}
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
