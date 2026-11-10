// Redis sayfası: sunucu geneli önbellek/kuyruk sunucusunun durumu, anahtar gezgini ve ayarları.

import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Eye, EyeOff, Search, Trash2 } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useState } from "react";
import { Link, useSearchParams } from "react-router";
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
  Meter,
  Modal,
  Mono,
  PageHeader,
  Panel,
  Select,
  Status,
  Table,
  Td,
  Th,
  Tabs,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDuration, formatPercent, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Türler ----

type RedisType = "string" | "list" | "set" | "zset" | "hash" | "stream";

interface KeyspaceEntry {
  db: number;
  keys: number;
  expires: number;
}

interface RedisConfig {
  maxmemory_mb: number;
  policy: string;
  password_set: boolean;
  appendonly: boolean;
  snapshots: boolean;
}

interface RedisOff {
  installed: boolean;
  running: false;
}

interface RedisOn {
  installed: true;
  running: true;
  version: string;
  uptime_secs: number;
  used_memory: number;
  used_memory_peak: number;
  used_memory_rss: number;
  connected_clients: number;
  ops_per_sec: number;
  total_commands: number;
  /** Yüzde olarak; henüz okuma yapılmadıysa null. */
  hit_rate: number | null;
  evicted_keys: number;
  expired_keys: number;
  last_save_at: number;
  keyspace: KeyspaceEntry[];
  databases: number;
  config: RedisConfig;
  policies: string[];
}

type RedisStatus = RedisOff | RedisOn;

interface KeyRow {
  key: string;
  type: RedisType;
  ttl: number;
  size: number;
}

interface KeysPage {
  cursor: string;
  keys: KeyRow[];
}

interface KeyContent {
  value?: { text: string; truncated: boolean; bytes: number };
  items?: string[];
  pairs?: { member: string; score: string }[];
  fields?: { field: string; value: string }[];
}

interface KeyDetail {
  key: string;
  type: RedisType;
  ttl: number;
  length: number;
  content: KeyContent;
}

interface ConfigRequest {
  maxmemory_mb: number;
  policy: string;
  appendonly: boolean;
  password?: string;
}

const TAB_IDS = ["status", "keys", "settings"] as const;
type TabId = (typeof TAB_IDS)[number];

/** Adres çubuğundaki `?sekme=` değerleri. */
const TAB_SLUGS: Record<TabId, string> = {
  status: "durum",
  keys: "anahtarlar",
  settings: "ayarlar",
};

const LINK_BUTTON_CLASS =
  "inline-flex h-9 items-center justify-center rounded-md border border-transparent bg-accent px-3.5 font-medium text-accent-ink transition-colors hover:bg-accent-hover";

const PASSWORD_ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const PASSWORD_PATTERN = /^[A-Za-z0-9_\-+=.@:!]+$/;
const MIN_PASSWORD_LENGTH = 16;

const numberFormat = new Intl.NumberFormat("tr-TR");

// ---- Yardımcılar ----

function formatNumber(value: number): string {
  return numberFormat.format(value);
}

function policyLabel(policy: string): string {
  switch (policy) {
    case "noeviction":
      return t("Yeni yazmaları reddet");
    case "allkeys-lru":
      return t("En uzun süredir kullanılmayanı sil");
    case "allkeys-lfu":
      return t("En az kullanılanı sil");
    case "allkeys-random":
      return t("Rastgele sil");
    case "volatile-lru":
      return t("En uzun süredir kullanılmayanı sil (yalnızca süreli anahtarlar)");
    case "volatile-lfu":
      return t("En az kullanılanı sil (yalnızca süreli anahtarlar)");
    case "volatile-random":
      return t("Rastgele sil (yalnızca süreli anahtarlar)");
    case "volatile-ttl":
      return t("Süresi en yakın olanı sil");
    default:
      return policy;
  }
}

function formatTtl(ttl: number): string {
  return ttl < 0 ? t("Süresiz") : formatDuration(ttl);
}

