// Veritabanı silme onayı ("önce yedek al" seçeneğiyle) ve silinenlerin 7 günlük çöp kutusu.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcw, Trash2 } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, Checkbox, ErrorBlock, IconButton, Input, Modal, Mono, Panel, Table, Td, Th } from "../components/ui";
import type { DatabaseInfo, DatabaseTrashItem } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

const TRASH_KEY = ["database-trash"];

/** Silme onayı: kalıcı silmeden önce dökümü alıp 7 gün saklama seçeneği. */
export function DropDatabaseModal({ database, onClose }: { database: DatabaseInfo; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [backup, setBackup] = useState(true);
  const [typed, setTyped] = useState("");

  const drop = useMutation({
    mutationFn: () => api.delete<{ trash_id: number | null }>(`/databases/${database.id}${backup ? "?backup=true" : ""}`),
    onSuccess: (result) => {
      feedback.success(
        result.trash_id === null
          ? t("{name} silindi", { name: database.name })
          : t("{name} silindi; 7 gün boyunca geri getirilebilir", { name: database.name }),
      );
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
      void queryClient.invalidateQueries({ queryKey: TRASH_KEY });
      onClose();
    },
    onError: (error) => feedback.error(error),
  });

  return (
    <Modal
      open
      onClose={drop.isPending ? () => undefined : onClose}
      title={t("{name} veritabanı silinsin mi?", { name: database.name })}
      footer={
        <>
          <Button disabled={drop.isPending} onClick={onClose}>
            {t("Vazgeç")}
          </Button>
          <Button variant="danger" disabled={typed !== database.name} loading={drop.isPending} onClick={() => drop.mutate()}>
            {t("Veritabanını sil")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <p className="text-ink-2">
          {backup
            ? t("Bütün tablolar ve veriler silinir. Silmeden önce veritabanının dökümü alınır ve 7 gün boyunca “Silinen veritabanları” listesinden geri getirilebilir.")
            : t("Bütün tablolar ve veriler kalıcı olarak silinir. Bu işlem geri alınamaz.")}
        </p>
        <Checkbox label={t("Silmeden önce yedeğini al (7 gün boyunca geri getirilebilir)")} checked={backup} onChange={(event) => setBackup(event.target.checked)} />
        <Input
          label={t("Onaylamak için veritabanının adını yazın: {name}", { name: database.name })}
          value={typed}
          autoComplete="off"
          onChange={(event) => setTyped(event.target.value)}
        />
        {drop.isPending && backup && <p className="text-[13.5px] text-ink-3">{t("Yedek alınıyor; büyük veritabanlarında birkaç dakika sürebilir.")}</p>}
      </div>
    </Modal>
  );
}

/** Silinmiş ama dökümü saklanan veritabanları. Liste boşsa hiçbir şey gösterilmez. */
export function TrashPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [jobId, setJobId] = useState<number | null>(null);
  const trash = useQuery({ queryKey: TRASH_KEY, queryFn: () => api.get<DatabaseTrashItem[]>("/databases/trash") });

  const restore = useMutation({
    mutationFn: (item: DatabaseTrashItem) => api.post<{ job_id: number }>(`/databases/trash/${item.id}/restore`),
    onSuccess: (result) => setJobId(result.job_id),
    onError: (error) => feedback.error(error),
  });
  const purge = useMutation({
    mutationFn: (item: DatabaseTrashItem) => api.delete(`/databases/trash/${item.id}`),
    onSuccess: () => {
      feedback.success(t("Döküm silindi"));
      void queryClient.invalidateQueries({ queryKey: TRASH_KEY });
    },
    onError: (error) => feedback.error(error),
  });

  async function askPurge(item: DatabaseTrashItem) {
    const confirmed = await feedback.confirm({
      title: t("{name} dökümü kalıcı olarak silinsin mi?", { name: item.name }),
      description: t("Bu veritabanı bir daha geri getirilemez."),
      confirmLabel: t("Dökümü sil"),
      danger: true,
    });
    if (confirmed) purge.mutate(item);
  }

  if (trash.isError) return <ErrorBlock message={errorMessage(trash.error)} onRetry={() => void trash.refetch()} />;
  if (!trash.data || trash.data.length === 0) return null;

  return (
    <>
      <Panel flush title={t("Silinen veritabanları")} actions={<span className="text-[13.5px] text-ink-3">{t("Dökümler 7 gün saklanır")}</span>}>
        <Table>
          <thead>
            <tr>
              <Th>{t("Ad")}</Th>
              <Th>{t("Silinme")}</Th>
              <Th>{t("Saklama süresi")}</Th>
              <Th align="right">{t("Boyut")}</Th>
              <Th align="right">{t("İşlemler")}</Th>
            </tr>
          </thead>
          <tbody>
            {trash.data.map((item) => (
              <tr key={item.id} className="hover:bg-sunken/50">
                <Td>
                  <Mono className="font-medium">{item.name}</Mono>
                  <p className="text-[13px] text-ink-3">
                    {item.engine === "postgresql" ? "PostgreSQL" : "MariaDB"}
                    {item.owner && ` · ${item.owner}`}
                  </p>
                </Td>
                <Td className="whitespace-nowrap text-ink-2">{formatDateTime(item.deleted_at)}</Td>
                <Td className="whitespace-nowrap text-ink-2">{t("{when} silinecek", { when: formatRelative(item.expires_at) })}</Td>
                <Td align="right" className="whitespace-nowrap text-ink-2">
                  {formatBytes(item.size)}
                </Td>
                <Td align="right">
                  <div className="flex justify-end gap-0.5">
                    <IconButton label={t("Geri getir")} disabled={restore.isPending} onClick={() => restore.mutate(item)}>
                      <RotateCcw className="size-4" />
                    </IconButton>
                    <IconButton label={t("Dökümü şimdi sil")} disabled={purge.isPending} onClick={() => void askPurge(item)}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  </div>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      </Panel>
      {jobId !== null && (
        <Modal
          open
          wide
          onClose={() => {
            setJobId(null);
            void queryClient.invalidateQueries({ queryKey: TRASH_KEY });
            void queryClient.invalidateQueries({ queryKey: ["databases"] });
          }}
          title={t("Veritabanı geri getiriliyor")}
        >
          <JobLog
            jobId={jobId}
            onDone={() => {
              void queryClient.invalidateQueries({ queryKey: TRASH_KEY });
              void queryClient.invalidateQueries({ queryKey: ["databases"] });
            }}
          />
        </Modal>
      )}
    </>
  );
}
