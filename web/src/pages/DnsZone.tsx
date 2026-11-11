// DNS bölgesi çalışma alanı: kayıt tablosu, kayıt ekleme/düzenleme, ad sunucuları,
// Cloudflare ayarları ve site/posta kayıtlarının otomatik eklenmesi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, Copy, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
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

interface ZoneDetail {
  zone: Zone;
  records: DnsRecord[];
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
function CopyValue({ value }: { value: string }) {
  const [copied, copy] = useCopy();
  return (
    <div className="flex min-w-0 items-center gap-1">
      <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-sunken/60 px-3 py-1.5 font-mono text-[13px] select-all">{value}</code>
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

function RecordsPanel({
  detail,
  onAdd,
  onEdit,
}: {
  detail: ZoneDetail;
  onAdd: () => void;
  onEdit: (record: DnsRecord) => void;
}) {
  const feedback = useFeedback();
  const updateRecords = useRecordsCache(detail.zone.id);
  const zone = detail.zone.domain;
  const cloudflare = detail.cloudflare;
  const [search, setSearch] = useState("");
  const [typeFilter, setTypeFilter] = useState("");

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
        if (a.short === "@" && b.short !== "@") return -1;
        if (b.short === "@" && a.short !== "@") return 1;
        return a.short.localeCompare(b.short) || typeOrder.indexOf(a.record.type) - typeOrder.indexOf(b.record.type) || a.record.content.localeCompare(b.record.content);
      });
  }, [detail.records, search, typeFilter, zone]);

  const total = detail.records.length;

  return (
    <Panel
      flush
      title={t("DNS kayıtları")}
      actions={
        <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={onAdd}>
          {t("Kayıt ekle")}
        </Button>
      }
    >
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
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2 border-b border-line px-4 sm:px-6 py-2">
            <SearchInput
              aria-label={t("Kayıtlarda ara")}
              placeholder={t("Ad ya da değer ara")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className="w-64"
            />
            <select
              aria-label={t("Türe göre süz")}
              value={typeFilter}
              onChange={(event) => setTypeFilter(event.target.value)}
              className="h-8 rounded-md border border-line-strong bg-surface px-2.5 pr-7 text-ink focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30"
            >
              <option value="">{t("Tümü")}</option>
              {presentTypes.map((type) => (
                <option key={type} value={type}>
                  {type}
                </option>
              ))}
            </select>
            <p className="tabular ml-auto text-[13.5px] text-ink-2">
              {rows.length === total ? t("{n} kayıt", { n: total }) : t("{shown} / {total} kayıt", { shown: rows.length, total })}
            </p>
          </div>
          {rows.length === 0 ? (
            <EmptyState title={t("Aramanızla eşleşen kayıt yok")} />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Tür")}</Th>
                  <Th>{t("Ad")}</Th>
                  <Th>{t("Değer")}</Th>
                  <Th>{t("TTL")}</Th>
                  {cloudflare && <Th>{t("Vekil")}</Th>}
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
                      <Td className="whitespace-nowrap">
                        <Badge tone="accent">{record.type}</Badge>
                      </Td>
                      <Td className="max-w-56">
                        <p className="truncate" title={record.name}>
                          <Mono className="text-[13.5px] font-medium">{short}</Mono>
                        </p>
                      </Td>
                      <Td className="max-w-md">
                        <p className="truncate" title={record.content}>
                          {record.priority !== null && <span className="tabular mr-1.5 text-[12.5px] text-ink-3">{record.priority}</span>}
                          <Mono>{record.content}</Mono>
                        </p>
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2">{formatTtl(record.ttl)}</Td>
                      {cloudflare && (
                        <Td className="whitespace-nowrap">
                          {proxyableTypes.includes(record.type) ? (
                            record.proxied ? <Status tone="accent">{t("Cloudflare üzerinden")}</Status> : <Status tone="idle">{t("Yalnızca DNS")}</Status>
                          ) : (
                            <span className="text-ink-3">—</span>
                          )}
                        </Td>
                      )}
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
          )}
        </>
      )}
    </Panel>
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
      <header className="mb-5 flex flex-wrap items-center justify-between gap-x-6 gap-y-3">
        <div className="flex min-w-0 flex-wrap items-center gap-x-4 gap-y-2">
          <h1 className="font-mono text-[22px] leading-tight font-medium tracking-[-0.01em] break-all">{zone.domain}</h1>
          <Badge tone={isLocal ? "idle" : "accent"}>{t(providerLabels[zone.provider])}</Badge>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setRecordModal({ record: null })}>
            {t("Kayıt ekle")}
          </Button>
          <Button loading={sync.isPending} onClick={() => sync.mutate()}>
            {t("Site ve posta kayıtlarını ekle")}
          </Button>
          <Button variant="danger" loading={removeZone.isPending} onClick={() => void confirmRemove()}>
            {isLocal ? t("Bölgeyi sil") : t("Bağlantıyı kaldır")}
          </Button>
        </div>
      </header>

      {isLocal && detail.nameservers && <NameserversPanel nameservers={detail.nameservers} serverIp={detail.server_ip} />}
      {detail.cloudflare && <CloudflareStrip zoneId={zone.id} info={detail.cloudflare} />}

      <RecordsPanel detail={detail} onAdd={() => setRecordModal({ record: null })} onEdit={(record) => setRecordModal({ record })} />

      {recordModal && <RecordModal detail={detail} record={recordModal.record} onClose={() => setRecordModal(null)} />}
      {report && <SyncReportModal report={report} onClose={() => setReport(null)} />}
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
