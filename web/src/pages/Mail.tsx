// Posta sayfası: alan adları listesi ile (yönetici için) kuyruk, istenmeyen posta
// ayarları ve etkinlik sekmeleri.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, RefreshCw, RotateCw, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Modal,
  Mono,
  PageHeader,
  Panel,
  Select,
  Status,
  Table,
  Td,
  Textarea,
  Th,
  Tabs,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

// ---- Türler (MailDomain sayfası da kullanır) ----

export interface MailDomain {
  id: number;
  user_id: number;
  owner: string;
  domain: string;
  catch_all: string;
  dkim_record: string;
  mailbox_count: number;
  alias_count: number;
  created_at: number;
}

export interface AutoReply {
  enabled: boolean;
  subject: string;
  body: string;
  /** "YYYY-MM-DD" ya da boş. */
  start_date: string;
  end_date: string;
}

export interface FilterCondition {
  field: "from" | "to" | "cc" | "subject" | "header" | "size_over";
  /** `field === "header"` iken başlık adı. */
  header: string;
  op: "contains" | "not_contains" | "is" | "matches";
  /** `size_over` için megabayt cinsinden sayı (dize). */
  value: string;
}

export interface FilterAction {
  kind: "move" | "copy" | "mark_read" | "flag" | "forward" | "discard";
  value: string;
}

export interface MailFilter {
  name: string;
  enabled: boolean;
  match_all: boolean;
  conditions: FilterCondition[];
  actions: FilterAction[];
  stop: boolean;
}

export interface Mailbox {
  id: number;
  domain_id: number;
  local: string;
  address: string;
  quota_mb: number;
  enabled: boolean;
  forward_to: string[];
  forward_keep: boolean;
  auto_reply: AutoReply;
  filters: MailFilter[];
  used_bytes: number | null;
  created_at: number;
}

export interface MailAlias {
  id: number;
  domain_id: number;
  local: string;
  address: string;
  destinations: string[];
  created_at: number;
}

export interface DnsRecord {
  kind: "MX" | "TXT";
  name: string;
  value: string;
  purpose: string;
  status: "ok" | "missing" | "mismatch" | "unknown";
  found: string;
}

export interface SpamSettings {
  reject_score: number;
  junk_score: number;
  greylisting: boolean;
  greylist_score: number;
  whitelist: string[];
  blacklist: string[];
}

interface MailService {
  name: string;
  label: string;
  running: boolean;
}

interface MailOverview {
  installed: boolean;
  services: MailService[];
  hostname: string;
  domains: MailDomain[];
  available_domains: string[];
  owners: { id: number; username: string }[];
}

interface QueueMessage {
  id: string;
  queue: string;
  arrived_at: number;
  size: number;
  sender: string;
  recipients: { address: string; reason: string | null }[];
}

interface MailSettings {
  hostname: string;
  spam: SpamSettings;
}

interface ActivityMessage {
  time: number;
  sender: string;
  recipients: string[] | null;
  subject: string;
  action: string;
  score: number;
  required_score: number;
  ip: string;
  size: number;
}

interface MailActivity {
  scanned: number | null;
  actions: Record<string, number> | null;
  learned: number | null;
  messages: ActivityMessage[];
}

// ---- Alan adı ekleme ----

