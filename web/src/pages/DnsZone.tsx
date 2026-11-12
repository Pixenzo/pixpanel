// DNS bölgesi çalışma alanı: kayıt tablosu, kayıt ekleme/düzenleme, ad sunucuları,
// Cloudflare ayarları ve site/posta kayıtlarının otomatik eklenmesi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, ChevronDown, ChevronUp, ChevronsUpDown, Copy, Download, Pencil, Plus, RefreshCw, ShieldCheck, Trash2, Upload, Wand2 } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useMemo, useState } from "react";
import { Link, useNavigate, useParams } from "react-router";
import { useFeedback } from "../components/feedback";
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
  Panel,
  SearchInput,
  Select,
  Status,
  Table,
  Td,
  Textarea,
  Th,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import type { DnsRecord, RecordType, Zone } from "./Dns";
import { providerLabels } from "./Dns";

// ---- Türler ----

interface CloudflareInfo {
  ssl_mode: string | null;
  development_mode: boolean | null;
  writable_kinds: string[];
}

interface DsRecord {
  key_tag: number;
  algorithm: number;
  digest_type: number;
  digest: string;
}

interface DnssecKey {
  id: number;
  keytype: string;
  algorithm: string;
  dnskey: string;
  ds: DsRecord[];
}

interface DnssecInfo {
  enabled: boolean;
  keys: DnssecKey[];
}

interface ZoneDetail {
  zone: Zone;
  records: DnsRecord[];
  /** Yalnızca bu sunucudaki bölgelerde dolu; Cloudflare bölgelerinde null (bölüm gösterilmez). */
  dnssec: DnssecInfo | null;
  kinds: string[];
  /** Yalnızca bu sunucuda barındırılan bölgelerde dolu. */
  nameservers: string[] | null;
  server_ip: string;
  cloudflare: CloudflareInfo | null;
}

interface RecordsResult {
  records: DnsRecord[];
}

interface CloudflareChanges {
  ssl_mode?: string;
  development_mode?: boolean;
}

const detailKey = (zoneId: number | string) => ["dns-zone", String(zoneId)] as const;

const typeOrder: RecordType[] = ["A", "AAAA", "CNAME", "MX", "TXT", "NS", "SRV", "CAA"];
const proxyableTypes: RecordType[] = ["A", "AAAA", "CNAME"];

const sslModes: { id: string; label: string }[] = [
  { id: "off", label: "Kapalı" },
  { id: "flexible", label: "Esnek" },
  { id: "full", label: "Tam" },
  { id: "strict", label: "Tam (katı)" },
];

const isRecordType = (value: string): value is RecordType => (typeOrder as string[]).includes(value);

// ---- Yardımcılar ----

/** Tam adı bölgeye göre kısaltır: bölgenin kendisi "@", alt ad "www". */
function relativeName(name: string, zone: string): string {
  const lower = name.toLowerCase().replace(/\.$/, "");
  if (lower === zone) return "@";
  if (lower.endsWith(`.${zone}`)) return name.slice(0, name.length - zone.length - 1);
  return name;
}

function formatTtl(ttl: number): string {
  if (ttl === 1) return t("Otomatik");
  if (ttl % 86400 === 0) return `${ttl / 86400} ${t("gün")}`;
  if (ttl % 3600 === 0) return `${ttl / 3600} ${t("sa")}`;
  if (ttl % 60 === 0) return `${ttl / 60} ${t("dk")}`;
  return `${ttl} ${t("sn")}`;
}

function useCopy(): [boolean, (value: string) => Promise<void>] {
  const [copied, setCopied] = useState(false);
  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Pano kapalıysa kullanıcı metni elle seçip kopyalayabilir.
    }
  }
  return [copied, copy];
}

