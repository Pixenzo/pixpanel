import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Archive,
  ChevronRight,
  ClipboardPaste,
  Copy,
  Download,
  File as FileIcon,
  FilePlus2,
  Folder,
  FolderPlus,
  Link2,
  PackageOpen,
  Pencil,
  Scissors,
  ShieldCheck,
  Trash2,
  Upload,
} from "lucide-react";
import type { DragEvent, FormEvent } from "react";
import { Suspense, lazy, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import { Button, Checkbox, ErrorBlock, Input, LoadingBlock, Modal, PageHeader, Panel, Select, Spinner, Table, Td, Th } from "../components/ui";
import { ApiError, api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";

const CodeEditor = lazy(() => import("../components/CodeEditor"));

interface FileEntry {
  name: string;
  kind: "file" | "dir" | "symlink" | "other";
  size: number;
  mode: number;
  mtime: number;
  target: string | null;
}

interface Listing {
  path: string;
  entries: FileEntry[];
}

interface Root {
  id: string;
  label: string;
  path: string;
}

/** Düzenleyicide açılabilecek en büyük dosya (sunucudaki sınırla aynı). */
const MAX_EDIT_BYTES = 2 * 1024 * 1024;
const ARCHIVE_PATTERN = /\.(zip|tar\.gz|tgz|tar)$/i;
/** Metin olmadığı bilinen, tıklanınca düzenleyici yerine indirilen türler. */
const BINARY_PATTERN = /\.(png|jpe?g|gif|webp|avif|ico|pdf|zip|gz|tgz|tar|rar|7z|mp[34]|webm|mov|woff2?|ttf|otf|eot|exe|bin|so|db|sqlite)$/i;

const joinPath = (dir: string, name: string) => (dir === "/" ? `/${name}` : `${dir}/${name}`);
const parentOf = (path: string) => path.slice(0, path.lastIndexOf("/")) || "/";

function fileOp<T = unknown>(scope: string, request: Record<string, unknown>) {
  return api.post<T>("/files/op", { scope, request });
}

const downloadUrl = (scope: string, path: string) =>
  `/api/v1/files/download?scope=${encodeURIComponent(scope)}&path=${encodeURIComponent(path)}`;

/** 0o644 → "rw-r--r--" */
function modeString(mode: number): string {
  const bits = "rwxrwxrwx";
  return bits
    .split("")
    .map((letter, index) => (mode & (1 << (8 - index)) ? letter : "-"))
    .join("");
}

const octal = (mode: number) => (mode & 0o777).toString(8).padStart(3, "0");

/** Dosyayı ilerleme bilgisiyle yükler (fetch yükleme ilerlemesi bildirmediği için XHR). */
function uploadFile(scope: string, dir: string, file: File, overwrite: boolean, onProgress: (ratio: number) => void): Promise<void> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    const query = `scope=${encodeURIComponent(scope)}&dir=${encodeURIComponent(dir)}&overwrite=${overwrite}`;
    request.open("POST", `/api/v1/files/upload?${query}`);
    request.setRequestHeader("x-pixpanel-csrf", "1");
    request.upload.onprogress = (event) => event.lengthComputable && onProgress(event.loaded / event.total);
    request.onerror = () => reject(new ApiError(0, "network", t("Yükleme sırasında bağlantı koptu")));
    request.onload = () => {
      if (request.status >= 200 && request.status < 300) return resolve();
      let message = t("Yükleme başarısız oldu ({status})", { status: request.status });
      try {
        message = JSON.parse(request.responseText).error.message;
      } catch {
        // Yanıt JSON değilse genel ileti kalır.
      }
      reject(new ApiError(request.status, "upload", message));
    };
    const form = new FormData();
    form.append("file", file);
    request.send(form);
  });
}