function CreateDomainModal({ onClose, overview, isAdmin }: { onClose: () => void; overview: MailOverview; isAdmin: boolean }) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [domain, setDomain] = useState(isAdmin ? "" : (overview.available_domains[0] ?? ""));
  const [ownerId, setOwnerId] = useState(overview.owners.some((owner) => owner.id === me.id) ? me.id : (overview.owners[0]?.id ?? me.id));

  const create = useMutation({
    mutationFn: () =>
      api.post<MailDomain>("/mail/domains", {
        domain: domain.trim().toLowerCase(),
        user_id: overview.owners.length > 1 ? ownerId : undefined,
      }),
    onSuccess: (created) => {
      feedback.success(t("{domain} için posta etkinleştirildi", { domain: created.domain }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      onClose();
      void navigate(`/mail/${created.id}`);
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  const nothingToAdd = !isAdmin && overview.available_domains.length === 0;

  return (
    <Modal
      open
      onClose={onClose}
      title={t("Alan adı ekle")}
      description={t("Alan adı için posta kutuları ve takma adlar oluşturabilmeniz için posta etkinleştirilir.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-mail-domain" loading={create.isPending} disabled={nothingToAdd || !domain.trim()}>
            {t("Alan adını ekle")}
          </Button>
        </>
      }
    >
      <form id="create-mail-domain" onSubmit={submit} className="flex flex-col gap-4">
        {nothingToAdd ? (
          <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
            {t("Posta açabileceğiniz bir alan adı yok. Posta, hesabınızdaki sitelerin alan adlarıyla kullanılır;")}{" "}
            <Link to="/sites" className="font-medium underline">
              {t("önce Siteler sayfasından bir site ekleyin")}
            </Link>
            .
          </p>
        ) : isAdmin ? (
          <>
            <Input
              label={t("Alan adı")}
              value={domain}
              onChange={(event) => setDomain(event.target.value)}
              list="mail-available-domains"
              placeholder="ornek.com"
              className="font-mono text-[13px]"
              autoCapitalize="none"
              spellCheck={false}
              autoFocus
              required
            />
            <datalist id="mail-available-domains">
              {overview.available_domains.map((item) => (
                <option key={item} value={item} />
              ))}
            </datalist>
          </>
        ) : (
          <Select label={t("Alan adı")} value={domain} onChange={(event) => setDomain(event.target.value)} hint={t("Hesabınızdaki, henüz postası açılmamış siteler listelenir.")}>
            {overview.available_domains.map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </Select>
        )}
        {overview.owners.length > 1 && (
          <Select label={t("Sahibi")} value={ownerId} onChange={(event) => setOwnerId(Number(event.target.value))} hint={t("Alan adı ve kutuları bu hesaba ait olur.")}>
            {overview.owners.map((owner) => (
              <option key={owner.id} value={owner.id}>
                {owner.username}
              </option>
            ))}
          </Select>
        )}
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

// ---- Alan adları ----

function ServiceStrip({ services, hostname }: { services: MailService[]; hostname: string }) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-x-6 gap-y-1.5 rounded-lg border border-line bg-surface px-4 py-2.5">
      <ul aria-label={t("Posta servisleri")} className="flex flex-wrap items-center gap-x-5 gap-y-1">
        {services.map((service) => (
          <li key={service.name} className="flex items-center gap-2 text-[13px]">
            <span className="text-ink-2">{service.label}</span>
            <Status tone={service.running ? "ok" : "bad"}>{service.running ? t("çalışıyor") : t("durdu")}</Status>
          </li>
        ))}
      </ul>
      {hostname && (
        <p className="ml-auto text-[13px] text-ink-2">
          {t("Sunucu adı")}: <Mono>{hostname}</Mono>
        </p>
      )}
    </div>
  );
}

function DomainsTab({ overview, isAdmin, showOwner, onAdd }: { overview: MailOverview; isAdmin: boolean; showOwner: boolean; onAdd: () => void }) {
  return (
    <>
      {isAdmin && overview.services.length > 0 && <ServiceStrip services={overview.services} hostname={overview.hostname} />}
      <Panel flush>
        {overview.domains.length === 0 ? (
          <EmptyState
            title={t("Henüz posta alan adı yok")}
            description={t("İlk alan adınızı ekleyin; ardından posta kutularını oluşturur ve DNS kayıtlarını alan adı sağlayıcınıza girersiniz.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAdd}>
                {t("Alan adı ekle")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Alan adı")}</Th>
                <Th align="right">{t("Posta kutusu")}</Th>
                <Th align="right">{t("Takma ad")}</Th>
                {showOwner && <Th>{t("Sahibi")}</Th>}
                <Th>{t("Oluşturulma")}</Th>
              </tr>
            </thead>
            <tbody>
              {overview.domains.map((domain) => (
                <tr key={domain.id} className="hover:bg-sunken/50">
                  <Td>
                    <Link to={`/mail/${domain.id}`} className="font-mono text-[13.5px] font-medium hover:text-accent hover:underline">
                      {domain.domain}
                    </Link>
                    {domain.catch_all && (
                      <p className="text-[12px] text-ink-3">
                        {t("Hepsini yakala")}: <Mono>{domain.catch_all}</Mono>
                      </p>
                    )}
                  </Td>
                  <Td align="right" className="tabular">
                    {domain.mailbox_count}
                  </Td>
                  <Td align="right" className="tabular">
                    {domain.alias_count}
                  </Td>
                  {showOwner && (
                    <Td>
                      <Mono>{domain.owner}</Mono>
                    </Td>
                  )}
                  <Td className="whitespace-nowrap text-ink-2">{formatDateTime(domain.created_at)}</Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
    </>
  );
}

// ---- Kuyruk ----

function QueueTab() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const queue = useQuery({ queryKey: ["mail-queue"], queryFn: () => api.get<QueueMessage[]>("/mail/queue"), refetchInterval: 15000 });

  const flush = useMutation({
    mutationFn: () => api.post("/mail/queue/flush"),
    onSuccess: () => {
      feedback.success(t("Bekleyen iletiler yeniden denenmek üzere sıraya alındı"));
      void queryClient.invalidateQueries({ queryKey: ["mail-queue"] });
    },
    onError: (error) => feedback.error(error),
  });

  const remove = useMutation({
    mutationFn: (id: string) => api.delete(`/mail/queue/${encodeURIComponent(id)}`),
    onSuccess: () => {
      feedback.success(t("İleti kuyruktan silindi"));
      void queryClient.invalidateQueries({ queryKey: ["mail-queue"] });
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(message: QueueMessage) {
    const confirmed = await feedback.confirm({
      title: t("İleti kuyruktan silinsin mi?"),
      description: t("{sender} adresinden gelen ileti teslim edilmeden silinir. Bu işlem geri alınamaz.", { sender: message.sender || t("Boş gönderen") }),
      confirmLabel: t("İletiyi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(message.id);
  }

  const messages = queue.data ?? [];

  return (
    <Panel
      flush
      title={t("Bekleyen iletiler")}
      actions={
        <>
          <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={queue.isFetching} onClick={() => void queue.refetch()}>
            {t("Yenile")}
          </Button>
          <Button size="sm" icon={<RotateCw className="size-3.5" />} loading={flush.isPending} disabled={messages.length === 0} onClick={() => flush.mutate()}>
            {t("Şimdi yeniden dene")}
          </Button>
        </>
      }
    >
      <p className="border-b border-line px-4 py-2.5 text-[13px] text-ink-2">
        {t("Kuyrukta, henüz teslim edilemeyen iletiler bekler. Sistem bunları belirli aralıklarla kendiliğinden yeniden dener; liste 15 saniyede bir yenilenir.")}
      </p>
      {queue.isPending && <LoadingBlock />}
      {queue.isError && (
        <div className="p-4">
          <ErrorBlock message={errorMessage(queue.error)} onRetry={() => void queue.refetch()} />
        </div>
      )}
      {queue.data && messages.length === 0 && <EmptyState title={t("Kuyruk boş; bütün iletiler teslim edildi.")} />}
      {messages.length > 0 && (
        <Table>
          <thead>
            <tr>
              <Th>{t("Kimlik")}</Th>
              <Th>{t("Gönderen")}</Th>
              <Th>{t("Alıcılar")}</Th>
              <Th align="right">{t("Boyut")}</Th>
              <Th>{t("Geliş")}</Th>
              <Th align="right">
                <span className="sr-only">{t("İşlemler")}</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {messages.map((message) => (
              <tr key={message.id} className="align-top hover:bg-sunken/50">
                <Td className="whitespace-nowrap">
                  <Mono className="text-ink-2">{message.id}</Mono>
                </Td>
                <Td className="max-w-56">
                  <p className="truncate">
                    <Mono>{message.sender || t("(boş gönderen)")}</Mono>
                  </p>
                </Td>
                <Td className="max-w-96">
                  <ul className="flex flex-col gap-1">
                    {message.recipients.map((recipient) => (
                      <li key={recipient.address} className="min-w-0">
                        <p className="truncate">
                          <Mono>{recipient.address}</Mono>
                        </p>
                        {recipient.reason && <p className="truncate text-[12.5px] text-ink-3">{recipient.reason}</p>}
                      </li>
                    ))}
                  </ul>
                </Td>
                <Td align="right" className="whitespace-nowrap tabular">
                  {formatBytes(message.size)}
                </Td>
                <Td className="whitespace-nowrap text-ink-2">{t(formatRelative(message.arrived_at))}</Td>
                <Td align="right">
                  <IconButton label={t("İletiyi sil")} disabled={remove.isPending} onClick={() => void confirmRemove(message)}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </Panel>
  );
}

// ---- İstenmeyen posta ayarları ----

const linesToList = (text: string) =>
  text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

function SpamForm({ settings }: { settings: MailSettings }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [hostname, setHostname] = useState(settings.hostname);
  const [junkScore, setJunkScore] = useState(String(settings.spam.junk_score));
  const [rejectScore, setRejectScore] = useState(String(settings.spam.reject_score));
  const [greylisting, setGreylisting] = useState(settings.spam.greylisting);
  const [greylistScore, setGreylistScore] = useState(String(settings.spam.greylist_score));
  const [whitelist, setWhitelist] = useState(settings.spam.whitelist.join("\n"));
  const [blacklist, setBlacklist] = useState(settings.spam.blacklist.join("\n"));
  const [localError, setLocalError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (body: MailSettings) => api.put<MailSettings>("/mail/settings", body),
    onSuccess: (updated) => {
      queryClient.setQueryData(["mail-settings"], updated);
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(t("Posta ayarları kaydedildi"));
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    const scores = [junkScore, rejectScore, greylistScore].map((value) => Number(value));
    if ([junkScore, rejectScore, greylistScore].some((value) => value.trim() === "") || scores.some((value) => !Number.isFinite(value))) {
      setLocalError(t("Puan eşikleri sayı olmalıdır."));
      return;
    }
    const [junk, reject, greylist] = scores as [number, number, number];
    if (junk >= reject) {
      setLocalError(t("Junk eşiği, reddetme eşiğinden küçük olmalıdır; aksi hâlde iletiler Junk'a alınmadan reddedilir."));
      return;
    }
    setLocalError(null);
    save.mutate({
      hostname: hostname.trim(),
      spam: {
        junk_score: junk,
        reject_score: reject,
        greylisting,
        greylist_score: greylist,
        whitelist: linesToList(whitelist),
        blacklist: linesToList(blacklist),
      },
    });
  }

  return (
    <form onSubmit={submit} className="flex max-w-3xl flex-col gap-4">
      <Panel title={t("Posta sunucusu")}>
        <Input
          label={t("Posta sunucusu adı")}
          value={hostname}
          onChange={(event) => setHostname(event.target.value)}
          placeholder="mail.ornek.com"
          hint={t(
            "MX kayıtlarınız bu adı göstermelidir. Bu adla bir site açıp Let's Encrypt sertifikası alırsanız posta servisleri de o sertifikayı kullanır.",
          )}
          className="max-w-md font-mono text-[13px]"
          autoCapitalize="none"
          spellCheck={false}
          required
        />
      </Panel>

      <Panel title={t("Puanlama eşikleri")}>
        <div className="flex flex-col gap-4">
          <p className="max-w-[75ch] text-[13px] text-ink-2">
            {t("Her gelen ileti bir istenmeyen posta puanı alır; puan yükseldikçe ileti daha şüphelidir. Eşikler bu puanlara göre ne yapılacağını belirler.")}
          </p>
          <div className="grid gap-4 sm:grid-cols-3">
            <Input
              label={t("Junk klasörüne taşıma eşiği")}
              type="number"
              step={0.5}
              value={junkScore}
              onChange={(event) => setJunkScore(event.target.value)}
              hint={t("Puanı bu değere ulaşan ileti alıcının Junk klasörüne taşınır.")}
              required
            />
            <Input
              label={t("Reddetme eşiği")}
              type="number"
              step={0.5}
              value={rejectScore}
              onChange={(event) => setRejectScore(event.target.value)}
              hint={t("Puanı bu değere ulaşan ileti hiç teslim edilmez; gönderene reddedildiği bildirilir.")}
              required
            />
            <Input
              label={t("Gri liste eşiği")}
              type="number"
              step={0.5}
              value={greylistScore}
              onChange={(event) => setGreylistScore(event.target.value)}
              hint={t("Gri liste açıkken puanı bu değeri geçen ilk kez görülen gönderenler beklemeye alınır.")}
              disabled={!greylisting}
              required
            />
          </div>
          <Checkbox
            label={t("Gri liste kullan")}
            hint={t("İlk kez görülen şüpheli gönderenlerden birkaç dakika sonra yeniden denemeleri istenir; gerçek posta sunucuları yeniden dener, istenmeyen posta gönderen çoğu yazılım denemez.")}
            checked={greylisting}
            onChange={(event) => setGreylisting(event.target.checked)}
          />
        </div>
      </Panel>

      <Panel title={t("Gönderen listeleri")}>
        <div className="grid gap-4 sm:grid-cols-2">
          <Textarea
            label={t("Güvenilir gönderenler")}
            value={whitelist}
            onChange={(event) => setWhitelist(event.target.value)}
            hint={t("Her satıra bir giriş: adres@ornek.com ya da @ornek.com. Bu gönderenlerin iletileri istenmeyen posta olarak işaretlenmez.")}
            placeholder={"adres@ornek.com\n@ornek.com"}
            spellCheck={false}
          />
          <Textarea
            label={t("Engellenen gönderenler")}
            value={blacklist}
            onChange={(event) => setBlacklist(event.target.value)}
            hint={t("Her satıra bir giriş: adres@ornek.com ya da @ornek.com. Bu gönderenlerin iletileri reddedilir.")}
            placeholder={"adres@ornek.com\n@ornek.com"}
            spellCheck={false}
          />
        </div>
      </Panel>

      {localError && <ErrorBlock message={localError} />}
      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div>
        <Button type="submit" variant="primary" loading={save.isPending}>
          {t("Değişiklikleri kaydet")}
        </Button>
      </div>
    </form>
  );
}

function SpamTab() {
  const settings = useQuery({ queryKey: ["mail-settings"], queryFn: () => api.get<MailSettings>("/mail/settings") });
  if (settings.isPending) return <LoadingBlock />;
  if (settings.isError) return <ErrorBlock message={errorMessage(settings.error)} onRetry={() => void settings.refetch()} />;
  // `key`, kaydetme sonrası gelen değerlerle formun sıfırlanmasını sağlar.
  return <SpamForm key={JSON.stringify(settings.data)} settings={settings.data} />;
}

// ---- Etkinlik ----

const actionView: Record<string, { tone: Tone; label: string }> = {
  "no action": { tone: "ok", label: "Teslim edildi" },
  "add header": { tone: "warn", label: "Junk" },
  "rewrite subject": { tone: "warn", label: "Junk" },
  greylist: { tone: "idle", label: "Gri liste" },
  "soft reject": { tone: "idle", label: "Gri liste" },
  reject: { tone: "bad", label: "Reddedildi" },
};

function ActivityTab() {
  const activity = useQuery({ queryKey: ["mail-activity"], queryFn: () => api.get<MailActivity>("/mail/activity"), refetchInterval: 20000 });
  if (activity.isPending) return <LoadingBlock />;
  if (activity.isError) return <ErrorBlock message={errorMessage(activity.error)} onRetry={() => void activity.refetch()} />;

  const { scanned, actions, learned, messages } = activity.data;
  const count = (...names: string[]) => (actions ? names.reduce((sum, name) => sum + (actions[name] ?? 0), 0) : null);
  const summary: { label: string; value: number | null }[] = [
    { label: t("Taranan ileti"), value: scanned },
    { label: t("Temiz"), value: count("no action") },
    { label: t("Junk'a alınan"), value: count("add header", "rewrite subject") },
    { label: t("Reddedilen"), value: count("reject") },
    { label: t("Gri listeye alınan"), value: count("greylist", "soft reject") },
    { label: t("Öğrenilen ileti"), value: learned },
  ];

  return (
    <div className="flex flex-col gap-4">
      <Panel>
        <dl className="grid grid-cols-2 gap-x-6 gap-y-3 sm:grid-cols-3 lg:grid-cols-6">
          {summary.map((item) => (
            <div key={item.label}>
              <dt className="text-[12.5px] text-ink-2">{item.label}</dt>
              <dd className="tabular text-[20px] leading-tight font-semibold">{item.value === null ? "—" : item.value.toLocaleString("tr-TR")}</dd>
            </div>
          ))}
        </dl>
      </Panel>

      <Panel flush title={t("Son işlenen iletiler")}>
        {messages.length === 0 ? (
          <EmptyState
            title={t("Henüz işlenen ileti yok")}
            description={t("Sunucuya gelen iletiler işlendikçe burada görünür; liste 20 saniyede bir yenilenir.")}
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Zaman")}</Th>
                <Th>{t("Gönderen")}</Th>
                <Th>{t("Alıcılar")}</Th>
                <Th>{t("Konu")}</Th>
                <Th>{t("Sonuç")}</Th>
                <Th align="right">{t("Puan")}</Th>
              </tr>
            </thead>
            <tbody>
              {messages.map((message, index) => {
                const view = actionView[message.action];
                const recipients = message.recipients ?? [];
                return (
                  <tr key={`${message.time}-${index}`} className="hover:bg-sunken/50">
                    <Td className="whitespace-nowrap text-ink-2">
                      <span title={formatDateTime(message.time)}>{t(formatRelative(message.time))}</span>
                    </Td>
                    <Td className="max-w-56">
                      <p className="truncate">
                        <Mono>{message.sender || t("(boş gönderen)")}</Mono>
                      </p>
                      {message.ip && <p className="truncate text-[12px] text-ink-3">{message.ip}</p>}
                    </Td>
                    <Td className="max-w-56">
                      {recipients.length === 0 ? (
                        <span className="text-ink-3">—</span>
                      ) : (
                        <p className="truncate" title={recipients.join(", ")}>
                          <Mono>{recipients[0]}</Mono>
                          {recipients.length > 1 && <span className="text-ink-3"> +{recipients.length - 1}</span>}
                        </p>
                      )}
                    </Td>
                    <Td className="max-w-72">
                      <p className="truncate" title={message.subject}>
                        {message.subject || <span className="text-ink-3">{t("(konu yok)")}</span>}
                      </p>
                    </Td>
                    <Td className="whitespace-nowrap">
                      {view ? <Badge tone={view.tone}>{t(view.label)}</Badge> : <Badge>{message.action}</Badge>}
                    </Td>
                    <Td align="right" className="whitespace-nowrap">
                      <Mono>
                        {message.score.toFixed(1)} / {message.required_score.toFixed(1)}
                      </Mono>
                    </Td>
                  </tr>
                );
              })}
            </tbody>
          </Table>
        )}
      </Panel>
    </div>
  );
}

// ---- Sayfa ----

type MailTab = "domains" | "queue" | "spam" | "activity";

/** Sekmenin adres çubuğundaki karşılığı (`?sekme=`). */
const tabParams: Record<MailTab, string> = {
  domains: "alan-adlari",
  queue: "kuyruk",
  spam: "istenmeyen-posta",
  activity: "etkinlik",
};

export function MailPage() {
  const me = useMe();
  const isAdmin = me.role === "admin";
  const showOwner = isAdmin || me.role === "reseller";
  const [searchParams, setSearchParams] = useSearchParams();
  const [creating, setCreating] = useState(false);
  const overview = useQuery({ queryKey: ["mail"], queryFn: () => api.get<MailOverview>("/mail") });

  const requested = (Object.keys(tabParams) as MailTab[]).find((tab) => tabParams[tab] === searchParams.get("sekme"));
  const tab: MailTab = isAdmin && requested ? requested : "domains";

  if (overview.isPending) return <LoadingBlock />;
  if (overview.isError) return <ErrorBlock message={errorMessage(overview.error)} onRetry={() => void overview.refetch()} />;

  const data = overview.data;

  if (!data.installed) {
    return (
      <>
        <PageHeader title={t("Posta")} description={t("Alan adlarınız için e-posta kutuları, takma adlar ve istenmeyen posta koruması.")} />
        <Panel flush>
          <EmptyState
            title={t("Posta sunucusu kurulu değil")}
            description={
              isAdmin
                ? t("E-posta kutuları oluşturabilmek için posta bileşenlerinin bu sunucuya kurulması gerekir.")
                : t("E-posta kutuları oluşturabilmek için posta sunucusunun kurulması gerekir. Sunucu yöneticinizden kurmasını isteyin.")
            }
            action={
              isAdmin ? (
                <Link to="/components" className="inline-flex h-9 items-center rounded-md border border-line-strong px-3.5 font-medium hover:bg-sunken">
                  {t("Bileşenler sayfasından kurun")}
                </Link>
              ) : undefined
            }
          />
        </Panel>
      </>
    );
  }

  const tabs: { id: MailTab; label: string }[] = [
    { id: "domains", label: t("Alan adları") },
    { id: "queue", label: t("Kuyruk") },
    { id: "spam", label: t("İstenmeyen posta") },
    { id: "activity", label: t("Etkinlik") },
  ];

  return (
    <>
      <PageHeader
        title={t("Posta")}
        description={t("Alan adlarınız için e-posta kutuları, takma adlar ve istenmeyen posta koruması.")}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
            {t("Alan adı ekle")}
          </Button>
        }
      />
      {isAdmin && (
        <Tabs
          tabs={tabs}
          value={tab}
          label={t("Posta bölümleri")}
          onChange={(next) => setSearchParams(next === "domains" ? {} : { sekme: tabParams[next] }, { replace: true })}
        />
      )}
      {tab === "domains" && <DomainsTab overview={data} isAdmin={isAdmin} showOwner={showOwner} onAdd={() => setCreating(true)} />}
      {tab === "queue" && <QueueTab />}
      {tab === "spam" && <SpamTab />}
      {tab === "activity" && <ActivityTab />}
      {creating && <CreateDomainModal onClose={() => setCreating(false)} overview={data} isAdmin={isAdmin} />}
    </>
  );
}
