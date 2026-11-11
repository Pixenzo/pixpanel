// Hesabın kendi yedekleri: müşteri kendi sitelerinin, veritabanlarının ve postasının
// yedeklerini görür, elle yedek alır, yerine geri yükler ve kendi aldığı yedeği siler.
// Yedek yerlerini ve düzenli planları yönetici tanımlar.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Database, Globe, Mail, Plus, RotateCcw, Trash2 } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, Checkbox, EmptyState, ErrorBlock, IconButton, LoadingBlock, Modal, Mono, PageHeader, Panel, Table, Td, Th } from "../components/ui";
import type { MyBackups, MySnapshot } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

const kindView: Record<string, { icon: LucideIcon; label: string }> = {
  site: { icon: Globe, label: "Site" },
  db: { icon: Database, label: "Veritabanı" },
  mail: { icon: Mail, label: "Posta" },
};

function SourceList({ title, items, selected, onToggle }: { title: string; items: { id: number; name: string }[]; selected: number[]; onToggle: (id: number, checked: boolean) => void }) {
  if (items.length === 0) return null;
  return (
    <fieldset>
      <legend className="mb-2 text-[13px] font-medium text-ink-2">{title}</legend>
      <div className="flex flex-col gap-2">
        {items.map((item) => (
          <Checkbox key={item.id} label={item.name} checked={selected.includes(item.id)} onChange={(event) => onToggle(item.id, event.target.checked)} />
        ))}
      </div>
    </fieldset>
  );
}

function RunModal({ data, onClose }: { data: MyBackups; onClose: () => void }) {
  const feedback = useFeedback();
  // Varsayılan: her şey seçili; çoğu kişi "hepsinin yedeğini al" demek ister.
  const [sites, setSites] = useState(() => data.sites.map((item) => item.id));
  const [databases, setDatabases] = useState(() => data.databases.map((item) => item.id));
  const [mail, setMail] = useState(() => data.mail_domains.map((item) => item.id));
  const toggle = (set: (update: (current: number[]) => number[]) => void) => (id: number, checked: boolean) =>
    set((current) => (checked ? [...current, id] : current.filter((item) => item !== id)));

  const run = useMutation({
    mutationFn: () => api.post<{ job_id: number }>("/backup/mine/run", { site_ids: sites, database_ids: databases, mail_domain_ids: mail }),
    onSuccess: () => {
      feedback.success(t("Yedekleme başladı; bitince haber verilecek."));
      onClose();
    },
  });
  const count = sites.length + databases.length + mail.length;

  function submit(event: FormEvent) {
    event.preventDefault();
    run.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("Yedek al")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="my-backup" loading={run.isPending} disabled={count === 0}>
            {t("Yedeği al")}
          </Button>
        </>
      }
    >
      <form id="my-backup" onSubmit={submit} className="flex flex-col gap-5">
        <SourceList title={t("Siteler")} items={data.sites} selected={sites} onToggle={toggle(setSites)} />
        <SourceList title={t("Veritabanları")} items={data.databases} selected={databases} onToggle={toggle(setDatabases)} />
        <SourceList title={t("Posta")} items={data.mail_domains} selected={mail} onToggle={toggle(setMail)} />
        <p className="text-[13px] text-ink-2">
          {t("Elle aldığınız yedekler siz silene kadar saklanır; en çok {limit} tane saklayabilirsiniz ({count} tane var).", {
            limit: data.manual_limit,
            count: data.manual_count,
          })}
        </p>
        {run.isError && <ErrorBlock message={errorMessage(run.error)} />}
      </form>
    </Modal>
  );
}

function RestoreModal({ snapshot, onClose }: { snapshot: MySnapshot; onClose: () => void }) {
  const feedback = useFeedback();
  const [deleteExtra, setDeleteExtra] = useState(false);
  const restore = useMutation({
    mutationFn: () => api.post<{ job_id: number }>("/backup/mine/restore", { target_id: snapshot.target_id, snapshot: snapshot.id, delete_extra: deleteExtra }),
    onSuccess: () => {
      feedback.success(t("Geri yükleme başladı; bitince haber verilecek."));
      onClose();
    },
  });
  const what = snapshot.kind === "site" ? t("sitenin dosyaları") : snapshot.kind === "db" ? t("veritabanının içeriği") : t("posta kutuları");

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{name} geri yüklensin mi?", { name: snapshot.name })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="danger" loading={restore.isPending} onClick={() => restore.mutate()}>
            {t("Geri yükle")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <p className="text-ink-2">
          {t("{when} alınan yedekteki {what} şimdikinin üzerine yazılır. Yedekten sonra yapılan değişiklikler kaybolur.", {
            when: formatDateTime(snapshot.time),
            what,
          })}
        </p>
        {snapshot.kind !== "db" && (
          <Checkbox
            label={t("Yedekte olmayan dosyaları sil")}
            hint={t("İşaretlemezseniz yedekten sonra eklenen dosyalar yerinde kalır.")}
            checked={deleteExtra}
            onChange={(event) => setDeleteExtra(event.target.checked)}
          />
        )}
        {restore.isError && <ErrorBlock message={errorMessage(restore.error)} />}
      </div>
    </Modal>
  );
}