function NameModal({
  title,
  label,
  initial,
  submitLabel,
  onSubmit,
  onClose,
}: {
  title: string;
  label: string;
  initial: string;
  submitLabel: string;
  onSubmit: (name: string) => Promise<unknown>;
  onClose: () => void;
}) {
  const [name, setName] = useState(initial);
  const action = useMutation({ mutationFn: () => onSubmit(name.trim()), onSuccess: onClose });
  function submit(event: FormEvent) {
    event.preventDefault();
    action.mutate();
  }
  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="file-name-form" loading={action.isPending}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <form id="file-name-form" onSubmit={submit} className="flex flex-col gap-4">
        <Input label={label} value={name} onChange={(event) => setName(event.target.value)} className="font-mono text-[13px]" spellCheck={false} autoFocus required />
        {action.isError && <ErrorBlock message={errorMessage(action.error)} />}
      </form>
    </Modal>
  );
}

function CompressModal({ count, suggested, onSubmit, onClose }: { count: number; suggested: string; onSubmit: (name: string, format: string) => Promise<unknown>; onClose: () => void }) {
  const [name, setName] = useState(suggested);
  const [format, setFormat] = useState("zip");
  const extension = format === "zip" ? ".zip" : ".tar.gz";
  const action = useMutation({ mutationFn: () => onSubmit(name.trim() + extension, format), onSuccess: onClose });
  return (
    <Modal
      open
      onClose={onClose}
      title={t("{count} öğeyi sıkıştır", { count })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="compress-form" loading={action.isPending}>
            {t("Arşivi oluştur")}
          </Button>
        </>
      }
    >
      <form
        id="compress-form"
        onSubmit={(event) => {
          event.preventDefault();
          action.mutate();
        }}
        className="flex flex-col gap-4"
      >
        <Input label={t("Arşiv adı")} value={name} onChange={(event) => setName(event.target.value)} hint={t("Uzantı kendiliğinden eklenir: {ext}", { ext: extension })} className="font-mono text-[13px]" autoFocus required />
        <Select label={t("Biçim")} value={format} onChange={(event) => setFormat(event.target.value)}>
          <option value="zip">ZIP</option>
          <option value="tar_gz">tar.gz</option>
        </Select>
        {action.isError && <ErrorBlock message={errorMessage(action.error)} />}
      </form>
    </Modal>
  );
}

const permissionRows: [string, number][] = [
  ["Sahibi", 6],
  ["Grup", 3],
  ["Diğerleri", 0],
];
const permissionColumns: [string, number][] = [
  ["Okuma", 4],
  ["Yazma", 2],
  ["Çalıştırma", 1],
];

