// Redis sayfası: sunucu geneli önbellek/kuyruk sunucusunun durumu, anahtar gezgini ve ayarları.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Eye, EyeOff } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import {
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Meter,
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
import type { KeyspaceEntry } from "./RedisBrowser";
import { RedisBrowser } from "./RedisBrowser";
import { RedisConsole } from "./RedisConsole";

// ---- Türler ----

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

interface ConfigRequest {
  maxmemory_mb: number;
  policy: string;
  appendonly: boolean;
  password?: string;
}

const TAB_IDS = ["status", "keys", "console", "settings"] as const;
type TabId = (typeof TAB_IDS)[number];

/** Adres çubuğundaki `?sekme=` değerleri. */
const TAB_SLUGS: Record<TabId, string> = {
  status: "durum",
  keys: "anahtarlar",
  console: "konsol",
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
    { id: "console", label: t("Konsol") },
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
        {tab === "console" && <RedisConsole info={data} />}
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

/** Anahtar gezgini; boşaltma onayı bu sayfadaki ortak akıştan gelir. */
function KeysTab({ info }: { info: RedisOn }) {
  const { flush, pending } = useFlush();
  return <RedisBrowser info={info} onFlush={(db) => void flush(db)} flushing={pending} />;
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