export function MyBackupsPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [running, setRunning] = useState(false);
  const [restoring, setRestoring] = useState<MySnapshot | null>(null);
  // Yedekleme ve geri yükleme görevleri bitince canlı akış bu sorguyu kendiliğinden yeniler.
  const backups = useQuery({ queryKey: ["my-backups"], queryFn: () => api.get<MyBackups>("/backup/mine") });

  const remove = useMutation({
    mutationFn: (snapshot: MySnapshot) => api.delete(`/backup/mine/snapshots/${snapshot.target_id}/${snapshot.id}`),
    onSuccess: () => feedback.success(t("Yedek siliniyor.")),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["my-backups"] }),
  });

  async function confirmRemove(snapshot: MySnapshot) {
    const confirmed = await feedback.confirm({
      title: t("Yedek silinsin mi?"),
      description: t("{name} için {when} alınan yedek kalıcı olarak silinir.", { name: snapshot.name, when: formatDateTime(snapshot.time) }),
      confirmLabel: t("Yedeği sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(snapshot);
  }

  if (backups.isPending) return <LoadingBlock />;
  if (backups.isError) return <ErrorBlock message={errorMessage(backups.error)} onRetry={() => void backups.refetch()} />;
  const data = backups.data;
  const hasSources = data.sites.length + data.databases.length + data.mail_domains.length > 0;

  return (
    <>
      <PageHeader
        title={t("Yedekler")}
        description={t("Sitelerinizin, veritabanlarınızın ve postanızın yedekleri. Düzenli yedekleri sunucu yöneticisi alır; siz de istediğiniz an elle yedek alıp geri yükleyebilirsiniz.")}
        actions={
          data.available && hasSources ? (
            <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setRunning(true)}>
              {t("Yedek al")}
            </Button>
          ) : undefined
        }
      />
      {!data.available ? (
        <Panel>
          <p className="font-medium">{t("Bu sunucuda yedekleme henüz kurulmamış")}</p>
          <p className="mt-1 text-ink-2">{t("Yedek alabilmeniz için sunucu yöneticisinin bir yedek yeri tanımlaması gerekir.")}</p>
        </Panel>
      ) : (
        <Panel flush>
          {data.snapshots.length === 0 ? (
            <EmptyState
              title={t("Henüz yedek yok")}
              description={hasSources ? t("İlk yedeğinizi şimdi alabilirsiniz; büyük değişikliklerden önce yedek almak iyi bir alışkanlıktır.") : t("Yedeklenecek bir siteniz, veritabanınız ya da posta alan adınız yok.")}
              action={
                hasSources ? (
                  <Button variant="primary" onClick={() => setRunning(true)}>
                    {t("Yedek al")}
                  </Button>
                ) : undefined
              }
            />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Ne yedeklendi")}</Th>
                  <Th>{t("Tür")}</Th>
                  <Th>{t("Alındığı an")}</Th>
                  <Th align="right">{t("Boyut")}</Th>
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {data.snapshots.map((snapshot) => {
                  const view = kindView[snapshot.kind] ?? kindView.site;
                  return (
                    <tr key={`${snapshot.target_id}-${snapshot.id}`} className="hover:bg-sunken/50">
                      <Td>
                        <span className="inline-flex items-center gap-2">
                          <view.icon aria-hidden className="size-4 shrink-0 text-ink-3" />
                          <Mono className="font-medium">{snapshot.name}</Mono>
                        </span>
                      </Td>
                      <Td>
                        <Badge>{t(view.label)}</Badge>
                        <span className="ml-2 text-[12.5px] text-ink-3">{snapshot.manual ? t("elle alındı") : t("düzenli yedek")}</span>
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2" >
                        <span title={formatDateTime(snapshot.time)}>{formatRelative(snapshot.time)}</span>
                      </Td>
                      <Td align="right" className="whitespace-nowrap">
                        {snapshot.size === null ? "" : formatBytes(snapshot.size)}
                      </Td>
                      <Td align="right" className="py-0.5">
                        <div className="flex flex-wrap items-center justify-end gap-1">
                          <Button size="sm" icon={<RotateCcw className="size-3.5" />} onClick={() => setRestoring(snapshot)}>
                            {t("Geri yükle")}
                          </Button>
                          {snapshot.manual && (
                            <IconButton label={t("Yedeği sil")} onClick={() => void confirmRemove(snapshot)}>
                              <Trash2 className="size-4" />
                            </IconButton>
                          )}
                        </div>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
      {running && <RunModal data={data} onClose={() => setRunning(false)} />}
      {restoring && <RestoreModal snapshot={restoring} onClose={() => setRestoring(null)} />}
    </>
  );
}