/** Tek satırlık, kopyalanabilir makine değeri. */
function CopyValue({ value, display }: { value: string; display?: string }) {
  const [copied, copy] = useCopy();
  return (
    <div className="flex min-w-0 items-center gap-1">
      <code title={value} className="min-w-0 flex-1 truncate rounded-md border border-line bg-sunken/60 px-3 py-1.5 font-mono text-[13px] select-all">{display ?? value}</code>
      <IconButton label={copied ? t("Kopyalandı") : t("Kopyala")} onClick={() => void copy(value)}>
        {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
      </IconButton>
    </div>
  );
}

/** Önbellekteki bölge ayrıntısının kayıtlarını günceller. */
function useRecordsCache(zoneId: number) {
  const queryClient = useQueryClient();
  return (records: DnsRecord[]) =>
    queryClient.setQueryData<ZoneDetail>(detailKey(zoneId), (old) => (old ? { ...old, records } : old));
}

// ---- Kayıt penceresi ----

interface ValueField {
  label: string;
  placeholder: string;
  hint: string;
}

const valueFields: Record<RecordType, ValueField> = {
  A: { label: "IPv4 adresi", placeholder: "203.0.113.10", hint: "Adın işaret edeceği IPv4 adresi." },
  AAAA: { label: "IPv6 adresi", placeholder: "2001:db8::1", hint: "Adın işaret edeceği IPv6 adresi." },
  CNAME: { label: "Hedef ad", placeholder: "hedef.ornek.com", hint: "Bu adın takma ad olarak işaret edeceği alan adı." },
  MX: {
    label: "Posta sunucusu",
    placeholder: "mail.ornek.com",
    hint: "Postayı kabul eden sunucunun adı (IP adresi olamaz). Önceliği düşük olan sunucu önce denenir.",
  },
  TXT: { label: "Metin", placeholder: "v=spf1 a mx ~all", hint: "Tırnak işareti eklemeyin; metin olduğu gibi kaydedilir." },
  NS: { label: "Ad sunucusu", placeholder: "ns1.ornek.com", hint: "Bu ad için yetkiyi devrettiğiniz ad sunucusu (alt alan adı delegasyonu)." },
  SRV: {
    label: "Ağırlık, port ve hedef",
    placeholder: "10 5060 sip.ornek.com",
    hint: "Boşlukla ayrılmış üç değer: ağırlık port hedef. Ad genellikle _hizmet._protokol biçimindedir.",
  },
  CAA: {
    label: "CAA değeri",
    placeholder: '0 issue "letsencrypt.org"',
    hint: 'Biçim: bayrak etiket "değer". Etiket issue, issuewild ya da iodef olabilir.',
  },
};

const baseTtls = [300, 3600, 14400, 86400];

/** Alan adı sabit bir son ek olarak gösterilen ad girdisi. */
function NameInput({ value, onChange, zone, autoFocus }: { value: string; onChange: (next: string) => void; zone: string; autoFocus?: boolean }) {
  return (
    <div className="flex flex-col gap-1.5">
      <label className="flex flex-col gap-1.5">
        <span className="text-[13.5px] font-medium text-ink-2">{t("Ad")}</span>
        <span className="flex">
          <input
            value={value}
            onChange={(event) => onChange(event.target.value)}
            placeholder="@"
            autoFocus={autoFocus}
            autoCapitalize="none"
            autoComplete="off"
            spellCheck={false}
            className="h-9 min-w-0 flex-1 rounded-l-md border border-r-0 border-line-strong bg-surface px-3 font-mono text-[13.5px] text-ink transition-colors placeholder:text-ink-3 focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30"
          />
          <span className="inline-flex h-9 max-w-[50%] items-center truncate rounded-r-md border border-line-strong bg-sunken px-3 font-mono text-[13.5px] text-ink-2">.{zone}</span>
        </span>
      </label>
      <p className="text-[13px] text-ink-3">{t("@ ya da boş: alan adının kendisi")}</p>
    </div>
  );
}

function RecordModal({ detail, record, onClose }: { detail: ZoneDetail; record: DnsRecord | null; onClose: () => void }) {
  const feedback = useFeedback();
  const updateRecords = useRecordsCache(detail.zone.id);
  const zone = detail.zone.domain;
  const cloudflare = detail.cloudflare;

  const availableTypes = typeOrder.filter((type) => (cloudflare ? cloudflare.writable_kinds.includes(type) : detail.kinds.length === 0 || detail.kinds.includes(type)));
  const [type, setType] = useState<RecordType>(record?.type ?? availableTypes[0] ?? "A");
  const [name, setName] = useState(record ? relativeName(record.name, zone) : "");
  const [content, setContent] = useState(record?.content ?? "");
  const [priority, setPriority] = useState(record?.priority === null || record?.priority === undefined ? "10" : String(record.priority));
  const [ttl, setTtl] = useState(record?.ttl ?? 3600);
  const [proxied, setProxied] = useState(record?.proxied ?? false);
  const [localError, setLocalError] = useState<string | null>(null);

  const ttlOptions = [...(cloudflare ? [1] : []), ...baseTtls, ...(ttl !== 1 && !baseTtls.includes(ttl) ? [ttl] : [])].sort((a, b) => a - b);
  const showProxy = cloudflare !== null && proxyableTypes.includes(type);
  const proxyOn = showProxy && proxied;
  const needsPriority = type === "MX" || type === "SRV";
  const field = valueFields[type];

  const save = useMutation({
    mutationFn: (next: DnsRecord) =>
      record
        ? api.put<RecordsResult>(`/dns/zones/${detail.zone.id}/records`, { old: record, new: next })
        : api.post<RecordsResult>(`/dns/zones/${detail.zone.id}/records`, { record: next }),
    onSuccess: (result) => {
      updateRecords(result.records);
      feedback.success(record ? t("Kayıt güncellendi") : t("Kayıt eklendi"));
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    let priorityValue: number | null = null;
    if (needsPriority) {
      priorityValue = Number(priority);
      if (priority.trim() === "" || !Number.isInteger(priorityValue) || priorityValue < 0 || priorityValue > 65535) {
        setLocalError(t("Öncelik, 0 ile 65535 arasında bir tam sayı olmalıdır."));
        return;
      }
    }
    if (!content.trim()) {
      setLocalError(t("Değer boş olamaz."));
      return;
    }
    setLocalError(null);
    save.mutate({
      id: record?.id ?? "",
      type,
      name: name.trim() || "@",
      content: content.trim(),
      // Vekil açıkken Cloudflare TTL'yi kendisi belirler.
      ttl: proxyOn ? 1 : ttl,
      priority: priorityValue,
      proxied: cloudflare !== null && proxyableTypes.includes(type) ? proxied : null,
    });
  }

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={record ? t("Kaydı düzenle") : t("Kayıt ekle")}
      description={zone}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="dns-record-form" loading={save.isPending}>
            {record ? t("Kaydet") : t("Kaydı ekle")}
          </Button>
        </>
      }
    >
      <form id="dns-record-form" onSubmit={submit} className="flex flex-col gap-5">
        <div className="grid gap-4 sm:grid-cols-[8rem_minmax(0,1fr)]">
          <Select label={t("Tür")} value={type} disabled={record !== null} onChange={(event) => isRecordType(event.target.value) && setType(event.target.value)}>
            {(record ? [record.type] : availableTypes).map((item) => (
              <option key={item} value={item}>
                {item}
              </option>
            ))}
          </Select>
          <NameInput value={name} onChange={setName} zone={zone} autoFocus />
        </div>

        <div className={classNames("grid gap-4", needsPriority && "sm:grid-cols-[8rem_minmax(0,1fr)]")}>
          {needsPriority && (
            <Input
              label={t("Öncelik")}
              type="number"
              min={0}
              max={65535}
              value={priority}
              onChange={(event) => setPriority(event.target.value)}
              required
            />
          )}
          {type === "TXT" ? (
            <Textarea
              label={t(field.label)}
              value={content}
              onChange={(event) => setContent(event.target.value)}
              placeholder={field.placeholder}
              hint={t(field.hint)}
              spellCheck={false}
              required
            />
          ) : (
            <div className="flex flex-col gap-1.5">
              <Input
                label={t(field.label)}
                value={content}
                onChange={(event) => setContent(event.target.value)}
                placeholder={field.placeholder}
                hint={t(field.hint)}
                className="font-mono text-[13.5px]"
                autoCapitalize="none"
                autoComplete="off"
                spellCheck={false}
                required
              />
              {type === "A" && detail.server_ip && (
                <div>
                  <button type="button" className="text-[13px] font-medium text-accent hover:underline" onClick={() => setContent(detail.server_ip)}>
                    {t("Bu sunucunun adresini kullan")} (<Mono>{detail.server_ip}</Mono>)
                  </button>
                </div>
              )}
            </div>
          )}
        </div>

        <div className="grid gap-4 sm:grid-cols-[12rem_minmax(0,1fr)] sm:items-start">
          <Select
            label={t("TTL")}
            value={proxyOn ? 1 : ttl}
            disabled={proxyOn}
            onChange={(event) => setTtl(Number(event.target.value))}
            hint={proxyOn ? t("Vekil açıkken TTL otomatiktir.") : undefined}
          >
            {ttlOptions.map((item) => (
              <option key={item} value={item}>
                {item === 1 ? t("Otomatik") : formatTtl(item)}
              </option>
            ))}
          </Select>
          {showProxy && (
            <div className="sm:pt-6">
              <Checkbox
                label={t("Cloudflare üzerinden vekille (turuncu bulut)")}
                hint={t("Açıkken trafik Cloudflare'den geçer; gerçek sunucu adresi gizlenir, önbellek ve koruma devreye girer.")}
                checked={proxied}
                onChange={(event) => setProxied(event.target.checked)}
              />
            </div>
          )}
        </div>

        {(localError || save.isError) && <ErrorBlock message={localError ?? errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

// ---- Eşitleme sonucu ----

function SyncReportModal({ report, onClose }: { report: string[]; onClose: () => void }) {
  return (
    <Modal open onClose={onClose} wide title={t("Site ve posta kayıtları")} footer={<Button variant="primary" onClick={onClose}>{t("Tamam")}</Button>}>
      {report.length === 0 ? (
        <p className="text-ink-2">{t("Eklenecek yeni kayıt yok; sitelerinizin ve posta alan adlarınızın kayıtları zaten tanımlı.")}</p>
      ) : (
        <ul className="flex list-disc flex-col gap-1 pl-5 text-ink-2">
          {report.map((line, index) => (
            <li key={index}>{line}</li>
          ))}
        </ul>
      )}
    </Modal>
  );
}

// ---- Ad sunucuları ----

function NameserversPanel({ nameservers, serverIp }: { nameservers: string[]; serverIp: string }) {
  return (
    <Panel title={t("Ad sunucuları")} className="mb-4">
      <div className="flex flex-col gap-3">
        <p className="max-w-[80ch] text-ink-2">
          {t("Alan adınızın kayıt şirketinde ad sunucularını bunlarla değiştirin. Bu adlar aynı alan adının altındaysa kayıt şirketinde “glue” (ad sunucusu kaydı) olarak {ip} adresiyle tanımlanmalıdır.", {
            ip: serverIp || "—",
          })}
        </p>
        {nameservers.length === 0 ? (
          <p className="text-ink-3">{t("Henüz ad sunucusu tanımlı değil.")}</p>
        ) : (
          <div className="grid gap-2 md:grid-cols-2">
            {nameservers.map((nameserver) => (
              <CopyValue key={nameserver} value={nameserver} />
            ))}
          </div>
        )}
      </div>
    </Panel>
  );
}

// ---- Cloudflare şeridi ----

function CloudflareStrip({ zoneId, info }: { zoneId: number; info: CloudflareInfo }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  const change = useMutation({
    mutationFn: (changes: CloudflareChanges) => api.post<unknown>(`/dns/zones/${zoneId}/cloudflare`, changes),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: detailKey(zoneId) });
      feedback.success(t("Cloudflare ayarı güncellendi"));
    },
    onError: (error) => feedback.error(error),
  });

  const purge = useMutation({
    mutationFn: () => api.post<unknown>(`/dns/zones/${zoneId}/purge`),
    onSuccess: () => feedback.success(t("Önbellek temizlendi")),
    onError: (error) => feedback.error(error),
  });

  async function confirmPurge() {
    const confirmed = await feedback.confirm({
      title: t("Önbellek temizlensin mi?"),
      description: t("Bölgenin Cloudflare önbelleğindeki bütün içerik silinir; sonraki istekler sunucunuza gider ve önbellek yeniden dolar. Bu kısa süre yükü artırabilir."),
      confirmLabel: t("Önbelleği temizle"),
    });
    if (confirmed) purge.mutate();
  }

  const sslValue = change.isPending && change.variables?.ssl_mode !== undefined ? change.variables.ssl_mode : (info.ssl_mode ?? "");
  const devValue = change.isPending && change.variables?.development_mode !== undefined ? change.variables.development_mode : (info.development_mode ?? false);
  const knownSsl = sslModes.some((mode) => mode.id === sslValue);

  return (
    <div className="mb-4 flex flex-wrap items-start gap-x-8 gap-y-3 rounded-lg border border-line bg-surface px-4 sm:px-6 py-3">
      <div className="flex min-w-48 flex-col gap-1">
        {info.ssl_mode === null ? (
          <>
            <span className="text-[13.5px] font-medium text-ink-2">{t("SSL kipi")}</span>
            <p className="max-w-64 text-[13px] text-ink-3">{t("Anahtarın Zone Settings izni yok; SSL kipi buradan değiştirilemez.")}</p>
          </>
        ) : (
          <Select
            label={t("SSL kipi")}
            value={sslValue}
            disabled={change.isPending}
            onChange={(event) => change.mutate({ ssl_mode: event.target.value })}
            hint={t("“Tam (katı)” için sunucuda geçerli bir sertifika olmalıdır.")}
            className="h-8 w-44"
          >
            {!knownSsl && <option value={sslValue}>{sslValue}</option>}
            {sslModes.map((mode) => (
              <option key={mode.id} value={mode.id}>
                {t(mode.label)}
              </option>
            ))}
          </Select>
        )}
      </div>

      <div className="flex min-w-48 flex-col gap-1">
        {info.development_mode === null ? (
          <>
            <span className="text-[13.5px] font-medium text-ink-2">{t("Geliştirme kipi")}</span>
            <p className="max-w-64 text-[13px] text-ink-3">{t("Anahtarın Zone Settings izni yok; geliştirme kipi buradan değiştirilemez.")}</p>
          </>
        ) : (
          <div className="sm:pt-5">
            <Checkbox
              label={t("Geliştirme kipi")}
              hint={t("Önbelleği 3 saat süreyle atlar.")}
              checked={devValue}
              disabled={change.isPending}
              onChange={(event) => change.mutate({ development_mode: event.target.checked })}
            />
          </div>
        )}
      </div>

      <div className="ml-auto sm:pt-5">
        <Button icon={<RefreshCw className="size-3.5" />} loading={purge.isPending} onClick={() => void confirmPurge()}>
          {t("Önbelleği temizle")}
        </Button>
      </div>
    </div>
  );
}

// ---- Kayıtlar ----

type SortKey = "name" | "type" | "content" | "ttl";

/** Tıklanınca tabloyu o sütuna göre sıralayan başlık. */
function SortHeader({ label, active, descending, onClick }: { label: string; active: boolean; descending: boolean; onClick: () => void }) {
  const Icon = !active ? ChevronsUpDown : descending ? ChevronDown : ChevronUp;
  return (
    <button type="button" onClick={onClick} className={classNames("-mx-1 inline-flex items-center gap-1 rounded px-1 hover:text-ink", active && "text-ink")}>
      {label}
      <Icon aria-hidden className={classNames("size-3.5", active ? "text-accent" : "text-ink-3")} />
    </button>
  );
}

function RecordsPanel({
  detail,
  onAdd,
  onEdit,
  tools,
}: {
  detail: ZoneDetail;
  onAdd: () => void;
  onEdit: (record: DnsRecord) => void;
  /** Araç çubuğunun sağındaki ek düğmeler (içe/dışa aktarma gibi). */
  tools: ReactNode;
}) {
  const feedback = useFeedback();
  const updateRecords = useRecordsCache(detail.zone.id);
  const zone = detail.zone.domain;
  const cloudflare = detail.cloudflare;
  const [search, setSearch] = useState("");
  const [typeFilter, setTypeFilter] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; descending: boolean }>({ key: "name", descending: false });
  const sortBy = (key: SortKey) => setSort((current) => ({ key, descending: current.key === key ? !current.descending : false }));

  const remove = useMutation({
    mutationFn: (record: DnsRecord) => api.post<RecordsResult>(`/dns/zones/${detail.zone.id}/records/delete`, { record }),
    onSuccess: (result) => {
      updateRecords(result.records);
      feedback.success(t("Kayıt silindi"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(record: DnsRecord) {
    const confirmed = await feedback.confirm({
      title: t("Kayıt silinsin mi?"),
      description: t("{type} kaydı {name} silinir. Bu kaydı kullanan hizmetler çalışmayabilir.", {
        type: record.type,
        name: relativeName(record.name, zone),
      }),
      confirmLabel: t("Kaydı sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(record);
  }

  const presentTypes = useMemo(() => {
    const present = new Set(detail.records.map((record) => record.type));
    return typeOrder.filter((type) => present.has(type));
  }, [detail.records]);

  const rows = useMemo(() => {
    const query = search.trim().toLowerCase();
    return detail.records
      .filter((record) => (typeFilter ? record.type === typeFilter : true))
      .filter((record) => (query ? record.name.toLowerCase().includes(query) || record.content.toLowerCase().includes(query) : true))
      .map((record) => ({ record, short: relativeName(record.name, zone) }))
      .sort((a, b) => {
        // Varsayılan sıra: kök (@) kayıtları başta, sonra ada göre.
        const byName = () => {
          if (a.short === "@" && b.short !== "@") return -1;
          if (b.short === "@" && a.short !== "@") return 1;
          return a.short.localeCompare(b.short);
        };
        const byType = () => typeOrder.indexOf(a.record.type) - typeOrder.indexOf(b.record.type);
        const byContent = () => a.record.content.localeCompare(b.record.content);
        const order =
          sort.key === "type"
            ? byType() || byName() || byContent()
            : sort.key === "content"
              ? byContent() || byName()
              : sort.key === "ttl"
                ? a.record.ttl - b.record.ttl || byName()
                : byName() || byType() || byContent();
        return sort.descending ? -order : order;
      });
  }, [detail.records, search, typeFilter, zone, sort]);

  const total = detail.records.length;

  return (
    <section aria-label={t("DNS kayıtları")}>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <SearchInput
          aria-label={t("Kayıtlarda ara")}
          placeholder={t("Ad ya da değer ara")}
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          className="min-w-48 flex-1 sm:w-auto"
        />
        <select
          aria-label={t("Türe göre süz")}
          value={typeFilter}
          onChange={(event) => setTypeFilter(event.target.value)}
          className="h-9.5 rounded-lg border border-line-strong bg-surface px-3 pr-8 text-ink focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 max-sm:h-10.5"
        >
          <option value="">{t("Bütün türler")}</option>
          {presentTypes.map((type) => (
            <option key={type} value={type}>
              {type}
            </option>
          ))}
        </select>
        {tools}
        <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAdd}>
          {t("Kayıt ekle")}
        </Button>
      </div>
      <Panel flush>
        {total === 0 ? (
          <EmptyState
            title={t("Bu bölgede henüz kayıt yok")}
            description={t("İlk kaydı ekleyin ya da sitelerinizin ve posta alan adlarınızın kayıtlarını otomatik ekletin.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAdd}>
                {t("Kayıt ekle")}
              </Button>
            }
          />
        ) : rows.length === 0 ? (
          <EmptyState title={t("Aramanızla eşleşen kayıt yok")} />
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>
                    <SortHeader label={t("Ad")} active={sort.key === "name"} descending={sort.descending} onClick={() => sortBy("name")} />
                  </Th>
                  <Th>
                    <SortHeader label={t("Tür")} active={sort.key === "type"} descending={sort.descending} onClick={() => sortBy("type")} />
                  </Th>
                  <Th>
                    <SortHeader label={t("Değer")} active={sort.key === "content"} descending={sort.descending} onClick={() => sortBy("content")} />
                  </Th>
                  {cloudflare && <Th>{t("Vekil")}</Th>}
                  <Th>
                    <SortHeader label={t("TTL")} active={sort.key === "ttl"} descending={sort.descending} onClick={() => sortBy("ttl")} />
                  </Th>
                  <Th align="right">
                    <span className="sr-only">{t("İşlemler")}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {rows.map(({ record, short }) => {
                  const locked = cloudflare !== null && !cloudflare.writable_kinds.includes(record.type);
                  const lockedTitle = t("Bu tür kayıtlar Cloudflare panelinden yönetilir.");
                  return (
                    <tr key={record.id || `${record.type}-${record.name}-${record.content}`} className="hover:bg-sunken/50">
                      <Td className="max-w-64">
                        <p className="truncate" title={record.name}>
                          {short === "@" ? (
                            <Mono className="text-[13.5px] font-medium">{zone}</Mono>
                          ) : (
                            <>
                              <Mono className="text-[13.5px] font-medium">{short}</Mono>
                              <Mono className="text-[13.5px] text-ink-3">.{zone}</Mono>
                            </>
                          )}
                        </p>
                      </Td>
                      <Td className="whitespace-nowrap">
                        <span className="font-mono text-[13px] font-semibold text-ink-2">{record.type}</span>
                      </Td>
                      <Td className="max-w-md">
                        <p className="truncate" title={record.content}>
                          {record.priority !== null && <span className="tabular mr-1.5 text-[12.5px] text-ink-3">{record.priority}</span>}
                          <Mono>{record.content}</Mono>
                        </p>
                      </Td>
                      {cloudflare && (
                        <Td className="whitespace-nowrap">
                          {proxyableTypes.includes(record.type) ? (
                            record.proxied ? <Status tone="accent">{t("Cloudflare üzerinden")}</Status> : <Status tone="idle">{t("Yalnızca DNS")}</Status>
                          ) : (
                            <span className="text-ink-3">{t("Yalnızca DNS")}</span>
                          )}
                        </Td>
                      )}
                      <Td className="whitespace-nowrap text-ink-2">{formatTtl(record.ttl)}</Td>
                      <Td align="right" className="whitespace-nowrap">
                        <div className="flex justify-end gap-1">
                          <IconButton label={locked ? lockedTitle : t("Düzenle")} disabled={locked} onClick={() => onEdit(record)}>
                            <Pencil className="size-4" />
                          </IconButton>
                          <IconButton label={locked ? lockedTitle : t("Sil")} disabled={locked || remove.isPending} onClick={() => void confirmRemove(record)}>
                            <Trash2 className="size-4" />
                          </IconButton>
                        </div>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          </>
        )}
        {total > 0 && (
          <p className="tabular border-t border-line px-4 py-2.5 text-[13px] text-ink-3 sm:px-6">
            {rows.length === total ? t("{n} kayıt", { n: total }) : t("{shown} / {total} kayıt", { shown: rows.length, total })}
          </p>
        )}
      </Panel>
    </section>
  );
}

// ---- Öneriler ----

interface Advice {
  id: string;
  title: string;
  detail: string;
}

/** Kayıtlara bakarak eksik görünen yaygın ayarları bulur (posta sahteciliği koruması, sertifika yetkisi). */
function zoneAdvice(detail: ZoneDetail): Advice[] {
  const zone = detail.zone.domain;
  const records = detail.records;
  const has = (type: string, name: string, test?: (content: string) => boolean) =>
    records.some((record) => record.type === type && record.name.replace(/\.$/, "").toLowerCase() === name && (!test || test(record.content.toLowerCase().replace(/^"/, ""))));
  const advice: Advice[] = [];
  const receivesMail = has("MX", zone);
  if (receivesMail && !has("TXT", zone, (content) => content.startsWith("v=spf1"))) {
    advice.push({
      id: "spf",
      title: t("Gönderen doğrulaması (SPF) eksik"),
      detail: t("SPF kaydı olmadan alan adınız adına başkaları posta gönderebilir ve iletileriniz istenmeyen klasörüne düşebilir. “Site ve posta kayıtlarını ekle” bu kaydı ekler."),
    });
  }
  if (receivesMail && !has("TXT", `_dmarc.${zone}`, (content) => content.startsWith("v=dmarc1"))) {
    advice.push({
      id: "dmarc",
      title: t("Sahte postaları engelleyen DMARC kaydı eksik"),
      detail: t("DMARC kaydı, alan adınızı taklit eden iletilerin alıcılara ulaşmasını engeller. “Site ve posta kayıtlarını ekle” bu kaydı ekler."),
    });
  }
  if (has("A", zone) && !has("A", `www.${zone}`) && !has("CNAME", `www.${zone}`) && !has("AAAA", `www.${zone}`)) {
    advice.push({
      id: "www",
      title: t("www adı tanımlı değil"),
      detail: t("Ziyaretçilerin bir bölümü adresi www ile yazar; www.{zone} için bir A ya da CNAME kaydı ekleyin.", { zone }),
    });
  }
  if (detail.zone.provider === "local" && detail.dnssec && !detail.dnssec.enabled && records.length > 0) {
    advice.push({
      id: "dnssec",
      title: t("DNSSEC kapalı"),
      detail: t("DNSSEC, ziyaretçilerin sahte yanıtlarla başka adrese yönlendirilmesini önler. Aşağıdaki DNSSEC bölümünden açabilirsiniz."),
    });
  }
  return advice;
}

function AdvicePanel({ advice }: { advice: Advice[] }) {
  const [open, setOpen] = useState(true);
  if (advice.length === 0) return null;
  return (
    <section aria-label={t("Öneriler")} className="mb-5 overflow-hidden rounded-2xl border border-line bg-surface shadow-card">
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)} className="flex w-full items-center gap-2.5 px-4 py-3 text-left sm:px-6">
        <span className="font-medium">{t("Öneriler")}</span>
        <span className="tabular grid h-5 min-w-5 place-items-center rounded-full bg-accent px-1.5 text-[12px] font-semibold text-accent-ink">{advice.length}</span>
        <ChevronDown aria-hidden className={classNames("ml-auto size-4 text-ink-3 transition-transform", open && "rotate-180")} />
      </button>
      {open && (
        <ul className="flex flex-col gap-3 border-t border-line px-4 py-3.5 sm:px-6">
          {advice.map((item) => (
            <li key={item.id} className="flex gap-3">
              <span aria-hidden className="mt-2 size-1.5 shrink-0 rounded-full bg-accent" />
              <div className="min-w-0">
                <p className="font-medium">{item.title}</p>
                <p className="max-w-[80ch] text-[13.5px] text-ink-2">{item.detail}</p>
              </div>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

// ---- DNSSEC ----

const dsAlgorithms: Record<number, string> = { 8: "RSA/SHA-256", 13: "ECDSA P-256", 14: "ECDSA P-384", 15: "Ed25519", 16: "Ed448" };
const dsDigests: Record<number, string> = { 1: "SHA-1", 2: "SHA-256", 4: "SHA-384" };

function labelled(value: number, names: Record<number, string>): string {
  return names[value] ? `${value} (${names[value]})` : String(value);
}

function DsField({ label, value, shown }: { label: string; value: string; shown?: string }) {
  return (
    <div className="flex min-w-0 flex-col gap-1">
      <span className="text-[13px] font-medium text-ink-2">{label}</span>
      <CopyValue value={value} display={shown} />
    </div>
  );
}

function DnssecPanel({ zone, info }: { zone: Zone; info: DnssecInfo }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  const toggle = useMutation({
    mutationFn: (enabled: boolean) => api.post<{ dnssec: DnssecInfo }>(`/dns/zones/${zone.id}/dnssec`, { enabled }),
    onSuccess: (result, enabled) => {
      queryClient.setQueryData<ZoneDetail>(detailKey(zone.id), (old) => (old ? { ...old, dnssec: result.dnssec } : old));
      feedback.success(enabled ? t("DNSSEC açıldı. DS kaydını kayıt firmasına girmeyi unutmayın.") : t("DNSSEC kapatıldı"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmDisable() {
    const confirmed = await feedback.confirm({
      title: t("{domain} için DNSSEC kapatılsın mı?", { domain: zone.domain }),
      description: t(
        "Kapatmadan ÖNCE alan adını aldığınız kayıt firmasından DS kaydını kaldırmış olmalısınız. DS kaydı yerinde dururken DNSSEC kapatılırsa doğrulama yapan çözücüler alan adını çözemez; siteniz ve postanız erişilemez olur. DS kaldırıldıktan sonra yayılması için kayıt süresi (genellikle 1–2 gün) beklenir.",
      ),
      confirmLabel: t("DNSSEC'i kapat"),
      danger: true,
      typeToConfirm: zone.domain,
    });
    if (confirmed) toggle.mutate(false);
  }

  async function confirmEnable() {
    const confirmed = await feedback.confirm({
      title: t("{domain} için DNSSEC açılsın mı?", { domain: zone.domain }),
      description: t(
        "Bölge imzalanır ve ECDSA P-256 anahtarı üretilir. DNSSEC, kayıt firmasına DS kaydı girilene kadar doğrulanmaz; DS girildikten sonra da bu bölgeyi kapatmadan önce DS'yi kaldırmanız gerekir.",
      ),
      confirmLabel: t("DNSSEC'i aç"),
    });
    if (confirmed) toggle.mutate(true);
  }

  return (
    <Panel
      title={
        <span className="inline-flex items-center gap-2">
          <ShieldCheck className="size-4 text-ink-2" /> {t("DNSSEC")}
        </span>
      }
      actions={
        info.enabled ? (
          <Button size="sm" variant="danger" loading={toggle.isPending} onClick={() => void confirmDisable()}>
            {t("DNSSEC'i kapat")}
          </Button>
        ) : (
          <Button size="sm" variant="primary" loading={toggle.isPending} onClick={() => void confirmEnable()}>
            {t("DNSSEC'i aç")}
          </Button>
        )
      }
      className="mb-4"
    >
      <div className="flex flex-col gap-4">
        <div className="flex flex-wrap items-center gap-3">
          <Status tone={info.enabled ? "ok" : "idle"}>{info.enabled ? t("Açık") : t("Kapalı")}</Status>
          <p className="max-w-[80ch] text-ink-2">
            {info.enabled
              ? t("Bölge imzalı yayınlanıyor. Yeni ve değişen kayıtlar kendiliğinden imzalanır.")
              : t("DNSSEC, yanıtların sahte olmadığını doğrulanabilir hâle getirir. Açınca bölge ECDSA P-256 ile imzalanır; ardından alan adını aldığınız kayıt firmasına DS kaydını girmeniz gerekir.")}
          </p>
        </div>

        {info.enabled && (
          <>
            <div role="note" className="rounded-lg border border-warn/40 bg-warn-soft px-4 py-3 text-warn">
              <p className="font-medium">{t("DS kaydını kayıt firmasına girin")}</p>
              <ul className="mt-1 list-disc pl-5 text-[13.5px]">
                <li>{t("Aşağıdaki DS kaydı alan adını aldığınız kayıt firmasının panelinde girilmedikçe çözümleme doğrulanmaz; DNSSEC bir güvenlik sağlamaz.")}</li>
                <li>{t("DNSSEC'i kapatmadan ÖNCE kayıt firmasından DS kaydını kaldırın; yoksa alan adı çözülmez.")}</li>
                <li>{t("Çoğu firma yalnızca DS kaydını ister; bazıları DNSKEY de ister, ikisi de aşağıda.")}</li>
              </ul>
            </div>

            {info.keys.map((key) => (
              <div key={key.id} className="flex flex-col gap-4 rounded-lg border border-line p-4">
                <p className="text-[13.5px] text-ink-2">
                  {t("Anahtar")} {key.id} · {key.keytype.toUpperCase()} · {key.algorithm}
                </p>
                {key.ds.length === 0 && <p className="text-ink-3">{t("Bu anahtar için DS kaydı üretilemedi.")}</p>}
                {key.ds.map((ds) => (
                  <div key={`${ds.digest_type}`} className="flex flex-col gap-3">
                    <div className="grid gap-3 sm:grid-cols-3">
                      <DsField label={t("Anahtar etiketi (Key Tag)")} value={String(ds.key_tag)} />
                      <DsField label={t("Algoritma")} value={String(ds.algorithm)} shown={labelled(ds.algorithm, dsAlgorithms)} />
                      <DsField label={t("Özet türü (Digest Type)")} value={String(ds.digest_type)} shown={labelled(ds.digest_type, dsDigests)} />
                    </div>
                    <DsField label={t("Özet (Digest)")} value={ds.digest} />
                    <DsField label={t("DS kaydı (tek satır)")} value={`${zone.domain}. IN DS ${ds.key_tag} ${ds.algorithm} ${ds.digest_type} ${ds.digest}`} />
                  </div>
                ))}
                {key.dnskey && <DsField label={t("DNSKEY kaydı")} value={`${zone.domain}. IN DNSKEY ${key.dnskey}`} />}
              </div>
            ))}
          </>
        )}
      </div>
    </Panel>
  );
}

// ---- Bölge dosyası: dışa ve içe aktarma ----

interface SkippedLine {
  line: number;
  text: string;
  reason: string;
}

interface ImportReply {
  add: DnsRecord[];
  existing_count: number;
  remove: DnsRecord[];
  skipped: SkippedLine[];
  skipped_total: number;
  notes: string[];
  result: { added: number; removed: number; failed: string[] } | null;
  records?: DnsRecord[];
}

const MAX_IMPORT_BYTES = 512 * 1024;
const PREVIEW_ROWS = 100;

function RecordList({ records, zone }: { records: DnsRecord[]; zone: string }) {
  return (
    <Table>
      <thead>
        <tr>
          <Th>{t("Tür")}</Th>
          <Th>{t("Ad")}</Th>
          <Th>{t("Değer")}</Th>
          <Th>{t("TTL")}</Th>
        </tr>
      </thead>
      <tbody>
        {records.slice(0, PREVIEW_ROWS).map((record, index) => (
          <tr key={index}>
            <Td className="whitespace-nowrap">
              <Badge tone="accent">{record.type}</Badge>
            </Td>
            <Td className="max-w-40">
              <p className="truncate" title={record.name}>
                <Mono>{relativeName(record.name, zone)}</Mono>
              </p>
            </Td>
            <Td className="max-w-xs">
              <p className="truncate" title={record.content}>
                {record.priority !== null && <span className="tabular mr-1.5 text-[12.5px] text-ink-3">{record.priority}</span>}
                <Mono>{record.content}</Mono>
              </p>
            </Td>
            <Td className="whitespace-nowrap text-ink-2">{formatTtl(record.ttl)}</Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

function ImportModal({ detail, onClose }: { detail: ZoneDetail; onClose: () => void }) {
  const feedback = useFeedback();
  const updateRecords = useRecordsCache(detail.zone.id);
  const zone = detail.zone.domain;
  const [text, setText] = useState("");
  const [replace, setReplace] = useState(false);
  const [preview, setPreview] = useState<ImportReply | null>(null);
  const [applied, setApplied] = useState<ImportReply | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);

  const run = useMutation({
    mutationFn: (apply: boolean) => api.post<ImportReply>(`/dns/zones/${detail.zone.id}/import`, { text, replace, apply }),
    onSuccess: (reply, apply) => {
      if (!apply) {
        setPreview(reply);
        return;
      }
      if (reply.records) updateRecords(reply.records);
      setApplied(reply);
      if (reply.result && reply.result.failed.length === 0) {
        feedback.success(t("{added} kayıt eklendi, {removed} kayıt silindi", { added: reply.result.added, removed: reply.result.removed }));
        onClose();
      }
    },
  });

  function edit(next: string) {
    setText(next);
    setPreview(null);
    setApplied(null);
    setLocalError(null);
  }

  async function load(file: File | undefined) {
    if (!file) return;
    if (file.size > MAX_IMPORT_BYTES) {
      setLocalError(t("Dosya en çok {kb} KB olabilir.", { kb: MAX_IMPORT_BYTES / 1024 }));
      return;
    }
    edit(await file.text());
  }

  function startPreview() {
    if (!text.trim()) {
      setLocalError(t("Bölge dosyasını yapıştırın ya da bir dosya seçin."));
      return;
    }
    if (new Blob([text]).size > MAX_IMPORT_BYTES) {
      setLocalError(t("Metin en çok {kb} KB olabilir.", { kb: MAX_IMPORT_BYTES / 1024 }));
      return;
    }
    setLocalError(null);
    run.mutate(false);
  }

  async function apply() {
    if (!preview) return;
    if (preview.remove.length > 0) {
      const confirmed = await feedback.confirm({
        title: t("{n} var olan kayıt silinsin mi?", { n: preview.remove.length }),
        description: t("“Var olanları değiştir” açık: dosyada bulunan ad ve türlerdeki, dosyada olmayan kayıtlar silinir. Bu işlem geri alınamaz."),
        confirmLabel: t("Uygula"),
        danger: true,
      });
      if (!confirmed) return;
    }
    run.mutate(true);
  }

  const nothingToDo = preview !== null && preview.add.length === 0 && preview.remove.length === 0;

  return (
    <Modal
      open
      onClose={onClose}
      extraWide
      title={t("Bölge dosyasını içe aktar")}
      description={t("BIND biçimindeki bölge dosyasından {domain} bölgesine kayıt ekler.", { domain: zone })}
      footer={
        <>
          <Button onClick={onClose}>{applied ? t("Kapat") : t("Vazgeç")}</Button>
          {!applied && (
            <>
              <Button loading={run.isPending && !run.variables} onClick={startPreview} disabled={run.isPending}>
                {t("Önizle")}
              </Button>
              <Button variant="primary" loading={run.isPending && run.variables === true} disabled={!preview || nothingToDo || run.isPending} onClick={() => void apply()}>
                {t("Uygula")}
              </Button>
            </>
          )}
        </>
      }
    >
      <div className="flex flex-col gap-5">
        <Textarea
          label={t("Bölge dosyası")}
          value={text}
          onChange={(event) => edit(event.target.value)}
          placeholder={"$ORIGIN " + zone + ".\n$TTL 3600\n@ IN A 203.0.113.10\nwww IN CNAME @"}
          hint={t("En çok 512 KB ve 5000 kayıt. A, AAAA, CNAME, MX, TXT, NS, SRV ve CAA alınır; SOA ve öteki türler atlanır.")}
          spellCheck={false}
          className="min-h-48"
        />
        <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
          <label className="inline-flex cursor-pointer items-center gap-2 text-[13.5px] font-medium text-accent hover:underline">
            <Upload className="size-4" />
            {t("Dosyadan yükle")}
            <input type="file" accept=".zone,.txt,.db,text/plain" className="sr-only" onChange={(event) => void load(event.target.files?.[0])} />
          </label>
          <Checkbox
            label={t("Aynı ad ve türdeki var olan kayıtları değiştir")}
            hint={t("Kapalıyken var olan kayıtlara dokunulmaz, yalnızca eksikler eklenir.")}
            checked={replace}
            onChange={(event) => {
              setReplace(event.target.checked);
              setPreview(null);
              setApplied(null);
            }}
          />
        </div>

        {(localError || run.isError) && <ErrorBlock message={localError ?? errorMessage(run.error)} />}

        {preview && (
          <div className="flex flex-col gap-4">
            <div className="flex flex-wrap gap-x-6 gap-y-2">
              <Status tone="ok">{t("Eklenecek: {n}", { n: preview.add.length })}</Status>
              <Status tone="idle">{t("Zaten var: {n}", { n: preview.existing_count })}</Status>
              {preview.remove.length > 0 && <Status tone="bad">{t("Silinecek: {n}", { n: preview.remove.length })}</Status>}
              <Status tone="warn">{t("Atlanacak: {n}", { n: preview.skipped_total })}</Status>
            </div>
            {preview.notes.map((note) => (
              <p key={note} className="text-ink-2">
                {note}
              </p>
            ))}
            {nothingToDo && <p className="text-ink-2">{t("Eklenecek ya da silinecek bir şey yok.")}</p>}
            {preview.add.length > 0 && (
              <div className="flex flex-col gap-2">
                <h3 className="font-semibold">{t("Eklenecek kayıtlar")}</h3>
                <RecordList records={preview.add} zone={zone} />
                {preview.add.length > PREVIEW_ROWS && <p className="text-[13px] text-ink-3">{t("İlk {n} kayıt gösteriliyor; hepsi eklenir.", { n: PREVIEW_ROWS })}</p>}
              </div>
            )}
            {preview.remove.length > 0 && (
              <div className="flex flex-col gap-2">
                <h3 className="font-semibold text-bad">{t("Silinecek kayıtlar")}</h3>
                <RecordList records={preview.remove} zone={zone} />
              </div>
            )}
            {preview.skipped.length > 0 && (
              <div className="flex flex-col gap-2">
                <h3 className="font-semibold">{t("Atlanan satırlar")}</h3>
                <Table>
                  <thead>
                    <tr>
                      <Th>{t("Satır")}</Th>
                      <Th>{t("İçerik")}</Th>
                      <Th>{t("Neden")}</Th>
                    </tr>
                  </thead>
                  <tbody>
                    {preview.skipped.slice(0, PREVIEW_ROWS).map((item, index) => (
                      <tr key={index}>
                        <Td className="tabular whitespace-nowrap text-ink-2">{item.line}</Td>
                        <Td className="max-w-xs">
                          <p className="truncate" title={item.text}>
                            <Mono>{item.text}</Mono>
                          </p>
                        </Td>
                        <Td className="text-ink-2">{item.reason}</Td>
                      </tr>
                    ))}
                  </tbody>
                </Table>
                {preview.skipped_total > Math.min(preview.skipped.length, PREVIEW_ROWS) && (
                  <p className="text-[13px] text-ink-3">{t("Atlanan satırların bir kısmı gösteriliyor.")}</p>
                )}
              </div>
            )}
          </div>
        )}

        {applied?.result && applied.result.failed.length > 0 && (
          <div className="flex flex-col gap-2">
            <ErrorBlock message={t("{added} kayıt eklendi, {removed} silindi; bazı kayıtlar yazılamadı:", { added: applied.result.added, removed: applied.result.removed })} />
            <ul className="list-disc pl-5 text-ink-2">
              {applied.result.failed.map((line, index) => (
                <li key={index}>{line}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Modal>
  );
}

// ---- Çalışma alanı ----

function ZoneWorkspace({ detail }: { detail: ZoneDetail }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const { zone } = detail;
  const isLocal = zone.provider === "local";
  const [recordModal, setRecordModal] = useState<{ record: DnsRecord | null } | null>(null);
  const [report, setReport] = useState<string[] | null>(null);
  const [importing, setImporting] = useState(false);

  const exportZone = useMutation({
    mutationFn: () => api.get<{ filename: string; content: string }>(`/dns/zones/${zone.id}/export`),
    onSuccess: (file) => {
      const url = URL.createObjectURL(new Blob([file.content], { type: "text/plain;charset=utf-8" }));
      const link = document.createElement("a");
      link.href = url;
      link.download = file.filename;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
    },
    onError: (error) => feedback.error(error),
  });

  const sync = useMutation({
    mutationFn: () => api.post<{ report: string[] }>(`/dns/zones/${zone.id}/sync`),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: detailKey(zone.id) });
      setReport(result.report);
    },
    onError: (error) => feedback.error(error),
  });

  const removeZone = useMutation({
    mutationFn: () => api.delete(`/dns/zones/${zone.id}`),
    onSuccess: () => {
      queryClient.removeQueries({ queryKey: detailKey(zone.id) });
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      void queryClient.invalidateQueries({ queryKey: ["dns-cloudflare"] });
      feedback.success(isLocal ? t("{domain} bölgesi silindi", { domain: zone.domain }) : t("{domain} bölgesinin bağlantısı kaldırıldı", { domain: zone.domain }));
      void navigate("/dns");
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove() {
    const confirmed = await feedback.confirm({
      title: isLocal ? t("{domain} bölgesi silinsin mi?", { domain: zone.domain }) : t("{domain} bağlantısı kaldırılsın mı?", { domain: zone.domain }),
      description: isLocal
        ? t("Bölge ve içindeki bütün kayıtlar bu sunucudan silinir. Alan adının ad sunucuları hâlâ bu sunucuyu gösteriyorsa siteleriniz ve postanız erişilemez olur. Bu işlem geri alınamaz.")
        : t("Bölge yalnızca panelden kaldırılır; Cloudflare'deki bölge ve kayıtlar silinmez. Bağlantıyı daha sonra yeniden kurabilirsiniz."),
      confirmLabel: isLocal ? t("Bölgeyi sil") : t("Bağlantıyı kaldır"),
      danger: true,
      typeToConfirm: isLocal ? zone.domain : undefined,
    });
    if (confirmed) removeZone.mutate();
  }

  return (
    <>
      <Link to="/dns" className="mb-3 inline-flex items-center gap-1.5 text-[13.5px] font-medium text-ink-2 hover:text-ink">
        <ArrowLeft className="size-3.5" /> {t("DNS")}
      </Link>
      <header className="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
            <h1 className="text-[23px] leading-[1.15] font-semibold tracking-[-0.02em] break-all sm:text-[26px]">
              <span className="font-mono font-medium">{zone.domain}</span>
            </h1>
            <Badge tone={isLocal ? "idle" : "accent"}>{t(providerLabels[zone.provider])}</Badge>
          </div>
          <p className="mt-1.5 max-w-[42rem] text-ink-2">{t("Alan adının sitelere, postaya ve öteki hizmetlere nasıl yönleneceğini buradaki kayıtlar belirler.")}</p>
        </div>
        <Button variant="danger" loading={removeZone.isPending} onClick={() => void confirmRemove()}>
          {isLocal ? t("Bölgeyi sil") : t("Bağlantıyı kaldır")}
        </Button>
      </header>

      <AdvicePanel advice={zoneAdvice(detail)} />
      {detail.cloudflare && <CloudflareStrip zoneId={zone.id} info={detail.cloudflare} />}

      <RecordsPanel
        detail={detail}
        onAdd={() => setRecordModal({ record: null })}
        onEdit={(record) => setRecordModal({ record })}
        tools={
          <>
            <Button loading={sync.isPending} icon={<Wand2 className="size-3.5" />} onClick={() => sync.mutate()}>
              {t("Site ve posta kayıtlarını ekle")}
            </Button>
            <Button icon={<Upload className="size-3.5" />} onClick={() => setImporting(true)}>
              {t("İçe aktar")}
            </Button>
            <Button icon={<Download className="size-3.5" />} loading={exportZone.isPending} onClick={() => exportZone.mutate()}>
              {t("Dışa aktar")}
            </Button>
          </>
        }
      />

      <div className="mt-5 flex flex-col">
        {isLocal && detail.nameservers && <NameserversPanel nameservers={detail.nameservers} serverIp={detail.server_ip} />}
        {isLocal && detail.dnssec && <DnssecPanel zone={zone} info={detail.dnssec} />}
      </div>

      {recordModal && <RecordModal detail={detail} record={recordModal.record} onClose={() => setRecordModal(null)} />}
      {report && <SyncReportModal report={report} onClose={() => setReport(null)} />}
      {importing && <ImportModal detail={detail} onClose={() => setImporting(false)} />}
    </>
  );
}

// ---- Sayfa ----

export function DnsZonePage() {
  const { id } = useParams();
  const detail = useQuery({ queryKey: detailKey(id ?? ""), queryFn: () => api.get<ZoneDetail>(`/dns/zones/${id}`), enabled: Boolean(id) });

  if (detail.isPending) return <LoadingBlock />;
  if (detail.isError) return <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />;

  return <ZoneWorkspace detail={detail.data} />;
}
