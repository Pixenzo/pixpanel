// Redis anahtar gezgini: solda ad alanı ağacı / düz liste, sağda seçili anahtarın ayrıntısı ve düzenleyicisi.

import type { InfiniteData, UseInfiniteQueryResult } from "@tanstack/react-query";
import { keepPreviousData, useInfiniteQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronRight, Copy, Pencil, Plus, RefreshCw, Trash2, X } from "lucide-react";
import type { FormEvent, KeyboardEvent, ReactNode } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Select, Spinner, Textarea } from "../components/ui";
import type { Tone } from "../components/ui";
import { api, ApiError, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDuration } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Türler ----

export type RedisType = "string" | "hash" | "list" | "set" | "zset" | "stream";

const REDIS_TYPES: RedisType[] = ["string", "hash", "list", "set", "zset", "stream"];
/** Gezgin üzerinden oluşturulabilen türler (akışlar salt okunurdur). */
const CREATABLE_TYPES: RedisType[] = ["string", "hash", "list", "set", "zset"];

export interface KeyspaceEntry {
  db: number;
  keys: number;
  expires: number;
}

/** Gezginin Redis durumundan ihtiyaç duyduğu kısım. */
export interface BrowserInfo {
  keyspace: KeyspaceEntry[];
  databases: number;
}

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

interface HashField {
  field: string;
  value: string;
}

interface ZsetPair {
  member: string;
  score: string;
}

interface StreamEntry {
  id: string;
  fields: HashField[];
}

interface StringValue {
  text: string;
  truncated: boolean;
  bytes: number;
  binary: boolean;
}

interface KeyContent {
  value?: StringValue;
  items?: string[];
  offset?: number;
  pairs?: ZsetPair[];
  fields?: HashField[];
  entries?: StreamEntry[];
}

interface KeyDetailPage {
  key: string;
  type: RedisType;
  ttl: number;
  size: number;
  encoding: string;
  length: number;
  content: KeyContent;
  /** Boş dize: devamı yok. */
  next: string;
}

type KeyOp =
  | "set_string"
  | "set_ttl"
  | "rename"
  | "hash_set"
  | "hash_delete"
  | "list_push"
  | "list_set"
  | "list_remove"
  | "set_add"
  | "set_remove"
  | "zset_add"
  | "zset_remove";

type OpArgs = Record<string, string | number | null>;

// ---- Sabitler ve küçük yardımcılar ----

const ROW_HEIGHT = 28;
const OVERSCAN = 8;
const LOAD_ALL_LIMIT = 5000;
const PREVIEW_CHARS = 200;

/** Tür başına tek ve tutarlı renk eşlemesi; bütün gezgin boyunca bu kullanılır. */
const TYPE_TONE: Record<RedisType, Tone> = {
  string: "idle",
  hash: "accent",
  list: "ok",
  set: "warn",
  zset: "bad",
  stream: "idle",
};

const FIELD_BASE =
  "min-w-0 rounded-md border border-line-strong bg-surface px-2.5 font-mono text-[13px] text-ink placeholder:text-ink-3 " +
  "transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 disabled:bg-sunken disabled:text-ink-3";
const FIELD_INPUT = `${FIELD_BASE} h-9`;
const FIELD_AREA = `${FIELD_BASE} py-1.5`;
const TOOLBAR_SELECT =
  "h-9 rounded-md border border-line-strong bg-surface px-3 text-ink transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30";
const LINK_BUTTON = "text-[13px] text-accent hover:underline";

const numberFormat = new Intl.NumberFormat("tr-TR");
const collator = new Intl.Collator(undefined, { numeric: true });

function formatNumber(value: number): string {
  return numberFormat.format(value);
}

const enc = encodeURIComponent;

/** Kalan süre, liste satırları için kısa biçimde ("5 dk"); süresizse boş. */
function ttlShort(ttl: number): string {
  if (ttl < 0) return "";
  if (ttl < 60) return t("{n} sn", { n: ttl });
  if (ttl < 3600) return t("{n} dk", { n: Math.floor(ttl / 60) });
  if (ttl < 86400) return t("{n} sa", { n: Math.floor(ttl / 3600) });
  return t("{n} gün", { n: Math.floor(ttl / 86400) });
}

function ttlLong(ttl: number): string {
  return ttl < 0 ? t("Süresiz") : formatDuration(ttl);
}

/** Metin bir JSON nesnesi ya da dizisiyse 2 boşluklu biçimli hâlini verir. */
function prettyJson(text: string): string | null {
  const trimmed = text.trim();
  if (trimmed.length === 0 || trimmed.length > 1_000_000) return null;
  const first = trimmed[0];
  if (first !== "{" && first !== "[") return null;
  try {
    return JSON.stringify(JSON.parse(trimmed), null, 2);
  } catch {
    return null;
  }
}

function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(value), delay);
    return () => window.clearTimeout(id);
  }, [value, delay]);
  return debounced;
}

/** Adres çubuğundaki `db` değerini, geçerli aralıkta bir sayıya çevirir. */
function readDb(params: URLSearchParams, databases: number): number {
  const raw = Number(params.get("db") ?? "0");
  return Number.isInteger(raw) && raw >= 0 && raw < databases ? raw : 0;
}

/** Konsol gibi gezgin dışı sekmelerin de aynı `db` parametresini paylaşması için. */
export function useDbParam(databases: number): [number, (next: number) => void] {
  const [searchParams, setSearchParams] = useSearchParams();
  const db = readDb(searchParams, databases);
  function setDb(next: number) {
    const params = new URLSearchParams(searchParams);
    params.set("db", String(next));
    setSearchParams(params, { replace: true });
  }
  return [db, setDb];
}

// ---- Ortak bileşenler ----

export function DbSelect({ value, info, onChange }: { value: number; info: BrowserInfo; onChange: (db: number) => void }) {
  const options = Array.from({ length: Math.max(1, info.databases) }, (_unused, n) => n);
  return (
    <select aria-label={t("Veritabanı")} value={value} onChange={(event) => onChange(Number(event.target.value))} className={TOOLBAR_SELECT}>
      {options.map((n) => {
        const count = info.keyspace.find((entry) => entry.db === n)?.keys;
        return (
          <option key={n} value={n}>
            {count === undefined ? `db${n}` : t("db{n} ({keys} anahtar)", { n, keys: formatNumber(count) })}
          </option>
        );
      })}
    </select>
  );
}

function TypeBadge({ type, fixed }: { type: RedisType; fixed?: boolean }) {
  const badge = <Badge tone={TYPE_TONE[type]}>{type}</Badge>;
  return fixed ? <span className="inline-block w-14 shrink-0">{badge}</span> : badge;
}