/** Tarayıcının rastgele sayı üretecinden, harf ve rakamlardan oluşan parola üretir. */
function generatePassword(length: number): string {
  // Kalan-bölme yanlılığını önlemek için alfabe boyunun katını aşan değerler atılır.
  const limit = Math.floor(0x100000000 / PASSWORD_ALPHABET.length) * PASSWORD_ALPHABET.length;
  const buffer = new Uint32Array(1);
  let result = "";
  while (result.length < length) {
    crypto.getRandomValues(buffer);
    const value = buffer[0];
    if (value !== undefined && value < limit) result += PASSWORD_ALPHABET.charAt(value % PASSWORD_ALPHABET.length);
  }
  return result;
}

/** Boşaltma işlemi: onay penceresi, istek ve önbellek yenileme tek yerde. */
function useFlush() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: (db: number | undefined) => api.post<{ ok: boolean }>("/redis/flush", db === undefined ? {} : { db }),
    onSuccess: (_result, db) => {
      void queryClient.invalidateQueries({ queryKey: ["redis"] });
      feedback.success(db === undefined ? t("Bütün veritabanları boşaltıldı.") : t("db{n} boşaltıldı.", { n: db }));
    },
    onError: (error) => feedback.error(error),
  });

  async function flush(db?: number) {
    const confirmed = await feedback.confirm(
      db === undefined
        ? {
            title: t("Bütün veritabanları boşaltılsın mı?"),
            description: t("Redis'teki bütün anahtarlar silinir. Sunucudaki uygulamaların önbelleği, oturumları ve kuyrukları da buna dahildir. Bu işlem geri alınamaz."),
            confirmLabel: t("Hepsini boşalt"),
            danger: true,
            typeToConfirm: "hepsini sil",
          }
        : {
            title: t("db{n} boşaltılsın mı?", { n: db }),
            description: t("Bu veritabanındaki bütün anahtarlar silinir. Bu işlem geri alınamaz."),
            confirmLabel: t("Veritabanını boşalt"),
            danger: true,
            typeToConfirm: `db${db}`,
          },
    );
    if (confirmed) mutation.mutate(db);
  }

  return { flush, pending: mutation.isPending };
}

// ---- Sayfa ----

export function RedisPage() {
  const [searchParams, setSearchParams] = useSearchParams();

  const info = useQuery({
    queryKey: ["redis", "info"],
    queryFn: () => api.get<RedisStatus>("/redis"),
    refetchInterval: 5000,
  });

  const requested = searchParams.get("sekme");
  const tab: TabId = TAB_IDS.find((id) => TAB_SLUGS[id] === requested) ?? "status";

  const tabs: { id: TabId; label: string }[] = [
    { id: "status", label: t("Durum") },
    { id: "keys", label: t("Anahtarlar") },
    { id: "settings", label: t("Ayarlar") },
  ];

  function selectTab(next: TabId) {
    const params = new URLSearchParams(searchParams);
    params.set("sekme", TAB_SLUGS[next]);
    setSearchParams(params, { replace: true });
  }

  /** Durum sekmesinden bir veritabanının anahtarlarına geçer. */
  function browseDatabase(db: number) {
    const params = new URLSearchParams(searchParams);
    params.set("sekme", TAB_SLUGS.keys);
    params.set("db", String(db));
    params.delete("kalip");
    setSearchParams(params, { replace: true });
  }

  const description = t("Redis, bu sunucudaki bütün uygulamaların ortak kullandığı bellek içi önbellek ve kuyruk sunucusudur.");

  let body: ReactNode;
  const data = info.data;
  if (!data) {
    body = info.isError ? <ErrorBlock message={errorMessage(info.error)} onRetry={() => void info.refetch()} /> : <LoadingBlock />;
  } else if (!data.installed) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Redis kurulu değil")}
          description={t("Bileşenler sayfasından kurun.")}
          action={
            <Link to="/components" className={LINK_BUTTON_CLASS}>
              {t("Bileşenlere git")}
            </Link>
          }
        />
      </Panel>
    );
  } else if (!data.running) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Redis servisi durmuş")}
          description={t("Redis kurulu ama servisi çalışmıyor. Servisler sayfasından redis servisini başlatın.")}
          action={
            <Link to="/services" className={LINK_BUTTON_CLASS}>
              {t("Servislere git")}
            </Link>
          }
        />
      </Panel>
    );
  } else {
    body = (
      <>
        <p className="mb-4 rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-warn">
          {t("Redis'te hesaplar arası yalıtım yoktur; buradaki veriler sunucudaki bütün uygulamalara aittir.")}
        </p>
        <Tabs tabs={tabs} value={tab} onChange={selectTab} label={t("Redis bölümleri")} />
        {tab === "status" && <StatusTab info={data} onBrowse={browseDatabase} />}
        {tab === "keys" && <KeysTab info={data} />}
        {tab === "settings" && <SettingsTab info={data} />}
      </>
    );
  }

  return (
    <>
      <PageHeader title="Redis" description={description} />
      {body}
    </>
  );
}

