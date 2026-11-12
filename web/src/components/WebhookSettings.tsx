// Web kancaları: panel olaylarının hesabın kendi adreslerine HTTP POST ile iletilmesi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, History, Pencil, Send, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Badge, Button, Checkbox, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Status, Table, Td, Th } from "./ui";

interface Delivery {
  id: number;
  event: string;
  attempt: number;
  status: number | null;
  duration_ms: number;
  error: string;
  at: number;
}

interface Hook {
  id: number;
  url: string;
  /** `["*"]` tüm olaylar demektir. */
  events: string[];
  enabled: boolean;
  created_at: number;
  last_delivery: Delivery | null;
}

interface WebhookIndex {
  hooks: Hook[];
  available_events: { id: string; label: string }[];
  max_hooks: number;
  private_allowed: boolean;
}

const ALL = "*";
const queryKey = ["webhooks"];

function deliveryOk(delivery: Delivery): boolean {
  return delivery.status !== null && delivery.status >= 200 && delivery.status < 300 && delivery.error === "";
}

/** Gizli anahtar yalnızca oluşturulurken ve yenilenirken gösterilir; kopyalama düğmesiyle. */
function SecretModal({ secret, onClose }: { secret: string; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    try {
      await navigator.clipboard.writeText(secret);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Pano kapalıysa metin elle seçilip kopyalanabilir.
    }
  }
  return (
    <Modal
      open
      onClose={onClose}
      title={t("Gizli anahtarınız")}
      description={t("Bu anahtar yalnızca şimdi gösterilir. Kaydedin; kaybederseniz yenisini üretmeniz gerekir.")}
      wide
      footer={
        <Button variant="primary" onClick={onClose}>
          {t("Kaydettim")}
        </Button>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="flex items-start gap-2">
          <code className="min-w-0 flex-1 rounded-md border border-line bg-sunken/60 px-3 py-2 font-mono text-[13px] break-all select-all">{secret}</code>
          <IconButton label={copied ? t("Kopyalandı") : t("Kopyala")} onClick={() => void copy()} className="mt-0.5">
            {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
          </IconButton>
        </div>
        <p className="max-w-[70ch] text-[13.5px] text-ink-2">
          {t("Her istekte {header} başlığı bulunur: gövdenin bu anahtarla hesaplanan HMAC-SHA256 değeri (sha256=...). Alıcınız bu imzayı doğrulayarak isteğin panelden geldiğinden emin olabilir.", {
            header: "X-PixPanel-Signature",
          })}
        </p>
      </div>
    </Modal>
  );
}

function HistoryModal({ hook, onClose }: { hook: Hook; onClose: () => void }) {
  const history = useQuery({
    queryKey: ["webhook-deliveries", hook.id],
    queryFn: () => api.get<Delivery[]>(`/webhooks/${hook.id}/deliveries`),
  });
  return (
    <Modal open onClose={onClose} title={t("Son gönderimler")} description={hook.url} extraWide>
      {history.isPending && <LoadingBlock />}
      {history.isError && <ErrorBlock message={errorMessage(history.error)} onRetry={() => void history.refetch()} />}
      {history.data && history.data.length === 0 && <EmptyState title={t("Henüz gönderim yapılmadı")} />}
      {history.data && history.data.length > 0 && (
        <Table>
          <thead>
            <tr>
              <Th>{t("Zaman")}</Th>
              <Th>{t("Olay")}</Th>
              <Th>{t("Deneme")}</Th>
              <Th>{t("Sonuç")}</Th>
              <Th>{t("Süre")}</Th>
              <Th>{t("Hata")}</Th>
            </tr>
          </thead>
          <tbody>
            {history.data.map((delivery) => (
              <tr key={delivery.id}>
                <Td>{formatDateTime(delivery.at)}</Td>
                <Td>
                  <Mono>{delivery.event}</Mono>
                </Td>
                <Td>{delivery.attempt}</Td>
                <Td>
                  <Status tone={deliveryOk(delivery) ? "ok" : "bad"}>{delivery.status !== null ? delivery.status : t("Ulaşılamadı")}</Status>
                </Td>
                <Td>{delivery.duration_ms} ms</Td>
                <Td className="max-w-[28ch] break-words text-ink-2">{delivery.error || "—"}</Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </Modal>
  );
}

function HookForm({ index, hook, onClose, onSecret }: { index: WebhookIndex; hook: Hook | null; onClose: () => void; onSecret: (secret: string) => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [url, setUrl] = useState(hook?.url ?? "");
  const [all, setAll] = useState(hook ? hook.events.includes(ALL) : true);
  const [events, setEvents] = useState<string[]>(hook && !hook.events.includes(ALL) ? hook.events : []);
  const [enabled, setEnabled] = useState(hook?.enabled ?? true);

  const save = useMutation({
    mutationFn: async (): Promise<string | null> => {
      const body = { url, events: all ? [ALL] : events, enabled };
      if (hook) {
        await api.put<Hook>(`/webhooks/${hook.id}`, body);
        return null;
      }
      // Gizli anahtar yalnızca oluşturma yanıtında gelir.
      return (await api.post<{ hook: Hook; secret: string }>("/webhooks", body)).secret;
    },
    onSuccess: (secret) => {
      void queryClient.invalidateQueries({ queryKey });
      if (secret === null) feedback.success(t("Web kancası kaydedildi"));
      else onSecret(secret);
      onClose();
    },
  });

  function toggle(id: string, checked: boolean) {
    setEvents((current) => (checked ? [...current.filter((item) => item !== id), id] : current.filter((item) => item !== id)));
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  const canSave = url.trim() !== "" && (all || events.length > 0);

  return (
    <Modal open onClose={onClose} title={hook ? t("Web kancasını düzenle") : t("Web kancası ekle")} wide>
      <form onSubmit={submit} className="flex flex-col gap-5">
        <Input
          label={t("Adres")}
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          placeholder="https://ornek.com/pixpanel-kanca"
          hint={
            index.private_allowed
              ? t("http:// ya da https:// ile başlamalı. Özel ağ adreslerine yalnızca yönetici gönderebilir; yerel (loopback) adresler yasaktır.")
              : t("http:// ya da https:// ile başlamalı. Sunucunun iç ağındaki ve yerel adresler kullanılamaz.")
          }
          className="font-mono text-[13.5px]"
          autoCapitalize="none"
          spellCheck={false}
          required
        />
        <fieldset className="flex flex-col gap-3">
          <legend className="mb-2 font-semibold">{t("Hangi olaylar gönderilsin?")}</legend>
          <Checkbox label={t("Tüm olaylar")} checked={all} onChange={(event) => setAll(event.target.checked)} />
          {!all && (
            <div className="flex max-h-64 flex-col gap-2.5 overflow-y-auto border-l-2 border-line pl-4">
              {index.available_events.map((item) => (
                <Checkbox key={item.id} label={t(item.label)} hint={item.id} checked={events.includes(item.id)} onChange={(event) => toggle(item.id, event.target.checked)} />
              ))}
            </div>
          )}
        </fieldset>
        <Checkbox label={t("Etkin")} hint={t("Kapalıyken olaylar bu adrese gönderilmez.")} checked={enabled} onChange={(event) => setEnabled(event.target.checked)} />
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div className="flex justify-end gap-2.5">
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button type="submit" variant="primary" loading={save.isPending} disabled={!canSave}>
            {hook ? t("Değişiklikleri kaydet") : t("Kancayı ekle")}
          </Button>
        </div>
      </form>
    </Modal>
  );
}

function eventSummary(hook: Hook): string {
  if (hook.events.includes(ALL)) return t("Tüm olaylar");
  return t("{n} olay", { n: hook.events.length });
}

/** Hesabım sayfasına gömülen, kendi kendine yeten web kancaları bölümü. */
export function WebhookSettings() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<Hook | "new" | null>(null);
  const [history, setHistory] = useState<Hook | null>(null);
  const [secret, setSecret] = useState<string | null>(null);

  const index = useQuery({ queryKey, queryFn: () => api.get<WebhookIndex>("/webhooks") });

  const test = useMutation({
    mutationFn: (hook: Hook) => api.post<Delivery>(`/webhooks/${hook.id}/test`),
    onSuccess: (delivery) => {
      void queryClient.invalidateQueries({ queryKey });
      void queryClient.invalidateQueries({ queryKey: ["webhook-deliveries"] });
      if (deliveryOk(delivery)) feedback.success(t("Sınama gönderimi ulaştı ({status})", { status: delivery.status ?? 0 }));
      else feedback.error(new Error(delivery.error || t("Sınama gönderimi başarısız oldu")));
    },
    onError: (error) => feedback.error(error),
  });

  const rotate = useMutation({
    mutationFn: (hook: Hook) => api.post<{ secret: string }>(`/webhooks/${hook.id}/secret`),
    onSuccess: (result) => setSecret(result.secret),
    onError: (error) => feedback.error(error),
  });

  const toggle = useMutation({
    mutationFn: (hook: Hook) => api.put<Hook>(`/webhooks/${hook.id}`, { url: hook.url, events: hook.events, enabled: !hook.enabled }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey }),
    onError: (error) => feedback.error(error),
  });

  const remove = useMutation({
    mutationFn: (hook: Hook) => api.delete(`/webhooks/${hook.id}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey });
      feedback.success(t("Web kancası silindi"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(hook: Hook) {
    const confirmed = await feedback.confirm({
      title: t("Web kancası silinsin mi?"),
      description: t("Bu adrese bir daha olay gönderilmez ve gönderim geçmişi silinir."),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(hook);
  }

  async function confirmRotate(hook: Hook) {
    const confirmed = await feedback.confirm({
      title: t("Gizli anahtar yenilensin mi?"),
      description: t("Eski anahtarla hesaplanan imzalar geçersiz olur; alıcınızdaki anahtarı yeni anahtarla güncellemeniz gerekir."),
      confirmLabel: t("Anahtarı yenile"),
      danger: true,
    });
    if (confirmed) rotate.mutate(hook);
  }

  const data = index.data;
  const full = data !== undefined && data.hooks.length >= data.max_hooks;

  return (
    <Panel
      title={t("Web kancaları")}
      actions={
        data && (
          <Button size="sm" variant="primary" disabled={full} onClick={() => setEditing("new")}>
            {t("Web kancası ekle")}
          </Button>
        )
      }
    >
      <p className="mb-5 max-w-[75ch] text-ink-2">
        {t(
          "Hesabınızda bir olay olduğunda (yayın, sertifika, yedek, görev, denetim kaydı...) seçtiğiniz adrese JSON gövdeli bir HTTP POST gönderilir. CI, Slack ya da Discord köprüsü gibi kendi otomasyonunuzu bağlayabilirsiniz. Her istek gizli anahtarınızla imzalanır.",
        )}
      </p>
      {index.isPending && <LoadingBlock />}
      {index.isError && <ErrorBlock message={errorMessage(index.error)} onRetry={() => void index.refetch()} />}
      {data && data.hooks.length === 0 && <EmptyState title={t("Henüz web kancası yok")} description={t("“Web kancası ekle” ile ilk adresinizi tanımlayın.")} />}
      {data && data.hooks.length > 0 && (
        <ul className="flex flex-col divide-y divide-line">
          {data.hooks.map((hook) => (
            <li key={hook.id} className="flex flex-col gap-3 py-4 first:pt-0 last:pb-0">
              <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
                <div className="min-w-0 flex-1">
                  <p className="truncate font-mono text-[13.5px] font-medium">{hook.url}</p>
                  <p className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13.5px] text-ink-2">
                    <Badge tone={hook.enabled ? "ok" : "idle"}>{hook.enabled ? t("Etkin") : t("Kapalı")}</Badge>
                    <span>{eventSummary(hook)}</span>
                    {hook.last_delivery ? (
                      <>
                        <Status tone={deliveryOk(hook.last_delivery) ? "ok" : "bad"}>
                          {hook.last_delivery.status !== null ? hook.last_delivery.status : t("Ulaşılamadı")}
                        </Status>
                        <span>{formatRelative(hook.last_delivery.at)}</span>
                      </>
                    ) : (
                      <span>{t("Henüz gönderim yapılmadı")}</span>
                    )}
                  </p>
                </div>
                <div className="flex flex-wrap items-center gap-2">
                  <Button size="sm" icon={<Send className="size-4" />} loading={test.isPending && test.variables?.id === hook.id} onClick={() => test.mutate(hook)}>
                    {t("Sınama gönderimi")}
                  </Button>
                  <Button size="sm" icon={<History className="size-4" />} onClick={() => setHistory(hook)}>
                    {t("Geçmiş")}
                  </Button>
                  <Button size="sm" loading={toggle.isPending && toggle.variables?.id === hook.id} onClick={() => toggle.mutate(hook)}>
                    {hook.enabled ? t("Kapat") : t("Etkinleştir")}
                  </Button>
                  <Button size="sm" loading={rotate.isPending && rotate.variables?.id === hook.id} onClick={() => void confirmRotate(hook)}>
                    {t("Anahtarı yenile")}
                  </Button>
                  <IconButton label={t("Düzenle")} onClick={() => setEditing(hook)}>
                    <Pencil className="size-4" />
                  </IconButton>
                  <IconButton label={t("Sil")} onClick={() => void confirmRemove(hook)}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </div>
              </div>
              {hook.last_delivery && !deliveryOk(hook.last_delivery) && hook.last_delivery.error !== "" && (
                <p className="rounded-md bg-bad-soft px-3 py-2 text-[13.5px] text-bad">{hook.last_delivery.error}</p>
              )}
            </li>
          ))}
        </ul>
      )}
      {data && (
        <p className="mt-5 text-[13px] text-ink-3">
          {t("Hesap başına en çok {max} web kancası tanımlanabilir. Başarısız gönderim artan aralıkla en çok 3 kez denenir; son 20 gönderimin sonucu saklanır.", {
            max: data.max_hooks,
          })}
        </p>
      )}

      {data && editing !== null && (
        <HookForm
          key={editing === "new" ? "new" : editing.id}
          index={data}
          hook={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
          onSecret={setSecret}
        />
      )}
      {history && <HistoryModal hook={history} onClose={() => setHistory(null)} />}
      {secret && <SecretModal secret={secret} onClose={() => setSecret(null)} />}
    </Panel>
  );
}