function PermissionsModal({ initial, hasFolder, count, onSubmit, onClose }: { initial: number; hasFolder: boolean; count: number; onSubmit: (mode: number, recursive: boolean) => Promise<unknown>; onClose: () => void }) {
  const [mode, setMode] = useState(initial & 0o777);
  const [recursive, setRecursive] = useState(false);
  const action = useMutation({ mutationFn: () => onSubmit(mode, recursive), onSuccess: onClose });
  return (
    <Modal
      open
      onClose={onClose}
      title={count === 1 ? t("İzinleri değiştir") : t("{count} öğenin izinlerini değiştir", { count })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={action.isPending} onClick={() => action.mutate()}>
            {t("İzinleri uygula")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <table className="w-full text-left">
          <thead>
            <tr>
              <th />
              {permissionColumns.map(([label]) => (
                <th key={label} scope="col" className="pb-2 text-[13px] font-medium text-ink-2">
                  {t(label)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {permissionRows.map(([rowLabel, shift]) => (
              <tr key={rowLabel}>
                <th scope="row" className="py-1.5 pr-4 font-medium">
                  {t(rowLabel)}
                </th>
                {permissionColumns.map(([columnLabel, bit]) => {
                  const mask = bit << shift;
                  return (
                    <td key={columnLabel}>
                      <input
                        type="checkbox"
                        aria-label={`${t(rowLabel)}: ${t(columnLabel)}`}
                        className="size-4 accent-(--accent)"
                        checked={(mode & mask) !== 0}
                        onChange={(event) => setMode((current) => (event.target.checked ? current | mask : current & ~mask))}
                      />
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
        <p className="font-mono text-[13px] text-ink-2">
          {octal(mode)} {modeString(mode)}
        </p>
        {hasFolder && (
          <Checkbox
            label={t("Klasörün içindekilere de uygula")}
            hint={t("Klasörlere, içlerine girilebilmesi için çalıştırma izni kendiliğinden eklenir.")}
            checked={recursive}
            onChange={(event) => setRecursive(event.target.checked)}
          />
        )}
        {action.isError && <ErrorBlock message={errorMessage(action.error)} />}
      </div>
    </Modal>
  );
}

function EditorModal({ scope, path, onClose }: { scope: string; path: string; onClose: () => void }) {
  const feedback = useFeedback();
  const name = path.slice(path.lastIndexOf("/") + 1);
  const file = useQuery({
    queryKey: ["file-content", scope, path],
    queryFn: () => fileOp<{ content: string }>(scope, { op: "read", path }),
    gcTime: 0,
  });
  const [content, setContent] = useState<string | null>(null);
  const dirty = content !== null && content !== file.data?.content;
  const dark = document.documentElement.dataset.theme === "dark";

  const save = useMutation({
    mutationFn: () => fileOp(scope, { op: "write", path, content: content ?? file.data?.content ?? "" }),
    onSuccess: () => {
      feedback.success(t("{name} kaydedildi", { name }));
      void file.refetch();
    },
    onError: (error) => feedback.error(error),
  });

  async function close() {
    if (dirty) {
      const discard = await feedback.confirm({
        title: t("Kaydedilmemiş değişiklikler var"),
        description: t("{name} dosyasındaki değişiklikler kaybolacak.", { name }),
        confirmLabel: t("Kaydetmeden kapat"),
        danger: true,
      });
      if (!discard) return;
    }
    onClose();
  }

  return (
    <div role="dialog" aria-modal="true" aria-label={t("{name} düzenleniyor", { name })} className="fixed inset-0 z-40 flex flex-col bg-surface">
      <div className="flex h-13 shrink-0 items-center gap-3 border-b border-line px-4">
        <span className="min-w-0 flex-1 truncate font-mono text-[13px]" title={path}>
          {path}
          {dirty && <span className="ml-2 text-warn">{t("(kaydedilmedi)")}</span>}
        </span>
        <Button onClick={() => void close()}>{t("Kapat")}</Button>
        <Button variant="primary" disabled={!dirty} loading={save.isPending} onClick={() => save.mutate()}>
          {t("Kaydet")}
        </Button>
      </div>
      <div className="min-h-0 flex-1">
        {file.isPending && <LoadingBlock />}
        {file.isError && (
          <div className="p-4">
            <ErrorBlock message={errorMessage(file.error)} />
          </div>
        )}
        {file.data && (
          <Suspense fallback={<LoadingBlock />}>
            <CodeEditor filename={name} initialValue={file.data.content} dark={dark} onChange={setContent} onSave={() => dirty && save.mutate()} />
          </Suspense>
        )}
      </div>
    </div>
  );
}

type Dialog =
  | { kind: "mkdir" }
  | { kind: "touch" }
  | { kind: "rename"; entry: FileEntry }
  | { kind: "compress" }
  | { kind: "chmod" }
  | { kind: "edit"; path: string };

interface Clipboard {
  mode: "copy" | "cut";
  scope: string;
  paths: string[];
}

interface UploadState {
  name: string;
  ratio: number;
}

export function FilesPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const roots = useQuery({ queryKey: ["file-roots"], queryFn: () => api.get<Root[]>("/files/roots") });
  // Kapsam ve yol adres çubuğunda tutulur: geri tuşu çalışır, sayfa yenilenince aynı klasörde kalınır.
  const [searchParams, setSearchParams] = useSearchParams();
  const scopeChoice = searchParams.get("alan");
  const path = searchParams.get("yol") || "/";
  const navigateTo = (nextScope: string, nextPath: string) => setSearchParams({ alan: nextScope, yol: nextPath });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [clipboard, setClipboard] = useState<Clipboard | null>(null);
  const [uploads, setUploads] = useState<UploadState[]>([]);
  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);

  const scope = roots.data?.find((item) => item.id === scopeChoice)?.id ?? roots.data?.[0]?.id ?? "";
  const root = roots.data?.find((item) => item.id === scope);
  const setPath = (nextPath: string) => navigateTo(scope, nextPath);

  const listing = useQuery({
    queryKey: ["files", scope, path],
    queryFn: () => fileOp<Listing>(scope, { op: "list", path }),
    enabled: scope !== "",
  });

  // Klasör değişince seçim sıfırlanır.
  useEffect(() => setSelected(new Set()), [scope, path]);

  const entries = listing.data?.entries ?? [];
  const selectedEntries = useMemo(() => entries.filter((entry) => selected.has(entry.name)), [entries, selected]);
  const selectedPaths = selectedEntries.map((entry) => joinPath(path, entry.name));
  const single = selectedEntries.length === 1 ? selectedEntries[0] : null;
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["files", scope] });

  /** Bir dosya işlemini çalıştırır, listeyi yeniler ve sonucu bildirir. */
  const run = useMutation({
    mutationFn: ({ request }: { request: Record<string, unknown>; done: string }) => fileOp(scope, request),
    onSuccess: (_, { done }) => {
      feedback.success(done);
      setSelected(new Set());
    },
    onError: (error) => feedback.error(error),
    onSettled: refresh,
  });

  function open(entry: FileEntry) {
    const target = joinPath(path, entry.name);
    if (entry.kind === "dir") return setPath(target);
    if (entry.kind === "file" && entry.size <= MAX_EDIT_BYTES && !BINARY_PATTERN.test(entry.name)) {
      return setDialog({ kind: "edit", path: target });
    }
    window.location.assign(downloadUrl(scope, target));
  }

  async function upload(files: File[]) {
    for (const file of files) {
      const update = (ratio: number) => setUploads((list) => list.map((item) => (item.name === file.name ? { ...item, ratio } : item)));
      setUploads((list) => [...list.filter((item) => item.name !== file.name), { name: file.name, ratio: 0 }]);
      try {
        try {
          await uploadFile(scope, path, file, false, update);
        } catch (error) {
          // Aynı adlı dosya varsa üzerine yazmak için onay istenir.
          if (!(error instanceof ApiError) || !error.message.includes("zaten var")) throw error;
          const overwrite = await feedback.confirm({
            title: t("{name} zaten var", { name: file.name }),
            description: t("Üzerine yazarsanız eski dosyanın içeriği kaybolur."),
            confirmLabel: t("Üzerine yaz"),
            danger: true,
          });
          if (overwrite) await uploadFile(scope, path, file, true, update);
        }
      } catch (error) {
        feedback.error(error);
      } finally {
        setUploads((list) => list.filter((item) => item.name !== file.name));
      }
    }
    void refresh();
  }

  function onDrop(event: DragEvent) {
    event.preventDefault();
    setDragging(false);
    const files = Array.from(event.dataTransfer.files);
    if (files.length > 0) void upload(files);
  }

  async function remove() {
    const confirmed = await feedback.confirm({
      title: single ? t("{name} silinsin mi?", { name: single.name }) : t("{count} öğe silinsin mi?", { count: selectedEntries.length }),
      description: selectedEntries.some((entry) => entry.kind === "dir")
        ? t("Klasörler içindekilerle birlikte kalıcı olarak silinir. Bu işlem geri alınamaz.")
        : t("Dosyalar kalıcı olarak silinir. Bu işlem geri alınamaz."),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) run.mutate({ request: { op: "delete", paths: selectedPaths }, done: t("{count} öğe silindi", { count: selectedPaths.length }) });
  }

  function paste() {
    if (!clipboard) return;
    run.mutate(
      {
        request: { op: clipboard.mode === "copy" ? "copy" : "move", paths: clipboard.paths, dest: path },
        done: clipboard.mode === "copy" ? t("{count} öğe kopyalandı", { count: clipboard.paths.length }) : t("{count} öğe taşındı", { count: clipboard.paths.length }),
      },
      { onSuccess: () => clipboard.mode === "cut" && setClipboard(null) },
    );
  }

  if (roots.isPending) return <LoadingBlock />;
  if (roots.isError) return <ErrorBlock message={errorMessage(roots.error)} />;

  const crumbs = path.split("/").filter(Boolean);
  const allSelected = entries.length > 0 && selected.size === entries.length;
  const toolButton = "h-8";

  return (
    <>
      <PageHeader
        title={t("Dosyalar")}
        description={root ? t("{path} klasörünün içindesiniz.", { path: root.path }) : undefined}
        actions={
          roots.data.length > 1 ? (
            <select
              aria-label={t("Dosya alanı")}
              value={scope}
              onChange={(event) => navigateTo(event.target.value, "/")}
              className="h-9 rounded-md border border-line-strong bg-surface px-3"
            >
              {roots.data.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.label}
                </option>
              ))}
            </select>
          ) : undefined
        }
      />

      <Panel flush className={classNames(dragging && "outline-2 outline-accent")}>
        <div onDragOver={(event) => (event.preventDefault(), setDragging(true))} onDragLeave={() => setDragging(false)} onDrop={onDrop}>
          <nav aria-label={t("Klasör yolu")} className="flex flex-wrap items-center gap-1 border-b border-line px-3 py-2 font-mono text-[13px]">
            <button type="button" className="rounded px-1.5 py-0.5 font-medium hover:bg-sunken" onClick={() => setPath("/")}>
              {root?.label ?? "/"}
            </button>
            {crumbs.map((crumb, index) => (
              <span key={index} className="flex items-center gap-1">
                <ChevronRight aria-hidden className="size-3.5 text-ink-3" />
                <button
                  type="button"
                  className="rounded px-1.5 py-0.5 hover:bg-sunken"
                  aria-current={index === crumbs.length - 1 ? "location" : undefined}
                  onClick={() => setPath(`/${crumbs.slice(0, index + 1).join("/")}`)}
                >
                  {crumb}
                </button>
              </span>
            ))}
          </nav>

          <div className="flex flex-wrap items-center gap-1.5 border-b border-line px-3 py-2">
            <input ref={fileInput} type="file" multiple hidden onChange={(event) => (void upload(Array.from(event.target.files ?? [])), (event.target.value = ""))} />
            {selectedEntries.length === 0 ? (
              <>
                <Button size="sm" className={toolButton} icon={<Upload className="size-3.5" />} onClick={() => fileInput.current?.click()}>
                  {t("Yükle")}
                </Button>
                <Button size="sm" className={toolButton} icon={<FolderPlus className="size-3.5" />} onClick={() => setDialog({ kind: "mkdir" })}>
                  {t("Yeni klasör")}
                </Button>
                <Button size="sm" className={toolButton} icon={<FilePlus2 className="size-3.5" />} onClick={() => setDialog({ kind: "touch" })}>
                  {t("Yeni dosya")}
                </Button>
                <span className="ml-auto hidden text-[12.5px] text-ink-3 sm:block">{t("Yüklemek için dosyaları buraya sürükleyebilirsiniz.")}</span>
              </>
            ) : (
              <>
                <span className="mr-1 text-[13px] font-medium">{t("{count} öğe seçili", { count: selectedEntries.length })}</span>
                {single && (
                  <a href={downloadUrl(scope, joinPath(path, single.name))} className="inline-flex h-8 items-center gap-1.5 rounded-md border border-line-strong bg-surface px-2.5 text-[13px] font-medium hover:bg-sunken">
                    <Download className="size-3.5" /> {single.kind === "dir" ? t("Arşiv olarak indir") : t("İndir")}
                  </a>
                )}
                {single && (
                  <Button size="sm" className={toolButton} icon={<Pencil className="size-3.5" />} onClick={() => setDialog({ kind: "rename", entry: single })}>
                    {t("Yeniden adlandır")}
                  </Button>
                )}
                <Button size="sm" className={toolButton} icon={<Copy className="size-3.5" />} onClick={() => (setClipboard({ mode: "copy", scope, paths: selectedPaths }), setSelected(new Set()))}>
                  {t("Kopyala")}
                </Button>
                <Button size="sm" className={toolButton} icon={<Scissors className="size-3.5" />} onClick={() => (setClipboard({ mode: "cut", scope, paths: selectedPaths }), setSelected(new Set()))}>
                  {t("Kes")}
                </Button>
                <Button size="sm" className={toolButton} icon={<Archive className="size-3.5" />} onClick={() => setDialog({ kind: "compress" })}>
                  {t("Sıkıştır")}
                </Button>
                {single && ARCHIVE_PATTERN.test(single.name) && (
                  <Button
                    size="sm"
                    className={toolButton}
                    icon={<PackageOpen className="size-3.5" />}
                    loading={run.isPending}
                    onClick={() => {
                      const folder = single.name.replace(ARCHIVE_PATTERN, "");
                      run.mutate({
                        request: { op: "extract", path: joinPath(path, single.name), dest: joinPath(path, folder) },
                        done: t("Arşiv {folder} klasörüne açıldı", { folder }),
                      });
                    }}
                  >
                    {t("Arşivi aç")}
                  </Button>
                )}
                <Button size="sm" className={toolButton} icon={<ShieldCheck className="size-3.5" />} onClick={() => setDialog({ kind: "chmod" })}>
                  {t("İzinler")}
                </Button>
                <Button size="sm" className={toolButton} icon={<Trash2 className="size-3.5" />} onClick={() => void remove()}>
                  {t("Sil")}
                </Button>
              </>
            )}
          </div>

          {clipboard && clipboard.scope === scope && (
            <div className="flex flex-wrap items-center gap-3 border-b border-line bg-accent-soft px-3 py-2 text-accent">
              <span className="text-[13px] font-medium">
                {clipboard.mode === "copy" ? t("{count} öğe kopyalanmak üzere bekliyor", { count: clipboard.paths.length }) : t("{count} öğe taşınmak üzere bekliyor", { count: clipboard.paths.length })}
              </span>
              <Button size="sm" variant="primary" icon={<ClipboardPaste className="size-3.5" />} loading={run.isPending} onClick={paste}>
                {t("Buraya yapıştır")}
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setClipboard(null)}>
                {t("Vazgeç")}
              </Button>
            </div>
          )}

          {uploads.length > 0 && (
            <ul className="border-b border-line px-3 py-2">
              {uploads.map((item) => (
                <li key={item.name} className="flex items-center gap-3 py-1 text-[13px]">
                  <Spinner className="size-3.5 text-accent" />
                  <span className="min-w-0 flex-1 truncate font-mono">{item.name}</span>
                  <span className="h-1.5 w-40 overflow-hidden rounded-full bg-sunken">
                    <span className="block h-full rounded-full bg-accent transition-[width]" style={{ width: `${Math.round(item.ratio * 100)}%` }} />
                  </span>
                  <span className="tabular w-10 text-right text-ink-2">%{Math.round(item.ratio * 100)}</span>
                </li>
              ))}
            </ul>
          )}

          {listing.isPending && <LoadingBlock />}
          {listing.isError && (
            <div className="p-4">
              <ErrorBlock message={errorMessage(listing.error)} onRetry={() => void listing.refetch()} />
            </div>
          )}
          {listing.data && (
            <Table>
              <thead>
                <tr>
                  <Th className="w-8">
                    <input
                      type="checkbox"
                      aria-label={t("Tümünü seç")}
                      className="size-4 accent-(--accent)"
                      checked={allSelected}
                      onChange={(event) => setSelected(event.target.checked ? new Set(entries.map((entry) => entry.name)) : new Set())}
                    />
                  </Th>
                  <Th className="w-full">{t("Ad")}</Th>
                  <Th align="right">{t("Boyut")}</Th>
                  <Th>{t("Değiştirilme")}</Th>
                  <Th>{t("İzinler")}</Th>
                </tr>
              </thead>
              <tbody>
                {path !== "/" && (
                  <tr className="hover:bg-sunken/50">
                    <Td />
                    <Td>
                      <button type="button" className="flex items-center gap-2.5 font-mono text-[13px] text-ink-2" onClick={() => setPath(parentOf(path))}>
                        <Folder aria-hidden className="size-4" /> ..
                      </button>
                    </Td>
                    <Td />
                    <Td />
                    <Td />
                  </tr>
                )}
                {entries.map((entry) => {
                  const Icon = entry.kind === "dir" ? Folder : entry.kind === "symlink" ? Link2 : FileIcon;
                  const isSelected = selected.has(entry.name);
                  return (
                    <tr key={entry.name} className={classNames(isSelected ? "bg-accent-soft/60" : "hover:bg-sunken/50")}>
                      <Td>
                        <input
                          type="checkbox"
                          aria-label={t("{name} öğesini seç", { name: entry.name })}
                          className="size-4 accent-(--accent)"
                          checked={isSelected}
                          onChange={(event) =>
                            setSelected((current) => {
                              const next = new Set(current);
                              if (event.target.checked) next.add(entry.name);
                              else next.delete(entry.name);
                              return next;
                            })
                          }
                        />
                      </Td>
                      <Td className="max-w-0">
                        <button type="button" className="flex max-w-full items-center gap-2.5 text-left font-mono text-[13px] hover:text-accent" onClick={() => open(entry)}>
                          <Icon aria-hidden className={classNames("size-4 shrink-0", entry.kind === "dir" ? "text-accent" : "text-ink-3")} />
                          <span className="truncate">{entry.name}</span>
                          {entry.target && <span className="truncate text-ink-3">→ {entry.target}</span>}
                        </button>
                      </Td>
                      <Td align="right" className="whitespace-nowrap text-ink-2">
                        {entry.kind === "file" ? formatBytes(entry.size) : ""}
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2">{formatDateTime(entry.mtime)}</Td>
                      <Td className="font-mono text-[12.5px] whitespace-nowrap text-ink-2">
                        <span title={modeString(entry.mode)}>{octal(entry.mode)}</span>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          )}
          {listing.data && entries.length === 0 && <p className="px-4 py-8 text-ink-2">{t("Bu klasör boş. Dosya yükleyin ya da yeni bir klasör oluşturun.")}</p>}
        </div>
      </Panel>

      {dialog?.kind === "mkdir" && (
        <NameModal
          title={t("Yeni klasör")}
          label={t("Klasör adı")}
          initial=""
          submitLabel={t("Klasörü oluştur")}
          onClose={() => setDialog(null)}
          onSubmit={(name) => fileOp(scope, { op: "mkdir", path: joinPath(path, name) }).then(refresh)}
        />
      )}
      {dialog?.kind === "touch" && (
        <NameModal
          title={t("Yeni dosya")}
          label={t("Dosya adı")}
          initial=""
          submitLabel={t("Dosyayı oluştur")}
          onClose={() => setDialog(null)}
          onSubmit={async (name) => {
            const target = joinPath(path, name);
            await fileOp(scope, { op: "write", path: target, content: "", create_new: true });
            await refresh();
            setDialog({ kind: "edit", path: target });
          }}
        />
      )}
      {dialog?.kind === "rename" && (
        <NameModal
          title={t("{name} öğesini yeniden adlandır", { name: dialog.entry.name })}
          label={t("Yeni ad")}
          initial={dialog.entry.name}
          submitLabel={t("Yeniden adlandır")}
          onClose={() => setDialog(null)}
          onSubmit={(name) =>
            fileOp(scope, { op: "rename", from: joinPath(path, dialog.entry.name), to: joinPath(path, name) }).then(() => {
              setSelected(new Set());
              return refresh();
            })
          }
        />
      )}
      {dialog?.kind === "compress" && (
        <CompressModal
          count={selectedPaths.length}
          suggested={single ? single.name : (crumbs[crumbs.length - 1] ?? "arsiv")}
          onClose={() => setDialog(null)}
          onSubmit={(name, format) =>
            fileOp(scope, { op: "compress", paths: selectedPaths, dest: joinPath(path, name), format }).then(() => {
              feedback.success(t("{name} oluşturuldu", { name }));
              setSelected(new Set());
              return refresh();
            })
          }
        />
      )}
      {dialog?.kind === "chmod" && (
        <PermissionsModal
          initial={selectedEntries[0]?.mode ?? 0o644}
          hasFolder={selectedEntries.some((entry) => entry.kind === "dir")}
          count={selectedEntries.length}
          onClose={() => setDialog(null)}
          onSubmit={(mode, recursive) =>
            fileOp(scope, { op: "chmod", paths: selectedPaths, mode, recursive }).then(() => {
              feedback.success(t("İzinler güncellendi"));
              return refresh();
            })
          }
        />
      )}
      {dialog?.kind === "edit" && <EditorModal scope={scope} path={dialog.path} onClose={() => (setDialog(null), void refresh())} />}
    </>
  );
}