// ---- Durum sekmesi ----

function Stat({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="min-w-0">
      <dt className="text-[12.5px] text-ink-2">{label}</dt>
      <dd className="mt-0.5 font-medium break-words">{children}</dd>
    </div>
  );
}

function StatusTab({ info, onBrowse }: { info: RedisOn; onBrowse: (db: number) => void }) {
  const limitBytes = info.config.maxmemory_mb * 1024 * 1024;
  const hasLimit = info.config.maxmemory_mb > 0;
  const usedPercent = hasLimit ? (info.used_memory / limitBytes) * 100 : 0;
  const keyspace = [...info.keyspace].sort((a, b) => a.db - b.db);

  return (
    <div className="flex flex-col gap-4">
      <Panel title={t("Genel bakış")}>
        <dl className="grid grid-cols-2 gap-x-6 gap-y-4 sm:grid-cols-3 lg:grid-cols-5">
          <Stat label={t("Sürüm")}>
            <Mono>{info.version}</Mono>
          </Stat>
          <Stat label={t("Çalışma süresi")}>{formatDuration(info.uptime_secs)}</Stat>
          <Stat label={t("Bağlı istemci")}>{formatNumber(info.connected_clients)}</Stat>
          <Stat label={t("Saniyedeki işlem")}>{formatNumber(info.ops_per_sec)}</Stat>
          <Stat label={t("Toplam işlem")}>{formatNumber(info.total_commands)}</Stat>
          <Stat label={t("İsabet oranı")}>{info.hit_rate === null ? "—" : formatPercent(info.hit_rate, 1)}</Stat>
          <Stat label={t("Süresi dolan anahtar")}>{formatNumber(info.expired_keys)}</Stat>
          <Stat label={t("Bellekten atılan anahtar")}>{formatNumber(info.evicted_keys)}</Stat>
          <Stat label={t("Son disk kaydı")}>{formatRelative(info.last_save_at)}</Stat>
        </dl>
      </Panel>

      <Panel title={t("Bellek")}>
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
            <p>
              <Mono className="text-[14px] font-medium">{formatBytes(info.used_memory)}</Mono>
              <span className="text-ink-2"> / </span>
              {hasLimit ? <Mono className="text-[14px]">{formatBytes(limitBytes)}</Mono> : <span className="text-ink-2">{t("Sınır yok")}</span>}
            </p>
            {hasLimit && <span className="text-ink-2">{t("{percent} dolu", { percent: formatPercent(usedPercent, 1) })}</span>}
          </div>
          {hasLimit && <Meter value={usedPercent} label={t("Redis bellek doluluğu")} />}
          <dl className="grid grid-cols-2 gap-x-6 gap-y-4 sm:grid-cols-3">
            <Stat label={t("En yüksek kullanım")}>
              <Mono>{formatBytes(info.used_memory_peak)}</Mono>
            </Stat>
            <Stat label={t("İşletim sisteminden alınan (RSS)")}>
              <Mono>{formatBytes(info.used_memory_rss)}</Mono>
            </Stat>
          </dl>
        </div>
      </Panel>

      <Panel title={t("Veritabanları")} flush>
        {keyspace.length === 0 ? (
          <p className="px-4 py-4 text-ink-2">{t("Hiçbir veritabanında anahtar saklanmıyor.")}</p>
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Veritabanı")}</Th>
                <Th align="right">{t("Anahtar sayısı")}</Th>
                <Th align="right">{t("Süreli anahtar sayısı")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {keyspace.map((entry) => (
                <tr key={entry.db}>
                  <Td>
                    <Mono>{`db${entry.db}`}</Mono>
                  </Td>
                  <Td align="right">{formatNumber(entry.keys)}</Td>
                  <Td align="right">{formatNumber(entry.expires)}</Td>
                  <Td align="right">
                    <Button size="sm" onClick={() => onBrowse(entry.db)}>
                      {t("Anahtarlara göz at")}
                    </Button>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
    </div>
  );
}

// ---- Anahtarlar sekmesi ----

function KeysTab({ info }: { info: RedisOn }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const { flush, pending: flushing } = useFlush();

  const rawDb = Number(searchParams.get("db") ?? "0");
  const db = Number.isInteger(rawDb) && rawDb >= 0 && rawDb < info.databases ? rawDb : 0;
  const pattern = searchParams.get("kalip") ?? "";
  const [patternInput, setPatternInput] = useState(pattern);
  const [viewKey, setViewKey] = useState<string | null>(null);

  const keys = useInfiniteQuery({
    queryKey: ["redis", "keys", db, pattern],
    queryFn: ({ pageParam }) =>
      api.get<KeysPage>(`/redis/keys?db=${db}&pattern=${encodeURIComponent(pattern || "*")}&cursor=${encodeURIComponent(pageParam)}`),
    initialPageParam: "0",
    getNextPageParam: (last) => (last.cursor !== "0" ? last.cursor : undefined),
    refetchOnWindowFocus: false,
  });

  const remove = useMutation({
    mutationFn: (key: string) => api.delete<{ removed: number }>(`/redis/key?db=${db}&key=${encodeURIComponent(key)}`),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
      void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
    },
    onError: (error) => feedback.error(error),
  });

  function updateParams(patch: { db?: number; kalip?: string }) {
    const params = new URLSearchParams(searchParams);
    if (patch.db !== undefined) params.set("db", String(patch.db));
    if (patch.kalip !== undefined) {
      if (patch.kalip) params.set("kalip", patch.kalip);
      else params.delete("kalip");
    }
    setSearchParams(params, { replace: true });
  }

  function applyPattern(event: FormEvent) {
    event.preventDefault();
    updateParams({ kalip: patternInput.trim() });
  }

  async function removeKey(key: string, onDone?: () => void) {
    const confirmed = await feedback.confirm({
      title: t("Anahtar silinsin mi?"),
      description: t("“{key}” anahtarı db{db} içinden silinir. Bu anahtarı kullanan uygulamalar etkilenebilir.", { key, db }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(key, { onSuccess: onDone });
  }

  // Aynı anahtar SCAN sayfaları arasında tekrar gelebilir; yinelenenler atılır.
  const rows: KeyRow[] = [];
  const seen = new Set<string>();
  for (const page of keys.data?.pages ?? []) {
    for (const row of page.keys) {
      if (seen.has(row.key)) continue;
      seen.add(row.key);
      rows.push(row);
    }
  }

  const keyCount = (n: number) => info.keyspace.find((entry) => entry.db === n)?.keys;
  const dbOptions = Array.from({ length: Math.max(1, info.databases) }, (_unused, n) => n);

  return (
    <>
      <Panel flush>
        <div className="flex flex-col gap-2 border-b border-line px-3 py-3">
          <div className="flex flex-wrap items-end gap-3">
            <div className="w-56">
              <Select
                label={t("Veritabanı")}
                value={db}
                onChange={(event) => {
                  updateParams({ db: Number(event.target.value) });
                }}
              >
                {dbOptions.map((n) => {
                  const count = keyCount(n);
                  return (
                    <option key={n} value={n}>
                      {count === undefined ? `db${n}` : t("db{n} ({keys} anahtar)", { n, keys: formatNumber(count) })}
                    </option>
                  );
                })}
              </Select>
            </div>
            <form onSubmit={applyPattern} className="flex items-end gap-2">
              <div className="w-64">
                <Input
                  label={t("Kalıp")}
                  placeholder="oturum:*"
                  value={patternInput}
                  spellCheck={false}
                  autoComplete="off"
                  className="font-mono text-[13px]"
                  onChange={(event) => setPatternInput(event.target.value)}
                />
              </div>
              <Button type="submit" icon={<Search className="size-4" />}>
                {t("Ara")}
              </Button>
            </form>
            <div className="ml-auto">
              <Button variant="secondary" loading={flushing} className="text-bad" onClick={() => void flush(db)}>
                {t("Bu veritabanını boşalt")}
              </Button>
            </div>
          </div>
          <p className="text-[12.5px] text-ink-3">{t("* her şeye uyar")}</p>
        </div>

        {keys.isPending && <LoadingBlock />}
        {keys.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(keys.error)} onRetry={() => void keys.refetch()} />
          </div>
        )}
        {keys.data && rows.length === 0 && !keys.hasNextPage && (
          <EmptyState title={t("Bu veritabanında bu kalıba uyan anahtar yok")} />
        )}
        {keys.data && rows.length === 0 && keys.hasNextPage && (
          <p className="px-4 py-4 text-ink-2">{t("Şu ana kadar taranan kısımda eşleşen anahtar çıkmadı; tarama sürüyor.")}</p>
        )}
        {rows.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("Anahtar")}</Th>
                <Th>{t("Tür")}</Th>
                <Th>{t("Kalan süre")}</Th>
                <Th align="right">{t("Boyut")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={row.key}>
                  <Td className="max-w-md min-w-40">
                    <Mono className="block truncate">
                      <span title={row.key}>{row.key}</span>
                    </Mono>
                  </Td>
                  <Td>
                    <Badge>{row.type}</Badge>
                  </Td>
                  <Td>{formatTtl(row.ttl)}</Td>
                  <Td align="right">{formatBytes(row.size)}</Td>
                  <Td align="right">
                    <div className="flex justify-end gap-0.5">
                      <IconButton label={t("Görüntüle")} onClick={() => setViewKey(row.key)}>
                        <Eye className="size-4" />
                      </IconButton>
                      <IconButton
                        label={t("Sil")}
                        className="hover:text-bad"
                        disabled={remove.isPending && remove.variables === row.key}
                        onClick={() => void removeKey(row.key)}
                      >
                        <Trash2 className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
        {keys.data && (rows.length > 0 || keys.hasNextPage) && (
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            {keys.hasNextPage && (
              <Button size="sm" loading={keys.isFetchingNextPage} onClick={() => void keys.fetchNextPage()}>
                {t("Daha fazla yükle")}
              </Button>
            )}
            <span className="text-[12.5px] text-ink-3">
              {keys.hasNextPage ? t("{n} anahtar listelendi; tarama sürüyor", { n: formatNumber(rows.length) }) : t("{n} anahtar listelendi", { n: formatNumber(rows.length) })}
            </span>
          </div>
        )}
      </Panel>

      {viewKey !== null && (
        <KeyViewerModal
          db={db}
          keyName={viewKey}
          onClose={() => setViewKey(null)}
          onRemove={() => void removeKey(viewKey, () => setViewKey(null))}
        />
      )}
    </>
  );
}

function KeyViewerModal({ db, keyName, onClose, onRemove }: { db: number; keyName: string; onClose: () => void; onRemove: () => void }) {
  const detail = useQuery({
    queryKey: ["redis", "key", db, keyName],
    queryFn: () => api.get<KeyDetail>(`/redis/key?db=${db}&key=${encodeURIComponent(keyName)}`),
    refetchOnWindowFocus: false,
  });

  return (
    <Modal
      open
      wide
      onClose={onClose}
      title={t("Anahtar: db{n}", { n: db })}
      footer={
        <>
          <Button onClick={onClose}>{t("Kapat")}</Button>
          <Button variant="danger" onClick={onRemove}>
            {t("Sil")}
          </Button>
        </>
      }
    >
      {detail.isPending && <LoadingBlock />}
      {detail.isError && <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />}
      {detail.data && <KeyDetailView detail={detail.data} />}
    </Modal>
  );
}

function KeyDetailView({ detail }: { detail: KeyDetail }) {
  const content = detail.content;
  const isString = detail.type === "string";

  // Gösterilen öğe sayısı; gerçek sayıdan azsa kullanıcıya not düşülür.
  let shown = 0;
  if (detail.type === "list" || detail.type === "set") shown = content.items?.length ?? 0;
  else if (detail.type === "zset") shown = content.pairs?.length ?? 0;
  else if (detail.type === "hash") shown = content.fields?.length ?? 0;
  const partial = !isString && detail.type !== "stream" && detail.length > shown;

  return (
    <div className="flex flex-col gap-4">
      <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-4 gap-y-2">
        <dt className="text-[13px] text-ink-2">{t("Anahtar")}</dt>
        <dd className="min-w-0">
          <Mono className="break-all">{detail.key}</Mono>
        </dd>
        <dt className="text-[13px] text-ink-2">{t("Tür")}</dt>
        <dd>
          <Badge>{detail.type}</Badge>
        </dd>
        <dt className="text-[13px] text-ink-2">{t("Kalan süre")}</dt>
        <dd>{formatTtl(detail.ttl)}</dd>
        <dt className="text-[13px] text-ink-2">{isString ? t("Boyut") : t("Öğe sayısı")}</dt>
        <dd>{isString ? formatBytes(detail.length) : formatNumber(detail.length)}</dd>
      </dl>

      {isString && content.value && (
        <div className="flex flex-col gap-2">
          <pre className="max-h-[45vh] overflow-auto rounded-md border border-line bg-sunken/50 p-3 font-mono text-[12.5px] break-all whitespace-pre-wrap">
            {content.value.text}
          </pre>
          {content.value.truncated && (
            <p className="text-[12.5px] text-warn">{t("Değer büyük olduğu için kısaltıldı; toplam {size}.", { size: formatBytes(detail.length) })}</p>
          )}
        </div>
      )}

      {(detail.type === "list" || detail.type === "set") && content.items && (
        <ol className="flex max-h-[45vh] flex-col gap-1 overflow-auto rounded-md border border-line bg-sunken/50 p-3">
          {content.items.map((item, index) => (
            <li key={index} className="flex gap-3">
              <Mono className="w-10 shrink-0 text-right text-ink-3">{index + 1}</Mono>
              <Mono className="min-w-0 break-all whitespace-pre-wrap">{item}</Mono>
            </li>
          ))}
        </ol>
      )}

      {detail.type === "zset" && content.pairs && (
        <div className="max-h-[45vh] overflow-auto rounded-md border border-line">
          <Table>
            <thead>
              <tr>
                <Th>{t("Üye")}</Th>
                <Th align="right">{t("Skor")}</Th>
              </tr>
            </thead>
            <tbody>
              {content.pairs.map((pair, index) => (
                <tr key={index}>
                  <Td>
                    <Mono className="break-all whitespace-pre-wrap">{pair.member}</Mono>
                  </Td>
                  <Td align="right">
                    <Mono>{pair.score}</Mono>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        </div>
      )}

      {detail.type === "hash" && content.fields && (
        <div className="max-h-[45vh] overflow-auto rounded-md border border-line">
          <Table>
            <thead>
              <tr>
                <Th>{t("Alan")}</Th>
                <Th>{t("Değer")}</Th>
              </tr>
            </thead>
            <tbody>
              {content.fields.map((field, index) => (
                <tr key={index}>
                  <Td className="align-top">
                    <Mono className="break-all">{field.field}</Mono>
                  </Td>
                  <Td>
                    <Mono className="break-all whitespace-pre-wrap">{field.value}</Mono>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        </div>
      )}

      {detail.type === "stream" && (
        <p className="text-ink-2">{t("Bu akışta {n} kayıt var. Akış içerikleri burada gösterilmez.", { n: formatNumber(detail.length) })}</p>
      )}

      {partial && <p className="text-[12.5px] text-ink-3">{t("İlk {n} öğe gösteriliyor.", { n: formatNumber(shown) })}</p>}
    </div>
  );
}

// ---- Ayarlar sekmesi ----

function SettingsTab({ info }: { info: RedisOn }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const { flush, pending: flushing } = useFlush();
  const config = info.config;

  const [maxMemory, setMaxMemory] = useState(String(config.maxmemory_mb));
  const [policy, setPolicy] = useState(config.policy);
  const [appendOnly, setAppendOnly] = useState(config.appendonly);
  const [password, setPassword] = useState("");
  const [revealPassword, setRevealPassword] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (body: ConfigRequest) => api.post<{ ok: boolean }>("/redis/config", body),
    onSuccess: (_result, body) => {
      void queryClient.invalidateQueries({ queryKey: ["redis"] });
      setPassword("");
      setRevealPassword(false);
      feedback.success(body.password === "" ? t("Parola kaldırıldı.") : t("Redis ayarları kaydedildi."));
    },
  });

  const policies = info.policies.includes(policy) ? info.policies : [policy, ...info.policies];

  function submit(event: FormEvent) {
    event.preventDefault();
    setFormError(null);

    const trimmed = maxMemory.trim();
    const mb = Number(trimmed);
    if (trimmed === "" || !Number.isInteger(mb) || mb < 0) {
      setFormError(t("Bellek sınırı 0 ya da pozitif bir tam sayı (MB) olmalı."));
      return;
    }
    if (password !== "") {
      if (password.length < MIN_PASSWORD_LENGTH) {
        setFormError(t("Parola en az {n} karakter olmalı.", { n: MIN_PASSWORD_LENGTH }));
        return;
      }
      if (!PASSWORD_PATTERN.test(password)) {
        setFormError(t("Parola yalnızca harf, rakam ve _ - + = . @ : ! karakterlerini içerebilir."));
        return;
      }
    }

    const body: ConfigRequest = { maxmemory_mb: mb, policy, appendonly: appendOnly };
    if (password !== "") body.password = password;
    save.mutate(body);
  }

  async function removePassword() {
    const confirmed = await feedback.confirm({
      title: t("Redis parolası kaldırılsın mı?"),
      description: t("Parola kalktıktan sonra bu sunucudaki her süreç Redis'e parolasız bağlanabilir. Uygulamalarınızın ayarlarındaki parola alanı artık gerekmez."),
      confirmLabel: t("Parolayı kaldır"),
      danger: true,
    });
    if (!confirmed) return;
    setFormError(null);
    // Kaydedilmiş değerler gönderilir; formdaki henüz kaydedilmemiş düzenlemeler bu işleme karışmaz.
    save.mutate({ maxmemory_mb: config.maxmemory_mb, policy: config.policy, appendonly: config.appendonly, password: "" });
  }

  async function copyPassword() {
    try {
      await navigator.clipboard.writeText(password);
      feedback.success(t("Parola panoya kopyalandı."));
    } catch {
      feedback.error(new Error(t("Panoya kopyalanamadı. Parolayı elle seçip kopyalayın.")));
    }
  }

  const problem = formError ?? (save.isError ? errorMessage(save.error) : null);

  return (
    <div className="flex flex-col gap-4">
      <form onSubmit={submit} className="flex flex-col gap-4">
        <Panel title={t("Bellek ve kalıcılık")}>
          <div className="flex max-w-xl flex-col gap-4">
            <Input
              label={t("Bellek sınırı (MB)")}
              hint={t("0 = sınırsız. Sunucunun toplam belleğinden küçük bir değer verin.")}
              type="number"
              min={0}
              inputMode="numeric"
              value={maxMemory}
              onChange={(event) => setMaxMemory(event.target.value)}
            />
            <Select label={t("Bellek dolunca")} value={policy} onChange={(event) => setPolicy(event.target.value)}>
              {policies.map((item) => (
                <option key={item} value={item}>
                  {policyLabel(item)}
                </option>
              ))}
            </Select>
            <Checkbox
              label={t("Her yazmayı diske kaydet (AOF)")}
              hint={t("Veri kaybını azaltır, ancak diski daha çok kullanır.")}
              checked={appendOnly}
              onChange={(event) => setAppendOnly(event.target.checked)}
            />
          </div>
        </Panel>

        <Panel
          title={t("Parola")}
          actions={config.password_set ? <Status tone="ok">{t("Parola ayarlı")}</Status> : <Status tone="idle">{t("Parola yok")}</Status>}
        >
          <div className="flex max-w-xl flex-col gap-4">
            <p className="rounded-md border border-warn/40 bg-warn-soft px-3 py-2 text-warn">
              {t("Parola koyduğunuzda Redis kullanan uygulamaların ayarlarına da aynı parolayı yazmanız gerekir; aksi hâlde bağlanamazlar.")}
            </p>
            <div className="flex items-start gap-2">
              <div className="min-w-0 flex-1">
                <Input
                  label={config.password_set ? t("Yeni parola") : t("Parola")}
                  hint={t("Boş bırakırsanız parola değişmez. En az 16 karakter; harf, rakam ve _ - + = . @ : ! kullanılabilir.")}
                  type={revealPassword ? "text" : "password"}
                  value={password}
                  autoComplete="new-password"
                  spellCheck={false}
                  className="font-mono text-[13px]"
                  onChange={(event) => setPassword(event.target.value)}
                />
              </div>
              {/* Etiket yüksekliği kadar boşluk bırakılır ki düğmeler girdi kutusuyla hizalansın. */}
              <div className="flex gap-0.5 pt-[1.625rem]">
                <IconButton label={revealPassword ? t("Parolayı gizle") : t("Parolayı göster")} onClick={() => setRevealPassword((value) => !value)}>
                  {revealPassword ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
                </IconButton>
                <IconButton label={t("Kopyala")} disabled={password === ""} onClick={() => void copyPassword()}>
                  <Copy className="size-4" />
                </IconButton>
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                onClick={() => {
                  setPassword(generatePassword(24));
                  setRevealPassword(true);
                }}
              >
                {t("Rastgele üret")}
              </Button>
              {config.password_set && (
                <Button className="text-bad" disabled={save.isPending} onClick={() => void removePassword()}>
                  {t("Parolayı kaldır")}
                </Button>
              )}
            </div>
          </div>
        </Panel>

        {problem && <ErrorBlock message={problem} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending}>
            {t("Kaydet")}
          </Button>
        </div>
      </form>

      <Panel title={t("Tehlikeli bölge")}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="font-medium">{t("Bütün veritabanlarını boşalt")}</p>
            <p className="text-ink-2">{t("Redis'teki bütün anahtarları siler. Geri alınamaz.")}</p>
          </div>
          <Button variant="danger" loading={flushing} onClick={() => void flush()}>
            {t("Bütün veritabanlarını boşalt")}
          </Button>
        </div>
      </Panel>
    </div>
  );
}
