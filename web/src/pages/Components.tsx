import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, LoadingBlock, Modal, Mono, PageHeader, Panel, Status } from "../components/ui";
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
      <div className="flex flex-col gap-4">
        {components.data &&
          categories.map((category) => {
            const items = components.data.filter((component) => component.category === category.id);
            if (items.length === 0) return null;
            return (
              <Panel key={category.id} flush title={t(category.title)}>
                <ul>
                  {items.map((component) => (
                    <li key={component.id} className="flex flex-wrap items-center gap-x-6 gap-y-2 border-b border-line px-4 py-3 last:border-b-0">
                      <div className="min-w-48 flex-1">
                        <p className="font-medium">{component.name}</p>
                        <p className="text-[13px] text-ink-2">{t(component.description)}</p>
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