function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  compact,
}: {
  value: T;
  options: { id: T; label: string }[];
  onChange: (next: T) => void;
  label: string;
  compact?: boolean;
}) {
  return (
    <div role="group" aria-label={label} className={classNames("inline-flex shrink-0 rounded-md border border-line-strong bg-surface p-0.5", compact ? "h-7" : "h-9")}>
      {options.map((option) => (
        <button
          key={option.id}
          type="button"
          aria-pressed={value === option.id}
          onClick={() => onChange(option.id)}
          className={classNames(
            "rounded px-2.5 text-[13.5px] font-medium transition-colors",
            value === option.id ? "bg-accent-soft text-accent" : "text-ink-2 hover:text-ink",
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

// ---- Yazma işlemleri ----

interface RunOptions {
  /** Anahtar kümesi ya da süre değişti: liste ve veritabanı sayıları yenilensin. */
  listChanged?: boolean;
  /** Varsayılan: ayrıntı yeniden okunur. Yeniden adlandırmada kapatılır (eski ad artık yok). */
  refreshDetail?: boolean;
}

/** `POST /redis/key` işlemleri; başarıda bildirim gösterir, hatada hata bildirimi verir. */
function useKeyOps(db: number) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const mutation = useMutation({
    mutationFn: (vars: { key: string; op: KeyOp; args: OpArgs }) => api.post<{ result: string }>("/redis/key", { db, key: vars.key, op: vars.op, ...vars.args }),
  });

  async function run(key: string, op: KeyOp, args: OpArgs, message: string, options: RunOptions = {}): Promise<boolean> {
    try {
      await mutation.mutateAsync({ key, op, args });
    } catch (error) {
      feedback.error(error);
      return false;
    }
    feedback.success(message);
    if (options.refreshDetail !== false) void queryClient.invalidateQueries({ queryKey: ["redis", "key", db, key] });
    if (options.listChanged) {
      void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
      void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
    }
    return true;
  }

  return { run, pending: mutation.isPending };
}

type KeyOps = ReturnType<typeof useKeyOps>;

// ---- Gezgin ----

export function RedisBrowser({ info, onFlush, flushing }: { info: BrowserInfo; onFlush: (db: number) => void; flushing: boolean }) {
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();

  const db = readDb(searchParams, info.databases);
  const pattern = searchParams.get("kalip") ?? "";
  const rawType = searchParams.get("tur") ?? "";
  const type = REDIS_TYPES.find((item) => item === rawType) ?? "";
  const selectedKey = searchParams.get("anahtar") ?? null;

  const [patternInput, setPatternInput] = useState(pattern);
  const [mode, setMode] = useState<"tree" | "list">("tree");
  const [creating, setCreating] = useState(false);

  // Geri düğmesiyle kalıp değiştiğinde giriş kutusu da eşlensin.
  useEffect(() => setPatternInput(pattern), [pattern]);

  const keys = useInfiniteQuery({
    queryKey: ["redis", "keys", db, pattern, type],
    queryFn: ({ pageParam }) => {
      const params = new URLSearchParams({ db: String(db), pattern: pattern || "*", cursor: pageParam });
      if (type) params.set("type", type);
      return api.get<KeysPage>(`/redis/keys?${params.toString()}`);
    },
    initialPageParam: "0",
    getNextPageParam: (last) => (last.cursor !== "0" ? last.cursor : undefined),
    refetchOnWindowFocus: false,
  });

  // Aynı anahtar SCAN sayfaları arasında tekrar gelebilir; yinelenenler atılır.
  const rows = useMemo(() => {
    const result: KeyRow[] = [];
    const seen = new Set<string>();
    for (const page of keys.data?.pages ?? []) {
      for (const row of page.keys) {
        if (seen.has(row.key)) continue;
        seen.add(row.key);
        result.push(row);
      }
    }
    return result;
  }, [keys.data]);

  function patchParams(patch: Record<string, string | null>, replace: boolean) {
    const params = new URLSearchParams(searchParams);
    for (const [name, value] of Object.entries(patch)) {
      if (value === null || value === "") params.delete(name);
      else params.set(name, value);
    }
    setSearchParams(params, { replace });
  }

  function selectKey(key: string | null, replace: boolean) {
    patchParams({ anahtar: key }, replace);
  }

  function applyPattern(event: FormEvent) {
    event.preventDefault();
    patchParams({ kalip: patternInput.trim() }, false);
  }

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
    void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
    if (selectedKey) void queryClient.invalidateQueries({ queryKey: ["redis", "key", db, selectedKey] });
  }

  const scopeKey = `${db}|${pattern}|${type}`;

  return (
    <div className="flex flex-col gap-3 lg:h-[calc(100dvh-18rem)] lg:min-h-[30rem]">
      <div className="flex flex-wrap items-center gap-2">
        <DbSelect value={db} info={info} onChange={(next) => patchParams({ db: String(next), anahtar: null }, false)} />
        <form onSubmit={applyPattern}>
          <input
            type="search"
            aria-label={t("Kalıp")}
            title={t("Redis kalıbı: * her şeye uyar. Uygulamak için Enter'a basın.")}
            placeholder="oturum:*"
            value={patternInput}
            spellCheck={false}
            autoComplete="off"
            onChange={(event) => setPatternInput(event.target.value)}
            className={`${FIELD_INPUT} w-56`}
          />
        </form>
        <select aria-label={t("Tür")} value={type} onChange={(event) => patchParams({ tur: event.target.value }, false)} className={TOOLBAR_SELECT}>
          <option value="">{t("Tümü")}</option>
          {REDIS_TYPES.map((item) => (
            <option key={item} value={item}>
              {item}
            </option>
          ))}
        </select>
        <Segmented
          label={t("Görünüm")}
          value={mode}
          onChange={setMode}
          options={[
            { id: "tree", label: t("Ağaç") },
            { id: "list", label: t("Liste") },
          ]}
        />
        <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
          {t("Yeni anahtar")}
        </Button>
        <IconButton label={t("Yenile")} onClick={refresh} className="size-9 border border-line-strong">
          <RefreshCw className={classNames("size-4", keys.isFetching && "animate-[spin_0.8s_linear_infinite]")} />
        </IconButton>
        <div className="ml-auto">
          <Button variant="secondary" loading={flushing} className="text-bad" onClick={() => onFlush(db)}>
            {t("Bu veritabanını boşalt")}
          </Button>
        </div>
      </div>

      <div className="flex flex-col gap-3 lg:min-h-0 lg:flex-1 lg:flex-row">
        <section className="flex h-80 min-w-0 flex-col rounded-lg border border-line bg-surface lg:h-auto lg:min-h-0 lg:w-[38%] lg:min-w-[320px] lg:flex-none">
          <KeyPane query={keys} rows={rows} scopeKey={scopeKey} expandAll={pattern !== ""} selectedKey={selectedKey} onSelect={selectKey} mode={mode} />
        </section>
        <section className="flex min-h-[26rem] min-w-0 flex-col rounded-lg border border-line bg-surface lg:min-h-0 lg:flex-1">
          {selectedKey === null ? (
            <div className="flex flex-1 items-center justify-center">
              <EmptyState title={t("Bir anahtar seçin")} description={t("Soldaki listeden bir anahtar seçerek değerini görüntüleyin ve düzenleyin.")} />
            </div>
          ) : (
            <DetailPane db={db} keyName={selectedKey} onSelect={selectKey} />
          )}
        </section>
      </div>

      {creating && (
        <NewKeyModal
          db={db}
          onClose={() => setCreating(false)}
          onCreated={(key) => {
            setCreating(false);
            void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
            void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
            selectKey(key, false);
          }}
        />
      )}
    </div>
  );
}

// ---- Anahtar listesi (sol bölme) ----

interface Folder {
  id: string;
  name: string;
  count: number;
  folders: Folder[];
  keys: { name: string; row: KeyRow }[];
}

type ListEntry =
  | { kind: "folder"; id: string; depth: number; name: string; count: number; open: boolean }
  | { kind: "key"; id: string; depth: number; name: string; row: KeyRow };

/** Anahtarları `:` ile ad alanlarına böler. */
function buildTree(rows: KeyRow[]): Folder {
  const root: Folder = { id: "f:", name: "", count: 0, folders: [], keys: [] };
  const index = new Map<string, Folder>();
  for (const row of rows) {
    const parts = row.key.split(":");
    const leaf = parts.pop() ?? row.key;
    let folder = root;
    let path = "";
    for (const part of parts) {
      path = folder === root ? part : `${path}:${part}`;
      const id = `f:${path}`;
      let child = index.get(id);
      if (!child) {
        child = { id, name: part, count: 0, folders: [], keys: [] };
        index.set(id, child);
        folder.folders.push(child);
      }
      child.count += 1;
      folder = child;
    }
    folder.keys.push({ name: leaf, row });
    root.count += 1;
  }
  sortFolder(root);
  return root;
}

function sortFolder(folder: Folder) {
  folder.folders.sort((a, b) => collator.compare(a.name, b.name));
  folder.keys.sort((a, b) => collator.compare(a.name, b.name));
  for (const child of folder.folders) sortFolder(child);
}

function flattenTree(folder: Folder, depth: number, expanded: ReadonlySet<string>, out: ListEntry[]) {
  for (const child of folder.folders) {
    const open = expanded.has(child.id);
    out.push({ kind: "folder", id: child.id, depth, name: child.name, count: child.count, open });
    if (open) flattenTree(child, depth + 1, expanded, out);
  }
  for (const leaf of folder.keys) {
    out.push({ kind: "key", id: `k:${leaf.row.key}`, depth, name: leaf.name, row: leaf.row });
  }
}

function collectFolderIds(folder: Folder, out: Set<string>) {
  for (const child of folder.folders) {
    out.add(child.id);
    collectFolderIds(child, out);
  }
}

/** "a:b:c" anahtarının üst klasör kimlikleri: f:a, f:a:b. */
function ancestorIds(key: string): string[] {
  const parts = key.split(":");
  parts.pop();
  const ids: string[] = [];
  let path = "";
  for (const [position, part] of parts.entries()) {
    path = position === 0 ? part : `${path}:${part}`;
    ids.push(`f:${path}`);
  }
  return ids;
}

function countLoadedKeys(data: InfiniteData<KeysPage> | undefined): number {
  return data?.pages.reduce((sum, page) => sum + page.keys.length, 0) ?? 0;
}

interface KeyPaneProps {
  query: UseInfiniteQueryResult<InfiniteData<KeysPage>, Error>;
  rows: KeyRow[];
  scopeKey: string;
  expandAll: boolean;
  selectedKey: string | null;
  onSelect: (key: string | null, replace: boolean) => void;
  mode: "tree" | "list";
}

function KeyPane({ query, rows, scopeKey, expandAll, selectedKey, onSelect, mode }: KeyPaneProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const cancelRef = useRef(false);
  const appliedScope = useRef<string | null>(null);

  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [cursorId, setCursorId] = useState<string | null>(null);
  const [view, setView] = useState({ top: 0, height: 480 });
  const [loadingAll, setLoadingAll] = useState(false);

  const tree = useMemo(() => (mode === "tree" ? buildTree(rows) : null), [rows, mode]);

  const entries = useMemo<ListEntry[]>(() => {
    if (tree) {
      const out: ListEntry[] = [];
      flattenTree(tree, 0, expanded, out);
      return out;
    }
    return [...rows]
      .sort((a, b) => collator.compare(a.key, b.key))
      .map((row) => ({ kind: "key", id: `k:${row.key}`, depth: 0, name: row.key, row }));
  }, [tree, rows, expanded]);

  // Her yeni kapsamda (veritabanı, kalıp, tür) bir kez: tek üst ad alanı ya da arama varsa klasörleri aç.
  useEffect(() => {
    if (!tree || rows.length === 0 || appliedScope.current === scopeKey) return;
    appliedScope.current = scopeKey;
    const next = new Set<string>();
    if (expandAll) collectFolderIds(tree, next);
    else if (tree.folders.length === 1 && tree.keys.length === 0) next.add(tree.folders[0]?.id ?? "");
    if (selectedKey) for (const id of ancestorIds(selectedKey)) next.add(id);
    setExpanded(next);
  }, [tree, rows.length, scopeKey, expandAll, selectedKey]);

  // Kapsam değişince ya da bileşen kalkınca süren "tümünü yükle" döngüsü durur; liste başa döner.
  useEffect(() => {
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
    return () => {
      cancelRef.current = true;
    };
  }, [scopeKey]);

  useEffect(() => {
    const element = scrollRef.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setView((current) => ({ ...current, height: element.clientHeight })));
    observer.observe(element);
    setView((current) => ({ ...current, height: element.clientHeight }));
    return () => observer.disconnect();
  }, []);

  const start = Math.max(0, Math.floor(view.top / ROW_HEIGHT) - OVERSCAN);
  const end = Math.min(entries.length, Math.ceil((view.top + view.height) / ROW_HEIGHT) + OVERSCAN);

  function toggleFolder(id: string) {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }

  function reveal(index: number) {
    const element = scrollRef.current;
    if (!element) return;
    const top = index * ROW_HEIGHT;
    if (top < element.scrollTop) element.scrollTop = top;
    else if (top + ROW_HEIGHT > element.scrollTop + element.clientHeight) element.scrollTop = top + ROW_HEIGHT - element.clientHeight;
  }

  function moveTo(index: number) {
    const entry = entries[index];
    if (!entry) return;
    setCursorId(entry.id);
    if (entry.kind === "key") onSelect(entry.row.key, true);
    reveal(index);
  }

  function activate(entry: ListEntry) {
    setCursorId(entry.id);
    if (entry.kind === "folder") toggleFolder(entry.id);
    else onSelect(entry.row.key, false);
  }

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (entries.length === 0) return;
    const activeId = cursorId ?? (selectedKey ? `k:${selectedKey}` : "");
    const current = entries.findIndex((entry) => entry.id === activeId);
    const entry = entries[current];
    switch (event.key) {
      case "ArrowDown":
        event.preventDefault();
        moveTo(Math.min(entries.length - 1, current + 1));
        break;
      case "ArrowUp":
        event.preventDefault();
        moveTo(Math.max(0, current < 0 ? 0 : current - 1));
        break;
      case "Home":
        event.preventDefault();
        moveTo(0);
        break;
      case "End":
        event.preventDefault();
        moveTo(entries.length - 1);
        break;
      case "Enter":
        if (entry) {
          event.preventDefault();
          activate(entry);
        }
        break;
      case "ArrowRight":
        if (entry?.kind === "folder" && !entry.open) {
          event.preventDefault();
          toggleFolder(entry.id);
        }
        break;
      case "ArrowLeft":
        if (entry?.kind === "folder" && entry.open) {
          event.preventDefault();
          toggleFolder(entry.id);
        }
        break;
    }
  }

  async function loadAll() {
    cancelRef.current = false;
    setLoadingAll(true);
    try {
      let result = await query.fetchNextPage();
      while (!cancelRef.current && !result.isError && result.hasNextPage && countLoadedKeys(result.data) < LOAD_ALL_LIMIT) {
        result = await query.fetchNextPage();
      }
    } finally {
      setLoadingAll(false);
    }
  }

  const activeId = cursorId ?? "";
  const hasMore = query.hasNextPage;

  return (
    <>
      <div
        ref={scrollRef}
        role="tree"
        tabIndex={0}
        aria-label={t("Anahtarlar")}
        onKeyDown={onKeyDown}
        onScroll={(event) => {
          const top = event.currentTarget.scrollTop;
          setView((current) => (current.top === top ? current : { ...current, top }));
        }}
        className="min-h-0 flex-1 overflow-y-auto rounded-t-lg focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent/40"
      >
        {query.isPending && <LoadingBlock />}
        {query.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(query.error)} onRetry={() => void query.refetch()} />
          </div>
        )}
        {query.data && entries.length === 0 && !hasMore && <EmptyState title={t("Bu kalıba uyan anahtar yok")} description={t("Veritabanını, kalıbı ya da tür süzgecini değiştirin.")} />}
        {query.data && entries.length === 0 && hasMore && (
          <p className="px-4 sm:px-6 py-4 text-ink-2">{t("Şu ana kadar taranan kısımda eşleşen anahtar çıkmadı; tarama sürüyor.")}</p>
        )}
        {entries.length > 0 && (
          <div style={{ paddingTop: start * ROW_HEIGHT, paddingBottom: (entries.length - end) * ROW_HEIGHT }}>
            {entries.slice(start, end).map((entry) => {
              const selected = entry.kind === "key" && entry.row.key === selectedKey;
              const cursor = !selected && entry.id === activeId;
              const tone = selected ? "bg-accent-soft" : cursor ? "bg-sunken" : "hover:bg-sunken";
              const indent = { paddingLeft: 8 + entry.depth * 14 };
              if (entry.kind === "folder") {
                return (
                  <div
                    key={entry.id}
                    role="treeitem"
                    aria-expanded={entry.open}
                    aria-selected={false}
                    onClick={() => activate(entry)}
                    style={indent}
                    className={classNames("flex h-7 cursor-pointer items-center gap-1.5 pr-3 select-none", tone)}
                  >
                    <ChevronRight aria-hidden className={classNames("size-3.5 shrink-0 text-ink-3 transition-transform", entry.open && "rotate-90")} />
                    <span title={`${entry.name}:`} className="min-w-0 flex-1 truncate font-mono text-[13px] font-medium">
                      {entry.name === "" ? t("(boş)") : entry.name}:
                    </span>
                    <span className="shrink-0 text-[12.5px] text-ink-3 tabular-nums">{formatNumber(entry.count)}</span>
                  </div>
                );
              }
              const ttl = ttlShort(entry.row.ttl);
              return (
                <div
                  key={entry.id}
                  role="treeitem"
                  aria-selected={selected}
                  onClick={() => activate(entry)}
                  style={mode === "tree" ? { paddingLeft: indent.paddingLeft + 20 } : { paddingLeft: 8 }}
                  className={classNames("flex h-7 cursor-pointer items-center gap-2 pr-3 select-none", tone)}
                >
                  <TypeBadge type={entry.row.type} fixed />
                  <span title={entry.row.key} className="min-w-0 flex-1 truncate font-mono text-[13px]">
                    {entry.name === "" ? t("(boş)") : entry.name}
                  </span>
                  {ttl !== "" && <span className="shrink-0 text-[12.5px] text-ink-3 tabular-nums">{ttl}</span>}
                  {mode === "list" && <span className="w-16 shrink-0 text-right text-[12.5px] text-ink-3 tabular-nums">{formatBytes(entry.row.size)}</span>}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1 border-t border-line px-3 py-2">
        <span className="text-[13px] text-ink-3">{t("{n} anahtar yüklendi", { n: formatNumber(rows.length) })}</span>
        <div className="ml-auto flex items-center gap-2">
          {loadingAll ? (
            <>
              <Spinner className="text-ink-3" />
              <Button size="sm" onClick={() => (cancelRef.current = true)}>
                {t("Durdur")}
              </Button>
            </>
          ) : (
            hasMore && (
              <>
                <Button size="sm" loading={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>
                  {t("Daha fazla yükle")}
                </Button>
                <Button size="sm" disabled={query.isFetchingNextPage} onClick={() => void loadAll()}>
                  {t("Tümünü yükle")}
                </Button>
              </>
            )
          )}
        </div>
      </div>
    </>
  );
}

// ---- Ayrıntı (sağ bölme) ----

interface MergedContent {
  value?: StringValue;
  items: string[];
  offset: number;
  pairs: ZsetPair[];
  fields: HashField[];
  entries: StreamEntry[];
}

/** Sayfalanmış koleksiyon içeriğini tek diziye birleştirir. */
function mergePages(pages: KeyDetailPage[] | undefined): MergedContent {
  const merged: MergedContent = { items: [], offset: 0, pairs: [], fields: [], entries: [] };
  for (const [position, page] of (pages ?? []).entries()) {
    const content = page.content;
    if (position === 0) {
      merged.value = content.value;
      merged.offset = content.offset ?? 0;
    }
    merged.items.push(...(content.items ?? []));
    merged.pairs.push(...(content.pairs ?? []));
    merged.fields.push(...(content.fields ?? []));
    merged.entries.push(...(content.entries ?? []));
  }
  return merged;
}

function shownCount(type: RedisType, merged: MergedContent): number {
  switch (type) {
    case "list":
    case "set":
      return merged.items.length;
    case "zset":
      return merged.pairs.length;
    case "hash":
      return merged.fields.length;
    case "stream":
      return merged.entries.length;
    case "string":
      return 0;
  }
}

function DetailPane({ db, keyName, onSelect }: { db: number; keyName: string; onSelect: (key: string | null, replace: boolean) => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const ops = useKeyOps(db);

  // Süzgeç anahtara bağlıdır; anahtar değişince eski süzgeç yeni anahtara uygulanmaz.
  const [filterInput, setFilterInput] = useState({ key: keyName, text: "" });
  const inputText = filterInput.key === keyName ? filterInput.text : "";
  const applied = useDebounced(
    useMemo(() => ({ key: keyName, text: inputText }), [keyName, inputText]),
    300,
  );
  const filter = applied.key === keyName ? applied.text.trim() : "";

  const detail = useInfiniteQuery({
    queryKey: ["redis", "key", db, keyName, filter],
    queryFn: ({ pageParam }) => api.get<KeyDetailPage>(`/redis/key?db=${db}&key=${enc(keyName)}&cursor=${enc(pageParam)}&filter=${enc(filter)}`),
    initialPageParam: "",
    getNextPageParam: (last) => (last.next !== "" ? last.next : undefined),
    // Yeni anahtar yüklenirken önceki ayrıntı görünür kalır; yerleşim sıçramaz.
    placeholderData: keepPreviousData,
    refetchOnWindowFocus: false,
    retry: (count, error) => !(error instanceof ApiError && error.status === 404) && count < 2,
  });

  const pages = detail.data?.pages;
  const head = pages?.[0];
  const merged = useMemo(() => mergePages(pages), [pages]);

  if (!head) {
    if (detail.isError) {
      if (detail.error instanceof ApiError && detail.error.status === 404) {
        return (
          <div className="flex flex-1 items-center justify-center">
            <EmptyState
              title={t("Anahtar bulunamadı")}
              description={t("Anahtar silinmiş ya da süresi dolmuş olabilir.")}
              action={
                <Button
                  onClick={() => {
                    void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
                    onSelect(null, true);
                  }}
                >
                  {t("Seçimi kaldır")}
                </Button>
              }
            />
          </div>
        );
      }
      return (
        <div className="p-4">
          <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />
        </div>
      );
    }
    return (
      <div className="flex flex-1 items-center justify-center">
        <LoadingBlock />
      </div>
    );
  }

  // Başka bir anahtarın verisi yeni anahtar yüklenirken yer tutucu olarak gösteriliyor.
  const stale = head.key !== keyName;
  const shown = shownCount(head.type, merged);
  const isCollection = head.type !== "string";

  async function copyKey() {
    try {
      await navigator.clipboard.writeText(keyName);
      feedback.success(t("Anahtar adı panoya kopyalandı."));
    } catch {
      feedback.error(new Error(t("Panoya kopyalanamadı. Adı elle seçip kopyalayın.")));
    }
  }

  async function removeKey() {
    const confirmed = await feedback.confirm({
      title: t("Anahtar silinsin mi?"),
      description: t("“{key}” anahtarı db{db} içinden silinir. Bu anahtarı kullanan uygulamalar etkilenebilir.", { key: keyName, db }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (!confirmed) return;
    try {
      await api.delete<{ removed: number }>(`/redis/key?db=${db}&key=${enc(keyName)}`);
    } catch (error) {
      feedback.error(error);
      return;
    }
    feedback.success(t("Anahtar silindi."));
    queryClient.removeQueries({ queryKey: ["redis", "key", db, keyName] });
    void queryClient.invalidateQueries({ queryKey: ["redis", "keys"] });
    void queryClient.invalidateQueries({ queryKey: ["redis", "info"] });
    onSelect(null, false);
  }

  async function renameKey(next: string): Promise<boolean> {
    const name = next.trim();
    if (name === "" || name === keyName) return true;
    const ok = await ops.run(keyName, "rename", { new_key: name }, t("Anahtar yeniden adlandırıldı."), { listChanged: true, refreshDetail: false });
    if (!ok) return false;
    queryClient.removeQueries({ queryKey: ["redis", "key", db, keyName] });
    onSelect(name, true);
    return true;
  }

  function setTtl(seconds: number | null): Promise<boolean> {
    return ops.run(keyName, "set_ttl", { seconds }, seconds === null ? t("Süre kaldırıldı.") : t("Süre ayarlandı."), { listChanged: true });
  }

  return (
    <>
      <DetailHeader
        detail={head}
        keyName={keyName}
        fetching={detail.isFetching}
        busy={ops.pending}
        disabled={stale}
        onCopy={() => void copyKey()}
        onRefresh={() => void detail.refetch()}
        onRemove={() => void removeKey()}
        onRename={renameKey}
        onSetTtl={setTtl}
      />
      <div className={classNames("min-h-0 flex-1 overflow-auto p-4 transition-opacity", stale && "pointer-events-none opacity-60")}>
        {head.type === "string" && <StringView key={keyName} keyName={keyName} bytes={head.length} value={merged.value} ops={ops} />}
        {head.type === "hash" && (
          <HashView keyName={keyName} fields={merged.fields} filter={inputText} onFilter={(text) => setFilterInput({ key: keyName, text })} ops={ops} />
        )}
        {head.type === "list" && <ListView keyName={keyName} items={merged.items} offset={merged.offset} ops={ops} />}
        {head.type === "set" && <SetView keyName={keyName} members={merged.items} filter={inputText} onFilter={(text) => setFilterInput({ key: keyName, text })} ops={ops} />}
        {head.type === "zset" && <ZsetView keyName={keyName} pairs={merged.pairs} ops={ops} />}
        {head.type === "stream" && <StreamView entries={merged.entries} />}

        {isCollection && (
          <div className="mt-3 flex flex-wrap items-center gap-3">
            <span className="text-[13px] text-ink-3">{t("{shown} / {total} öğe", { shown: formatNumber(shown), total: formatNumber(head.length) })}</span>
            {detail.hasNextPage && (
              <Button size="sm" loading={detail.isFetchingNextPage} onClick={() => void detail.fetchNextPage()}>
                {t("Daha fazla yükle")}
              </Button>
            )}
          </div>
        )}
      </div>
    </>
  );
}

interface DetailHeaderProps {
  detail: KeyDetailPage;
  keyName: string;
  fetching: boolean;
  busy: boolean;
  disabled: boolean;
  onCopy: () => void;
  onRefresh: () => void;
  onRemove: () => void;
  onRename: (next: string) => Promise<boolean>;
  onSetTtl: (seconds: number | null) => Promise<boolean>;
}

function Meta({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline gap-1.5">
      <dt className="text-[13px] text-ink-3">{label}</dt>
      <dd className="text-[13.5px]">{children}</dd>
    </div>
  );
}

function DetailHeader({ detail, keyName, fetching, busy, disabled, onCopy, onRefresh, onRemove, onRename, onSetTtl }: DetailHeaderProps) {
  const [renaming, setRenaming] = useState(false);
  const [editingTtl, setEditingTtl] = useState(false);

  // Başka anahtara geçilince açık düzenleyiciler kapanır.
  useEffect(() => {
    setRenaming(false);
    setEditingTtl(false);
  }, [keyName]);

  return (
    <div className={classNames("shrink-0 border-b border-line px-4 sm:px-6 py-3", disabled && "pointer-events-none")}>
      <div className="flex items-start gap-3">
        <span className="mt-0.5">
          <TypeBadge type={detail.type} />
        </span>
        <div className="min-w-0 flex-1">
          {renaming ? (
            <InlineEditor
              initial={detail.key}
              label={t("Yeni anahtar adı")}
              saving={busy}
              onSave={async (value) => {
                if (await onRename(value)) setRenaming(false);
              }}
              onCancel={() => setRenaming(false)}
            />
          ) : (
            <Mono className="text-[14px] leading-snug break-all">{detail.key}</Mono>
          )}
        </div>
        <div className="flex shrink-0 gap-0.5">
          <IconButton label={t("Kopyala")} onClick={onCopy}>
            <Copy className="size-4" />
          </IconButton>
          <IconButton label={t("Yeniden adlandır")} onClick={() => setRenaming(true)} disabled={renaming}>
            <Pencil className="size-4" />
          </IconButton>
          <IconButton label={t("Yenile")} onClick={onRefresh}>
            <RefreshCw className={classNames("size-4", fetching && "animate-[spin_0.8s_linear_infinite]")} />
          </IconButton>
          <IconButton label={t("Sil")} className="hover:text-bad" onClick={onRemove}>
            <Trash2 className="size-4" />
          </IconButton>
        </div>
      </div>

      <dl className="mt-2.5 flex flex-wrap gap-x-6 gap-y-1.5">
        <Meta label={t("TTL")}>
          <span className="inline-flex items-center gap-1">
            {ttlLong(detail.ttl)}
            <IconButton label={t("Süreyi düzenle")} className="-my-1.5 size-6" onClick={() => setEditingTtl((open) => !open)}>
              <Pencil className="size-3.5" />
            </IconButton>
          </span>
        </Meta>
        <Meta label={t("Boyut")}>{formatBytes(detail.size)}</Meta>
        <Meta label={t("Öğe")}>{detail.type === "string" ? t("{n} bayt", { n: formatNumber(detail.length) }) : formatNumber(detail.length)}</Meta>
        <Meta label={t("Kodlama")}>
          <Mono>{detail.encoding}</Mono>
        </Meta>
      </dl>

      {editingTtl && (
        <TtlEditor
          ttl={detail.ttl}
          busy={busy}
          onClose={() => setEditingTtl(false)}
          onSave={async (seconds) => {
            if (await onSetTtl(seconds)) setEditingTtl(false);
          }}
        />
      )}
    </div>
  );
}

function TtlEditor({ ttl, busy, onSave, onClose }: { ttl: number; busy: boolean; onSave: (seconds: number | null) => Promise<void>; onClose: () => void }) {
  const [text, setText] = useState(ttl > 0 ? String(ttl) : "");
  const seconds = Number(text);
  const valid = text.trim() !== "" && Number.isInteger(seconds) && seconds > 0;

  return (
    <form
      className="mt-3 flex flex-wrap items-center gap-2"
      onSubmit={(event) => {
        event.preventDefault();
        if (valid) void onSave(seconds);
      }}
    >
      <input
        type="number"
        min={1}
        inputMode="numeric"
        aria-label={t("Saniye")}
        placeholder={t("saniye")}
        value={text}
        autoFocus
        onChange={(event) => setText(event.target.value)}
        className={`${FIELD_INPUT} w-32`}
      />
      <Button size="sm" onClick={() => setText("3600")}>
        {t("1 sa")}
      </Button>
      <Button size="sm" onClick={() => setText("86400")}>
        {t("1 gün")}
      </Button>
      <Button size="sm" onClick={() => setText("604800")}>
        {t("7 gün")}
      </Button>
      <Button size="sm" variant="primary" type="submit" disabled={!valid} loading={busy}>
        {t("Kaydet")}
      </Button>
      <Button size="sm" disabled={ttl < 0 || busy} onClick={() => void onSave(null)}>
        {t("Süreyi kaldır")}
      </Button>
      <Button size="sm" variant="ghost" onClick={onClose}>
        {t("Vazgeç")}
      </Button>
    </form>
  );
}

// ---- Satır içi düzenleme parçaları ----

/** Tek satırlık (ya da çok satırlı) değer düzenleyici: Enter kaydeder, Esc vazgeçer. */
function InlineEditor({
  initial,
  label,
  multiline,
  saving,
  inputMode,
  onSave,
  onCancel,
}: {
  initial: string;
  label: string;
  multiline?: boolean;
  saving: boolean;
  inputMode?: "decimal";
  onSave: (value: string) => void | Promise<void>;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const area = multiline || initial.includes("\n");

  return (
    <div className="flex items-start gap-1">
      {area ? (
        <textarea
          autoFocus
          rows={3}
          aria-label={label}
          value={text}
          spellCheck={false}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") onCancel();
            else if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) void onSave(text);
          }}
          className={`${FIELD_AREA} w-full flex-1`}
        />
      ) : (
        <input
          autoFocus
          aria-label={label}
          value={text}
          inputMode={inputMode}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Escape") onCancel();
            else if (event.key === "Enter") {
              event.preventDefault();
              void onSave(text);
            }
          }}
          className={`${FIELD_INPUT} w-full flex-1`}
        />
      )}
      <IconButton label={t("Kaydet")} disabled={saving} className="size-9 text-ok" onClick={() => void onSave(text)}>
        <Check className="size-4" />
      </IconButton>
      <IconButton label={t("Vazgeç")} className="size-9" onClick={onCancel}>
        <X className="size-4" />
      </IconButton>
    </div>
  );
}

interface AddField {
  name: string;
  placeholder: string;
  required?: boolean;
  grow?: boolean;
  inputMode?: "decimal";
}

/** Koleksiyonlara öğe ekleme satırı; başarıda alanlar temizlenir. */
function AddBar({ fields, submitLabel, busy, onSubmit }: { fields: AddField[]; submitLabel: string; busy: boolean; onSubmit: (values: Record<string, string>) => Promise<boolean> }) {
  const [values, setValues] = useState<Record<string, string>>({});
  const ready = fields.every((field) => !field.required || (values[field.name] ?? "").trim() !== "");

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!ready) return;
    if (await onSubmit(values)) setValues({});
  }

  return (
    <form onSubmit={(event) => void submit(event)} className="mb-3 flex flex-wrap items-center gap-2">
      {fields.map((field) => (
        <input
          key={field.name}
          aria-label={field.placeholder}
          placeholder={field.placeholder}
          value={values[field.name] ?? ""}
          inputMode={field.inputMode}
          spellCheck={false}
          autoComplete="off"
          onChange={(event) => setValues((current) => ({ ...current, [field.name]: event.target.value }))}
          className={classNames(FIELD_INPUT, field.grow ? "min-w-40 flex-[2]" : "min-w-32 flex-1")}
        />
      ))}
      <Button type="submit" icon={<Plus className="size-4" />} disabled={!ready} loading={busy}>
        {submitLabel}
      </Button>
    </form>
  );
}

/** Uzun ya da çok satırlı değeri kısaltır; açılınca JSON ise biçimli/ham seçeneği sunar. */
function ExpandableValue({ value }: { value: string }) {
  const [open, setOpen] = useState(false);
  const [raw, setRaw] = useState(false);
  const pretty = useMemo(() => (open ? prettyJson(value) : null), [open, value]);

  if (value === "") return <span className="text-[13px] text-ink-3">{t("(boş)")}</span>;

  const long = value.length > PREVIEW_CHARS || value.includes("\n");
  if (!long) return <Mono className="break-all">{value}</Mono>;

  if (!open) {
    return (
      <div>
        <div className="line-clamp-2 font-mono text-[13px] break-all">{value.slice(0, PREVIEW_CHARS)}</div>
        <button type="button" className={classNames(LINK_BUTTON, "mt-0.5")} onClick={() => setOpen(true)}>
          {t("tamamını göster")}
        </button>
      </div>
    );
  }

  return (
    <div className="flex flex-col items-start gap-1.5">
      {pretty !== null && (
        <Segmented
          compact
          label={t("Değer biçimi")}
          value={raw ? "raw" : "pretty"}
          onChange={(next) => setRaw(next === "raw")}
          options={[
            { id: "pretty", label: t("Biçimli") },
            { id: "raw", label: t("Ham") },
          ]}
        />
      )}
      <pre className="max-h-72 w-full overflow-auto rounded-md border border-line bg-sunken/50 p-2.5 font-mono text-[13px] break-all whitespace-pre-wrap">
        {pretty !== null && !raw ? pretty : value}
      </pre>
      <button type="button" className={LINK_BUTTON} onClick={() => setOpen(false)}>
        {t("daralt")}
      </button>
    </div>
  );
}

function RowActions({ children }: { children: ReactNode }) {
  return <div className="flex justify-end gap-0.5">{children}</div>;
}

function FilterBox({ value, onChange, placeholder }: { value: string; onChange: (text: string) => void; placeholder: string }) {
  return (
    <input
      type="search"
      aria-label={placeholder}
      placeholder={placeholder}
      value={value}
      spellCheck={false}
      autoComplete="off"
      onChange={(event) => onChange(event.target.value)}
      className={`${FIELD_INPUT} mb-3 w-full max-w-sm`}
    />
  );
}

function NoMatches({ filtered }: { filtered: boolean }) {
  return <p className="py-6 text-center text-ink-3">{filtered ? t("Süzgece uyan öğe yok.") : t("Bu anahtarda öğe yok.")}</p>;
}

/** Kaydırılabilir, sütun genişlikleri sabit tablo kabuğu. */
function ValueTable({ head, widths, children }: { head: ReactNode[]; widths: string[]; children: ReactNode }) {
  return (
    <div className="overflow-x-auto rounded-md border border-line">
      <table className="w-full table-fixed border-collapse text-left">
        <colgroup>
          {widths.map((width, index) => (
            <col key={index} className={width} />
          ))}
        </colgroup>
        <thead>
          <tr>
            {head.map((label, index) => (
              <th key={index} scope="col" className="border-b border-line bg-sunken/60 px-3 py-2 text-[13px] font-medium whitespace-nowrap text-ink-2">
                {label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>{children}</tbody>
      </table>
    </div>
  );
}

const CELL = "border-b border-line px-3 py-2 align-top last:border-b-0";

// ---- Türe göre gövdeler ----

function StringView({ keyName, bytes, value, ops }: { keyName: string; bytes: number; value: StringValue | undefined; ops: KeyOps }) {
  const text = value?.text ?? "";
  const pretty = useMemo(() => prettyJson(text), [text]);
  const [mode, setMode] = useState<"pretty" | "raw">("pretty");
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [draftIsFormatted, setDraftIsFormatted] = useState(false);

  if (!value) return <NoMatches filtered={false} />;

  const showPretty = pretty !== null && mode === "pretty";
  const canEdit = !value.truncated && !value.binary;

  function startEdit() {
    setDraft(showPretty && pretty !== null ? pretty : text);
    setDraftIsFormatted(showPretty);
    setEditing(true);
  }

  async function save() {
    if (await ops.run(keyName, "set_string", { value: draft }, t("Değer kaydedildi."))) setEditing(false);
  }

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {pretty !== null && !editing && (
          <Segmented
            compact
            label={t("Değer biçimi")}
            value={mode}
            onChange={setMode}
            options={[
              { id: "pretty", label: t("Biçimli") },
              { id: "raw", label: t("Ham") },
            ]}
          />
        )}
        <span className="text-[13px] text-ink-3">{t("{size} ({n} bayt)", { size: formatBytes(value.bytes), n: formatNumber(value.bytes) })}</span>
        {!editing && (
          <Button size="sm" className="ml-auto" disabled={!canEdit} icon={<Pencil className="size-3.5" />} onClick={startEdit}>
            {t("Düzenle")}
          </Button>
        )}
      </div>

      {!canEdit && (
        <p className="text-[13px] text-warn">
          {value.binary
            ? t("Bu değer ikili veri içeriyor; burada yalnızca önizlenir, düzenlenemez.")
            : t("Değer çok büyük olduğu için kısaltıldı ({size}); düzenlenemez.", { size: formatBytes(bytes) })}
        </p>
      )}

      {editing ? (
        <>
          {draftIsFormatted && <p className="text-[13px] text-warn">{t("Biçimli metin düzenleniyor; kaydettiğinizde değer bu biçimli hâliyle yazılır, özgün boşluklar değişir.")}</p>}
          <textarea
            autoFocus
            aria-label={t("Değer")}
            value={draft}
            spellCheck={false}
            onChange={(event) => setDraft(event.target.value)}
            className={`${FIELD_AREA} h-[min(50vh,28rem)] min-h-40 w-full resize-y`}
          />
          <div className="flex gap-2">
            <Button variant="primary" loading={ops.pending} onClick={() => void save()}>
              {t("Kaydet")}
            </Button>
            <Button disabled={ops.pending} onClick={() => setEditing(false)}>
              {t("Vazgeç")}
            </Button>
          </div>
        </>
      ) : (
        <pre className="max-h-[60vh] min-h-24 overflow-auto rounded-md border border-line bg-sunken/50 p-3 font-mono text-[13px] break-all whitespace-pre-wrap">
          {showPretty && pretty !== null ? pretty : text}
        </pre>
      )}
    </div>
  );
}

function HashView({ keyName, fields, filter, onFilter, ops }: { keyName: string; fields: HashField[]; filter: string; onFilter: (text: string) => void; ops: KeyOps }) {
  const [editing, setEditing] = useState<string | null>(null);

  return (
    <div>
      <AddBar
        fields={[
          { name: "field", placeholder: t("Alan"), required: true },
          { name: "value", placeholder: t("Değer"), grow: true },
        ]}
        submitLabel={t("Alan ekle")}
        busy={ops.pending}
        onSubmit={(values) => ops.run(keyName, "hash_set", { field: values.field ?? "", value: values.value ?? "" }, t("Alan eklendi."))}
      />
      <FilterBox value={filter} onChange={onFilter} placeholder={t("Alan adında ara")} />
      {fields.length === 0 ? (
        <NoMatches filtered={filter.trim() !== ""} />
      ) : (
        <ValueTable head={[t("Alan"), t("Değer"), <span key="a" className="sr-only">{t("İşlemler")}</span>]} widths={["w-[30%]", "", "w-20"]}>
          {fields.map((item) => (
            <tr key={item.field}>
              <td className={CELL}>
                <Mono className="break-all">{item.field}</Mono>
              </td>
              <td className={CELL}>
                {editing === item.field ? (
                  <InlineEditor
                    initial={item.value}
                    label={t("Değer")}
                    saving={ops.pending}
                    onCancel={() => setEditing(null)}
                    onSave={async (value) => {
                      if (await ops.run(keyName, "hash_set", { field: item.field, value }, t("Alan güncellendi."))) setEditing(null);
                    }}
                  />
                ) : (
                  <ExpandableValue value={item.value} />
                )}
              </td>
              <td className={CELL}>
                <RowActions>
                  <IconButton label={t("Değeri düzenle")} disabled={ops.pending} onClick={() => setEditing(item.field)}>
                    <Pencil className="size-4" />
                  </IconButton>
                  <IconButton label={t("Alanı sil")} className="hover:text-bad" disabled={ops.pending} onClick={() => void ops.run(keyName, "hash_delete", { field: item.field }, t("Alan silindi."))}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </RowActions>
              </td>
            </tr>
          ))}
        </ValueTable>
      )}
    </div>
  );
}

function ListView({ keyName, items, offset, ops }: { keyName: string; items: string[]; offset: number; ops: KeyOps }) {
  const [editing, setEditing] = useState<number | null>(null);

  return (
    <div>
      <AddBar
        fields={[{ name: "value", placeholder: t("Eklenecek değer"), grow: true }]}
        submitLabel={t("Sona ekle")}
        busy={ops.pending}
        onSubmit={(values) => ops.run(keyName, "list_push", { value: values.value ?? "" }, t("Değer listenin sonuna eklendi."), { listChanged: true })}
      />
      {items.length === 0 ? (
        <NoMatches filtered={false} />
      ) : (
        <ValueTable head={["#", t("Değer"), <span key="a" className="sr-only">{t("İşlemler")}</span>]} widths={["w-16", "", "w-20"]}>
          {items.map((item, position) => {
            const index = offset + position;
            return (
              <tr key={index}>
                <td className={CELL}>
                  <Mono className="text-ink-3">{index}</Mono>
                </td>
                <td className={CELL}>
                  {editing === index ? (
                    <InlineEditor
                      initial={item}
                      label={t("Değer")}
                      saving={ops.pending}
                      onCancel={() => setEditing(null)}
                      onSave={async (value) => {
                        if (await ops.run(keyName, "list_set", { index, value }, t("Öğe güncellendi."))) setEditing(null);
                      }}
                    />
                  ) : (
                    <ExpandableValue value={item} />
                  )}
                </td>
                <td className={CELL}>
                  <RowActions>
                    <IconButton label={t("Değeri düzenle")} disabled={ops.pending} onClick={() => setEditing(index)}>
                      <Pencil className="size-4" />
                    </IconButton>
                    <IconButton
                      label={t("Sil")}
                      title={t("Listede bu değere eşit ilk öğe silinir (yinelenenler varsa bu satır olmayabilir).")}
                      className="hover:text-bad"
                      disabled={ops.pending}
                      onClick={() => void ops.run(keyName, "list_remove", { value: item }, t("Öğe silindi."), { listChanged: true })}
                    >
                      <Trash2 className="size-4" />
                    </IconButton>
                  </RowActions>
                </td>
              </tr>
            );
          })}
        </ValueTable>
      )}
    </div>
  );
}

function SetView({ keyName, members, filter, onFilter, ops }: { keyName: string; members: string[]; filter: string; onFilter: (text: string) => void; ops: KeyOps }) {
  return (
    <div>
      <AddBar
        fields={[{ name: "member", placeholder: t("Yeni üye"), required: true, grow: true }]}
        submitLabel={t("Üye ekle")}
        busy={ops.pending}
        onSubmit={(values) => ops.run(keyName, "set_add", { member: values.member ?? "" }, t("Üye eklendi."), { listChanged: true })}
      />
      <FilterBox value={filter} onChange={onFilter} placeholder={t("Üyelerde ara")} />
      {members.length === 0 ? (
        <NoMatches filtered={filter.trim() !== ""} />
      ) : (
        <ValueTable head={[t("Üye"), <span key="a" className="sr-only">{t("İşlemler")}</span>]} widths={["", "w-16"]}>
          {members.map((member) => (
            <tr key={member}>
              <td className={CELL}>
                <ExpandableValue value={member} />
              </td>
              <td className={CELL}>
                <RowActions>
                  <IconButton label={t("Üyeyi sil")} className="hover:text-bad" disabled={ops.pending} onClick={() => void ops.run(keyName, "set_remove", { member }, t("Üye silindi."), { listChanged: true })}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </RowActions>
              </td>
            </tr>
          ))}
        </ValueTable>
      )}
    </div>
  );
}

function ZsetView({ keyName, pairs, ops }: { keyName: string; pairs: ZsetPair[]; ops: KeyOps }) {
  const feedback = useFeedback();
  const [editing, setEditing] = useState<string | null>(null);

  return (
    <div>
      <AddBar
        fields={[
          { name: "member", placeholder: t("Üye"), required: true, grow: true },
          { name: "score", placeholder: t("Puan"), required: true, inputMode: "decimal" },
        ]}
        submitLabel={t("Üye ekle")}
        busy={ops.pending}
        onSubmit={async (values) => {
          const score = Number(values.score);
          if (!Number.isFinite(score)) {
            feedback.error(new Error(t("Puan geçerli bir sayı olmalı.")));
            return false;
          }
          return ops.run(keyName, "zset_add", { member: values.member ?? "", score }, t("Üye eklendi."), { listChanged: true });
        }}
      />
      {pairs.length === 0 ? (
        <NoMatches filtered={false} />
      ) : (
        <ValueTable head={[t("Puan"), t("Üye"), <span key="a" className="sr-only">{t("İşlemler")}</span>]} widths={["w-44", "", "w-20"]}>
          {pairs.map((pair) => (
            <tr key={pair.member}>
              <td className={CELL}>
                {editing === pair.member ? (
                  <InlineEditor
                    initial={pair.score}
                    label={t("Puan")}
                    inputMode="decimal"
                    saving={ops.pending}
                    onCancel={() => setEditing(null)}
                    onSave={async (value) => {
                      const score = Number(value);
                      if (value.trim() === "" || !Number.isFinite(score)) {
                        feedback.error(new Error(t("Puan geçerli bir sayı olmalı.")));
                        return;
                      }
                      if (await ops.run(keyName, "zset_add", { member: pair.member, score }, t("Puan güncellendi."))) setEditing(null);
                    }}
                  />
                ) : (
                  <Mono>{pair.score}</Mono>
                )}
              </td>
              <td className={CELL}>
                <ExpandableValue value={pair.member} />
              </td>
              <td className={CELL}>
                <RowActions>
                  <IconButton label={t("Puanı düzenle")} disabled={ops.pending} onClick={() => setEditing(pair.member)}>
                    <Pencil className="size-4" />
                  </IconButton>
                  <IconButton label={t("Üyeyi sil")} className="hover:text-bad" disabled={ops.pending} onClick={() => void ops.run(keyName, "zset_remove", { member: pair.member }, t("Üye silindi."), { listChanged: true })}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </RowActions>
              </td>
            </tr>
          ))}
        </ValueTable>
      )}
    </div>
  );
}

function StreamView({ entries }: { entries: StreamEntry[] }) {
  return (
    <div className="flex flex-col gap-3">
      <p className="text-[13px] text-ink-3">{t("En yeni 100 kayıt gösteriliyor. Akışlar burada yalnızca okunur.")}</p>
      {entries.length === 0 && <NoMatches filtered={false} />}
      {entries.map((entry) => (
        <div key={entry.id} className="rounded-md border border-line px-3 py-2">
          <Mono className="text-accent">{entry.id}</Mono>
          <dl className="mt-1.5 grid grid-cols-[minmax(6rem,max-content)_minmax(0,1fr)] gap-x-4 gap-y-1">
            {entry.fields.map((item, index) => (
              <div key={index} className="contents">
                <dt>
                  <Mono className="break-all text-ink-2">{item.field}</Mono>
                </dt>
                <dd>
                  <Mono className="break-all whitespace-pre-wrap">{item.value}</Mono>
                </dd>
              </div>
            ))}
          </dl>
        </div>
      ))}
    </div>
  );
}

// ---- Yeni anahtar ----

function NewKeyModal({ db, onClose, onCreated }: { db: number; onClose: () => void; onCreated: (key: string) => void }) {
  const [name, setName] = useState("");
  const [type, setType] = useState<RedisType>("string");
  const [first, setFirst] = useState("");
  const [second, setSecond] = useState("");
  const [ttl, setTtl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setError(null);

    const key = name.trim();
    if (key === "") return setError(t("Anahtar adı boş olamaz."));
    let score = 0;
    if (type === "zset") {
      score = Number(second);
      if (second.trim() === "" || !Number.isFinite(score)) return setError(t("Puan geçerli bir sayı olmalı."));
    }
    if (type === "hash" && first.trim() === "") return setError(t("Alan adı boş olamaz."));
    if ((type === "set" || type === "zset") && first === "") return setError(t("Üye boş olamaz."));
    const seconds = Number(ttl);
    if (ttl.trim() !== "" && (!Number.isInteger(seconds) || seconds < 1)) return setError(t("Süre, 1 ya da daha büyük bir tam sayı (saniye) olmalı."));

    setBusy(true);
    try {
      // set_string var olan anahtarın üzerine yazar; bu yüzden önce anahtarın olmadığı doğrulanır.
      let exists = true;
      try {
        await api.get(`/redis/key?db=${db}&key=${enc(key)}&cursor=&filter=`);
      } catch (probeError) {
        if (!(probeError instanceof ApiError && probeError.status === 404)) throw probeError;
        exists = false;
      }
      if (exists) return setError(t("Bu adda bir anahtar zaten var."));

      let op: KeyOp;
      let args: OpArgs;
      switch (type) {
        case "hash":
          op = "hash_set";
          args = { field: first, value: second };
          break;
        case "list":
          op = "list_push";
          args = { value: first };
          break;
        case "set":
          op = "set_add";
          args = { member: first };
          break;
        case "zset":
          op = "zset_add";
          args = { member: first, score };
          break;
        default:
          op = "set_string";
          args = { value: first };
      }
      await api.post<{ result: string }>("/redis/key", { db, key, op, ...args });
      if (ttl.trim() !== "") await api.post<{ result: string }>("/redis/key", { db, key, op: "set_ttl", seconds });
      onCreated(key);
    } catch (failure) {
      setError(errorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      open
      wide
      onClose={onClose}
      title={t("Yeni anahtar: db{n}", { n: db })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="redis-new-key-form" loading={busy}>
            {t("Oluştur")}
          </Button>
        </>
      }
    >
      <form id="redis-new-key-form" onSubmit={(event) => void submit(event)} className="flex flex-col gap-5">
        <Input label={t("Anahtar adı")} value={name} autoFocus spellCheck={false} autoComplete="off" className="font-mono text-[13.5px]" placeholder="oturum:abc123" onChange={(event) => setName(event.target.value)} />
        <Select
          label={t("Tür")}
          value={type}
          onChange={(event) => {
            setType(event.target.value as RedisType);
            setFirst("");
            setSecond("");
          }}
        >
          {CREATABLE_TYPES.map((item) => (
            <option key={item} value={item}>
              {item}
            </option>
          ))}
        </Select>

        {type === "string" && <Textarea label={t("Değer")} value={first} spellCheck={false} onChange={(event) => setFirst(event.target.value)} />}
        {type === "hash" && (
          <div className="grid gap-4 sm:grid-cols-2">
            <Input label={t("Alan")} value={first} spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setFirst(event.target.value)} />
            <Input label={t("Değer")} value={second} spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setSecond(event.target.value)} />
          </div>
        )}
        {type === "list" && <Input label={t("İlk değer")} value={first} spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setFirst(event.target.value)} />}
        {type === "set" && <Input label={t("İlk üye")} value={first} spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setFirst(event.target.value)} />}
        {type === "zset" && (
          <div className="grid gap-4 sm:grid-cols-2">
            <Input label={t("Üye")} value={first} spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setFirst(event.target.value)} />
            <Input label={t("Puan")} value={second} inputMode="decimal" spellCheck={false} className="font-mono text-[13.5px]" onChange={(event) => setSecond(event.target.value)} />
          </div>
        )}

        <Input label={t("Süre (saniye, isteğe bağlı)")} hint={t("Boş bırakırsanız anahtar süresiz kalır.")} type="number" min={1} inputMode="numeric" value={ttl} onChange={(event) => setTtl(event.target.value)} />

        {error && <ErrorBlock message={error} />}
      </form>
    </Modal>
  );
}
