// Hesap başına Redis: her hesap kendi Linux kullanıcısıyla çalışan ayrı bir Redis örneği
// açabilir. Bu dosyada hesabın kendi sayfası (bağlantı bilgisi, anahtarlar, konsol) ve
// yöneticinin bütün örnekleri gördüğü liste bulunur.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy, Eye, EyeOff, Pencil, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import { Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Meter, Modal, Mono, PageHeader, Panel, Status, Table, Tabs, Td, Th } from "../components/ui";
import type { RedisAccountInstance, RedisAccountView } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { t } from "../lib/i18n";
import { RedisBrowser } from "./RedisBrowser";
import { RedisConsole } from "./RedisConsole";

const DATABASES = 16;

function CopyField({ label, value, hint, secret }: { label: string; value: string; hint?: string; secret?: boolean }) {
  const [copied, setCopied] = useState(false);
  const [shown, setShown] = useState(!secret);
  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Pano kapalıysa kullanıcı metni elle seçip kopyalayabilir.
    }
  }
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-[13.5px] font-medium text-ink-2">{label}</p>
      <div className="flex items-start gap-1">
        <code className="min-w-0 flex-1 rounded-md border border-line bg-sunken/60 px-3 py-2 font-mono text-[13px] break-all whitespace-pre-wrap select-all">
          {shown ? value : "•".repeat(24)}
        </code>
        {secret && (
          <IconButton label={shown ? t("Gizle") : t("Göster")} onClick={() => setShown((current) => !current)} className="mt-0.5">
            {shown ? <EyeOff className="size-4" /> : <Eye className="size-4" />}
          </IconButton>
        )}
        <IconButton label={copied ? t("Kopyalandı") : t("Kopyala")} onClick={() => void copy()} className="mt-0.5">
          {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
        </IconButton>
      </div>
      {hint && <p className="max-w-[75ch] text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}

function ConnectionTab({ instance }: { instance: RedisAccountInstance }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const password = instance.password ?? "";

  const rotate = useMutation({
    mutationFn: () => api.post<RedisAccountView>("/redis/mine/password"),
    onSuccess: (updated) => {
      queryClient.setQueryData(["redis-mine"], updated);
      feedback.success(t("Redis parolası yenilendi"));
    },
    onError: (error) => feedback.error(error),
  });

  const disable = useMutation({
    mutationFn: () => api.post<RedisAccountView>("/redis/mine/disable"),
    onSuccess: (updated) => {
      queryClient.setQueryData(["redis-mine"], updated);
      void queryClient.invalidateQueries({ queryKey: ["redis"] });
      feedback.success(t("Redis kapatıldı"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRotate() {
    const confirmed = await feedback.confirm({
      title: t("Redis parolası yenilensin mi?"),
      description: t("Eski parolayı kullanan uygulamalar bağlanamaz; ayar dosyalarını yeni parolayla güncellemeniz gerekir. Redis kısa süreliğine yeniden başlar, verileriniz korunur."),
      confirmLabel: t("Parolayı yenile"),
      danger: true,
    });
    if (confirmed) rotate.mutate();
  }

  async function confirmDisable() {
    const confirmed = await feedback.confirm({
      title: t("Redis kapatılsın mı?"),
      description: t("Redis örneğiniz durdurulur ve içindeki bütün veriler (önbellek, oturumlar, kuyruklar) kalıcı olarak silinir. Bu işlem geri alınamaz."),
      confirmLabel: t("Redis'i kapat"),
      danger: true,
      typeToConfirm: "redis",
    });
    if (confirmed) disable.mutate();
  }

  const limit = instance.memory_mb * 1024 * 1024;
  const laravel = `REDIS_HOST=127.0.0.1\nREDIS_PORT=${instance.port}\nREDIS_PASSWORD=${password}`;
  const wordpress = `define('WP_REDIS_HOST', '127.0.0.1');\ndefine('WP_REDIS_PORT', ${instance.port});\ndefine('WP_REDIS_PASSWORD', '${password}');`;

  return (
    <div className="flex flex-col gap-5">
      <Panel title={t("Durum")}>
        <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
          {instance.running ? <Status tone="ok">{t("Çalışıyor")}</Status> : <Status tone="bad">{t("Durmuş")}</Status>}
          {instance.version !== "" && <span className="text-[13.5px] text-ink-2">Redis {instance.version}</span>}
          {instance.keys !== null && <span className="text-[13.5px] text-ink-2">{t("{count} anahtar", { count: instance.keys })}</span>}
        </div>
        {instance.used_memory !== null && (
          <div className="mt-4 max-w-md">
            <div className="mb-1 flex items-baseline justify-between gap-3 text-[13.5px]">
              <span className="text-ink-2">{t("Bellek")}</span>
              <span className="tabular font-mono">
                {formatBytes(instance.used_memory)} / {formatBytes(limit, 0)}
              </span>
            </div>
            <Meter value={(instance.used_memory / limit) * 100} label={t("Bellek")} />
            <p className="mt-2 text-[13px] text-ink-3">{t("Sınır dolunca en uzun süredir kullanılmayan anahtarlar kendiliğinden atılır. Sınırı sunucu yöneticisi belirler.")}</p>
          </div>
        )}
      </Panel>

      <Panel title={t("Bağlantı bilgisi")}>
        <p className="mb-4 max-w-[75ch] text-ink-2">
          {t("Bu Redis yalnızca hesabınıza aittir ve yalnızca sunucunun içinden erişilir. Uygulamalarınız aşağıdaki adres ve parolayla bağlanır.")}
        </p>
        <div className="grid gap-4 sm:grid-cols-2">
          <CopyField label={t("Sunucu")} value={instance.host} />
          <CopyField label={t("Kapı (port)")} value={String(instance.port)} />
          <CopyField label={t("Parola")} value={password} secret />
          <CopyField label={t("Unix soketi")} value={instance.socket} hint={t("Aynı parolayla; yalnızca hesabınızın süreçleri erişebilir.")} />
        </div>
        <div className="mt-5 grid gap-4 border-t border-line pt-4 lg:grid-cols-2">
          <CopyField label={t("Laravel (.env)")} value={laravel} secret />
          <CopyField label={t("WordPress (wp-config.php)")} value={wordpress} secret hint={t("Redis Object Cache gibi bir eklentiyle birlikte kullanılır.")} />
        </div>
        <p className="mt-4 max-w-[75ch] text-[13px] text-ink-3">
          {t("Birden çok siteniz aynı Redis'i kullanıyorsa her birine ayrı veritabanı numarası (0–15) verin; anahtarları birbirine karışmaz.")}
        </p>
      </Panel>

      <Panel title={t("Tehlikeli bölge")}>
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => void confirmRotate()} loading={rotate.isPending}>
            {t("Parolayı yenile")}
          </Button>
          <Button variant="danger" onClick={() => void confirmDisable()} loading={disable.isPending}>
            {t("Redis'i kapat")}
          </Button>
        </div>
      </Panel>
    </div>
  );
}

type TabId = "connection" | "keys" | "console";
const TAB_SLUGS: Record<TabId, string> = { connection: "baglanti", keys: "anahtarlar", console: "konsol" };

function InstanceTabs({ instance }: { instance: RedisAccountInstance }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const requested = searchParams.get("sekme");
  const tab: TabId = (Object.keys(TAB_SLUGS) as TabId[]).find((id) => TAB_SLUGS[id] === requested) ?? "connection";
  const info = { keyspace: [], databases: DATABASES };

  const flush = useMutation({
    mutationFn: (db: number) => api.post("/redis/flush", { db }),
    onSuccess: (_result, db) => {
      void queryClient.invalidateQueries({ queryKey: ["redis"] });
      void queryClient.invalidateQueries({ queryKey: ["redis-mine"] });
      feedback.success(t("db{n} boşaltıldı.", { n: db }));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmFlush(db: number) {
    const confirmed = await feedback.confirm({
      title: t("db{n} boşaltılsın mı?", { n: db }),
      description: t("Bu veritabanındaki bütün anahtarlar silinir. Bu işlem geri alınamaz."),
      confirmLabel: t("Veritabanını boşalt"),
      danger: true,
      typeToConfirm: `db${db}`,
    });
    if (confirmed) flush.mutate(db);
  }

  function selectTab(next: TabId) {
    const params = new URLSearchParams(searchParams);
    params.set("sekme", TAB_SLUGS[next]);
    setSearchParams(params, { replace: true });
  }

  return (
    <>
      <Tabs
        tabs={[
          { id: "connection", label: t("Bağlantı") },
          { id: "keys", label: t("Anahtarlar") },
          { id: "console", label: t("Konsol") },
        ]}
        value={tab}
        onChange={selectTab}
        label={t("Redis bölümleri")}
      />
      {tab === "connection" && <ConnectionTab instance={instance} />}
      {tab === "keys" && <RedisBrowser info={info} onFlush={(db) => void confirmFlush(db)} flushing={flush.isPending} />}
      {tab === "console" && <RedisConsole info={info} />}
    </>
  );
}

/** Hesabın kendi Redis sayfası (yönetici olmayanlar için). */
export function AccountRedisPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const mine = useQuery({ queryKey: ["redis-mine"], queryFn: () => api.get<RedisAccountView>("/redis/mine"), refetchInterval: 10000 });

  const enable = useMutation({
    mutationFn: () => api.post<RedisAccountView>("/redis/mine/enable"),
    onSuccess: (updated) => {
      queryClient.setQueryData(["redis-mine"], updated);
      feedback.success(t("Redis açıldı"));
    },
  });

  let body;
  if (!mine.data) {
    body = mine.isError ? <ErrorBlock message={errorMessage(mine.error)} onRetry={() => void mine.refetch()} /> : <LoadingBlock />;
  } else if (!mine.data.available) {
    body = (
      <Panel flush>
        <EmptyState title={t("Redis bu sunucuda kurulu değil")} description={t("Redis kullanmak için sunucu yöneticinizden kurmasını isteyin.")} />
      </Panel>
    );
  } else if (mine.data.instance === null) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Redis kapalı")}
          description={t("Redis, uygulamalarınızı hızlandıran bellek içi önbellek ve kuyruk sunucusudur. Açtığınızda yalnızca hesabınıza ait, parola korumalı bir Redis çalışmaya başlar; öteki hesaplar verinize erişemez.")}
          action={
            <Button variant="primary" onClick={() => enable.mutate()} loading={enable.isPending}>
              {t("Redis'i aç")}
            </Button>
          }
        />
        {enable.isError && (
          <div className="px-4 sm:px-6 pb-4">
            <ErrorBlock message={errorMessage(enable.error)} />
          </div>
        )}
      </Panel>
    );
  } else {
    body = <InstanceTabs instance={mine.data.instance} />;
  }

  return (
    <>
      <PageHeader title="Redis" description={t("Hesabınıza özel bellek içi önbellek ve kuyruk sunucusu.")} />
      {body}
    </>
  );
}

// ---- Yönetici: hesap örnekleri ----

function MemoryModal({ instance, maxMemory, onClose }: { instance: RedisAccountInstance; maxMemory: number; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [memory, setMemory] = useState(String(instance.memory_mb));

  const save = useMutation({
    mutationFn: () => api.put(`/redis/accounts/${instance.user_id}`, { memory_mb: Number(memory) }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["redis-accounts"] });
      feedback.success(t("{name} hesabının bellek sınırı güncellendi", { name: instance.username }));
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{name} hesabının Redis bellek sınırı", { name: instance.username })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="redis-memory-form" loading={save.isPending}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </>
      }
    >
      <form id="redis-memory-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input
          label={t("Bellek sınırı (MB)")}
          type="number"
          min={16}
          max={maxMemory}
          inputMode="numeric"
          value={memory}
          onChange={(event) => setMemory(event.target.value)}
          hint={t("Sınır dolunca en eski önbellek kayıtları atılır. Değişiklik örneği kısa süreliğine yeniden başlatır; veriler korunur.")}
          autoFocus
          required
        />
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

/** Yöneticinin gördüğü liste: hangi hesap Redis açmış, ne kadar bellek kullanıyor. */
export function RedisAccountsPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<RedisAccountInstance | null>(null);
  const accounts = useQuery({
    queryKey: ["redis-accounts"],
    queryFn: () => api.get<{ accounts: RedisAccountInstance[]; max_memory_mb: number }>("/redis/accounts"),
    refetchInterval: 15000,
  });

  const close = useMutation({
    mutationFn: (instance: RedisAccountInstance) => api.delete(`/redis/accounts/${instance.user_id}`),
    onSuccess: (_result, instance) => feedback.success(t("{name} hesabının Redis'i kapatıldı", { name: instance.username })),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["redis-accounts"] }),
  });

  async function confirmClose(instance: RedisAccountInstance) {
    const confirmed = await feedback.confirm({
      title: t("{name} hesabının Redis'i kapatılsın mı?", { name: instance.username }),
      description: t("Hesabın Redis örneği durdurulur ve içindeki bütün veriler kalıcı olarak silinir. Hesabın uygulamaları Redis'e bağlanamaz."),
      confirmLabel: t("Redis'i kapat"),
      danger: true,
      typeToConfirm: instance.username,
    });
    if (confirmed) close.mutate(instance);
  }

  if (accounts.isPending) return <LoadingBlock />;
  if (accounts.isError) return <ErrorBlock message={errorMessage(accounts.error)} onRetry={() => void accounts.refetch()} />;

  return (
    <>
      <Panel flush title={t("Hesapların Redis örnekleri")}>
        {accounts.data.accounts.length === 0 ? (
          <EmptyState
            title={t("Henüz Redis açan hesap yok")}
            description={t("Her hesap kendi Redis sayfasından, yalnızca kendisine ait ayrı bir Redis açabilir. Açılan örnekler burada listelenir.")}
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Hesap")}</Th>
                <Th>{t("Durum")}</Th>
                <Th align="right">{t("Kapı (port)")}</Th>
                <Th align="right">{t("Anahtar")}</Th>
                <Th align="right">{t("Bellek")}</Th>
                <Th align="right">{t("İşlemler")}</Th>
              </tr>
            </thead>
            <tbody>
              {accounts.data.accounts.map((instance) => (
                <tr key={instance.user_id} className="hover:bg-sunken/50">
                  <Td>
                    <span className="font-medium">{instance.username}</span>
                  </Td>
                  <Td>{instance.running ? <Status tone="ok">{t("Çalışıyor")}</Status> : <Status tone="bad">{t("Durmuş")}</Status>}</Td>
                  <Td align="right">
                    <Mono>{instance.port}</Mono>
                  </Td>
                  <Td align="right">{instance.keys ?? "—"}</Td>
                  <Td align="right">
                    <span className="tabular font-mono text-[13.5px]">
                      {instance.used_memory === null ? "—" : formatBytes(instance.used_memory)} / {instance.memory_mb} MB
                    </span>
                  </Td>
                  <Td align="right" className="py-0.5">
                    <div className="flex justify-end gap-0.5">
                      <IconButton label={t("{name} hesabının bellek sınırını değiştir", { name: instance.username })} onClick={() => setEditing(instance)}>
                        <Pencil className="size-4" />
                      </IconButton>
                      <IconButton label={t("{name} hesabının Redis'ini kapat", { name: instance.username })} onClick={() => void confirmClose(instance)}>
                        <Trash2 className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
      {editing && <MemoryModal instance={editing} maxMemory={accounts.data.max_memory_mb} onClose={() => setEditing(null)} />}
    </>
  );
}
