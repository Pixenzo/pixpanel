// Posta → Karantina sekmesi: reddetme eşiğini aşan iletiler reddedilmek yerine tutulur
// (Postfix bekletme kuyruğu). Yönetici hepsini, diğer hesaplar yalnızca kendi alan adlarına
// gelenleri görür; her ileti bırakılabilir (alıcıya teslim edilir) ya da silinebilir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Eye, RefreshCw, Send, Trash2 } from "lucide-react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, LoadingBlock, Modal, Mono, Panel, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

interface QuarantineMessage {
  id: string;
  arrived_at: number;
  size: number;
  sender: string;
  recipients: string[];
  subject: string;
  /** Rspamd puanı; bilinmiyorsa boş. */
  score: number | null;
}

interface QuarantineList {
  /** Sunucu geneli kip: `reject` ya da `quarantine`. */
  mode: "reject" | "quarantine";
  retention_days: number;
  messages: QuarantineMessage[];
  truncated: boolean;
}

interface QuarantineView {
  id: string;
  sender: string;
  recipients: string[];
  size: number;
  arrived_at: number;
  raw: string;
  truncated: boolean;
}

function MessageModal({ id, onClose }: { id: string; onClose: () => void }) {
  const view = useQuery({ queryKey: ["mail-quarantine", id], queryFn: () => api.get<QuarantineView>(`/mail/quarantine/${encodeURIComponent(id)}`) });
  return (
    <Modal open onClose={onClose} title={t("İleti içeriği")} description={t("Başlıklar ve gövdenin ilk 64 KB'ı ham metin olarak gösterilir.")} extraWide footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      {view.isPending && <LoadingBlock />}
      {view.isError && <ErrorBlock message={errorMessage(view.error)} onRetry={() => void view.refetch()} />}
      {view.data && (
        <div className="flex flex-col gap-3">
          <pre className="max-h-[60vh] overflow-auto rounded-md border border-line bg-sunken/50 p-3 font-mono text-[12.5px] leading-relaxed whitespace-pre-wrap break-all">{view.data.raw}</pre>
          {view.data.truncated && <p className="text-[13px] text-ink-3">{t("İletinin tamamı gösterilmiyor; ilk 64 KB'ı okundu.")}</p>}
        </div>
      )}
    </Modal>
  );
}

export function MailQuarantineTab() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [viewing, setViewing] = useState<string | null>(null);
  const list = useQuery({ queryKey: ["mail-quarantine"], queryFn: () => api.get<QuarantineList>("/mail/quarantine"), refetchInterval: 30000 });

  const refresh = () => void queryClient.invalidateQueries({ queryKey: ["mail-quarantine"] });

  const release = useMutation({
    mutationFn: (id: string) => api.post(`/mail/quarantine/${encodeURIComponent(id)}/release`),
    onSuccess: () => {
      feedback.success(t("İleti bırakıldı; alıcıya teslim ediliyor"));
      refresh();
    },
    onError: (error) => feedback.error(error),
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/mail/quarantine/${encodeURIComponent(id)}`),
    onSuccess: (_, id) => {
      feedback.success(t("İleti silindi"));
      setSelected((current) => {
        const next = new Set(current);
        next.delete(id);
        return next;
      });
      refresh();
    },
    onError: (error) => feedback.error(error),
  });

  const removeMany = useMutation({
    mutationFn: (ids: string[]) => api.post<{ deleted: number; skipped: number }>("/mail/quarantine/bulk-delete", { ids }),
    onSuccess: (result) => {
      feedback.success(t("{count} ileti silindi", { count: result.deleted }));
      setSelected(new Set());
      refresh();
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRelease(message: QuarantineMessage) {
    const confirmed = await feedback.confirm({
      title: t("İleti bırakılsın mı?"),
      description: t("{sender} adresinden gelen ileti, istenmeyen posta olarak yakalanmıştı. Bırakırsanız alıcıların gelen kutusuna teslim edilir.", {
        sender: message.sender || t("Boş gönderen"),
      }),
      confirmLabel: t("İletiyi bırak"),
    });
    if (confirmed) release.mutate(message.id);
  }

  async function confirmRemove(message: QuarantineMessage) {
    const confirmed = await feedback.confirm({
      title: t("İleti silinsin mi?"),
      description: t("{sender} adresinden gelen ileti kalıcı olarak silinir. Bu işlem geri alınamaz.", { sender: message.sender || t("Boş gönderen") }),
      confirmLabel: t("İletiyi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(message.id);
  }

  async function confirmRemoveSelected() {
    const ids = [...selected];
    const confirmed = await feedback.confirm({
      title: t("Seçilen iletiler silinsin mi?"),
      description: t("{count} ileti kalıcı olarak silinir. Bu işlem geri alınamaz.", { count: ids.length }),
      confirmLabel: t("İletileri sil"),
      danger: true,
    });
    if (confirmed) removeMany.mutate(ids);
  }

  if (list.isPending) return <LoadingBlock />;
  if (list.isError) return <ErrorBlock message={errorMessage(list.error)} onRetry={() => void list.refetch()} />;

  const messages = list.data.messages;
  const allSelected = messages.length > 0 && messages.every((message) => selected.has(message.id));
  const busy = release.isPending || remove.isPending || removeMany.isPending;

  function toggleAll(checked: boolean) {
    setSelected(checked ? new Set(messages.map((message) => message.id)) : new Set());
  }

  function toggle(id: string, checked: boolean) {
    setSelected((current) => {
      const next = new Set(current);
      if (checked) next.add(id);
      else next.delete(id);
      return next;
    });
  }

  return (
    <>
      <Panel
        flush
        title={t("Karantina")}
        actions={
          <>
            <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={list.isFetching} onClick={() => void list.refetch()}>
              {t("Yenile")}
            </Button>
            <Button size="sm" variant="danger" icon={<Trash2 className="size-3.5" />} disabled={selected.size === 0 || busy} onClick={() => void confirmRemoveSelected()}>
              {t("Seçilenleri sil")}
            </Button>
          </>
        }
      >
        <p className="border-b border-line px-4 sm:px-6 py-2.5 text-[13.5px] text-ink-2">
          {list.data.mode === "quarantine"
            ? t("Reddetme eşiğini aşan iletiler reddedilmeyip burada tutulur; alıcıya teslim edilmez. {days} günden eski iletiler kendiliğinden silinir.", { days: list.data.retention_days })
            : t("Karantina kipi kapalı: reddetme eşiğini aşan iletiler SMTP'de reddediliyor. Kipi yönetici İstenmeyen posta sekmesinden açabilir; burada yalnızca daha önce tutulan iletiler görünür.")}
        </p>
        {messages.length === 0 && <EmptyState title={t("Karantinada ileti yok.")} />}
        {messages.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th className="w-10">
                  <input type="checkbox" aria-label={t("Hepsini seç")} className="size-4 accent-(--accent)" checked={allSelected} onChange={(event) => toggleAll(event.target.checked)} />
                </Th>
                <Th>{t("Gönderen")}</Th>
                <Th>{t("Alıcılar")}</Th>
                <Th>{t("Konu")}</Th>
                <Th align="right">{t("Puan")}</Th>
                <Th align="right">{t("Boyut")}</Th>
                <Th>{t("Tarih")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {messages.map((message) => (
                <tr key={message.id} className="align-top hover:bg-sunken/50">
                  <Td>
                    <input
                      type="checkbox"
                      aria-label={t("İletiyi seç")}
                      className="size-4 accent-(--accent)"
                      checked={selected.has(message.id)}
                      onChange={(event) => toggle(message.id, event.target.checked)}
                    />
                  </Td>
                  <Td className="max-w-56">
                    <p className="truncate">
                      <Mono>{message.sender || t("(boş gönderen)")}</Mono>
                    </p>
                  </Td>
                  <Td className="max-w-64">
                    <ul className="flex flex-col gap-1">
                      {message.recipients.slice(0, 3).map((recipient) => (
                        <li key={recipient} className="truncate">
                          <Mono>{recipient}</Mono>
                        </li>
                      ))}
                      {message.recipients.length > 3 && <li className="text-[13px] text-ink-3">{t("+{count} alıcı daha", { count: message.recipients.length - 3 })}</li>}
                    </ul>
                  </Td>
                  <Td className="max-w-72">
                    <p className="truncate">{message.subject || <span className="text-ink-3">{t("(konu yok)")}</span>}</p>
                  </Td>
                  <Td align="right" className="whitespace-nowrap tabular">
                    {message.score === null ? <span className="text-ink-3">—</span> : <Badge tone="bad">{message.score.toFixed(1)}</Badge>}
                  </Td>
                  <Td align="right" className="whitespace-nowrap tabular">
                    {formatBytes(message.size)}
                  </Td>
                  <Td className="whitespace-nowrap text-ink-2">
                    <span title={formatDateTime(message.arrived_at)}>{t(formatRelative(message.arrived_at))}</span>
                  </Td>
                  <Td align="right" className="whitespace-nowrap">
                    <IconButton label={t("İçeriği göster")} onClick={() => setViewing(message.id)}>
                      <Eye className="size-4" />
                    </IconButton>
                    <IconButton label={t("İletiyi bırak")} disabled={busy} onClick={() => void confirmRelease(message)}>
                      <Send className="size-4" />
                    </IconButton>
                    <IconButton label={t("İletiyi sil")} disabled={busy} onClick={() => void confirmRemove(message)}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
        {list.data.truncated && <p className="border-t border-line px-4 sm:px-6 py-2.5 text-[13px] text-ink-3">{t("Yalnızca en yeni 300 ileti gösteriliyor.")}</p>}
      </Panel>
      {viewing && <MessageModal id={viewing} onClose={() => setViewing(null)} />}
    </>
  );
}
