// Docker yönetimi: konteynerler, compose yığınları, imajlar, birimler ve ağlar.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Download,
  Eraser,
  Eye,
  EyeOff,
  Info,
  Pencil,
  Play,
  Plus,
  RotateCw,
  ScrollText,
  Square,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react";
import type { ReactNode } from "react";
import { Suspense, lazy, useEffect, useRef, useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
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
  PageHeader,
  Panel,
  SearchInput,
  Select,
  Spinner,
  Status,
  Table,
  Td,
  Textarea,
  Th,
  Tabs,
} from "../components/ui";
import type { Tone } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

const CodeEditor = lazy(() => import("../components/CodeEditor"));

// ---- Türler ----

interface DockerInfo {
  installed: boolean;
  running: boolean;
  version?: string;
  compose_version?: string | null;
  containers?: number;
  containers_running?: number;
  images?: number;
  storage_driver?: string;
  root_dir?: string;
}

interface PortBinding {
  container_port: number;
  protocol: string;
  host_ip: string;
  host_port: number;
}

type ContainerState = "running" | "exited" | "paused" | "created" | "restarting" | "dead";
type ContainerAction = "start" | "stop" | "restart" | "pause" | "unpause" | "kill";

interface Container {
  id: string;
  name: string;
  image: string;
  state: ContainerState;
  status_text: string;
  created: string;
  started_at: string;
  restart_policy: string;
  stack: string | null;
  ports: PortBinding[];
  mounts: string[];
}

interface ContainerStats {
  id: string;
  cpu: string;
  memory: string;
  memory_percent: string;
  network: string;
  block: string;
}

interface ContainerDetail {
  container: Container;
  command: string[] | null;
  entrypoint: string[] | null;
  env: string[] | null;
  networks: string[] | null;
  exit_code: number | null;
  error: string | null;
}

interface DockerImage {
  id: string;
  repository: string;
  tag: string;
  size: string;
  created: string;
}

interface DockerVolume {
  name: string;
  driver: string;
  mountpoint: string;
}

interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
}

interface Stack {
  name: string;
  status: string;
}

interface JobStarted {
  job_id: number;
}

interface CreateContainerRequest {
  image: string;
  name?: string;
  ports?: { host_port: number; container_port: number; udp?: boolean; public?: boolean }[];
  env?: { key: string; value: string }[];
  volumes?: { source: string; target: string; read_only?: boolean }[];
  restart?: "no" | "always" | "unless-stopped" | "on-failure";
  memory_mb?: number;
  command?: string[];
}

type JobHandler = (jobId: number, title: string) => void;

const TAB_IDS = ["containers", "stacks", "images", "volumes", "networks"] as const;
type TabId = (typeof TAB_IDS)[number];

const BUILTIN_NETWORKS = ["bridge", "host", "none"];

const LINK_BUTTON_CLASS =
  "inline-flex h-9 items-center justify-center rounded-md border border-transparent bg-accent px-3.5 font-medium text-accent-ink transition-colors hover:bg-accent-hover";

const ROW_INPUT_CLASS =
  "h-8 min-w-0 rounded-md border border-line-strong bg-surface px-2 text-ink placeholder:text-ink-3 " +
  "transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30";

const EXAMPLE_COMPOSE = `# Örnek compose dosyası: tek bir web sunucusu.
# Portun başındaki 127.0.0.1, portu yalnızca bu sunucuya bağlar;
# dışarıdan erişim için bir sitenin ters vekilini kullanın.
services:
  web:
    image: nginx:1.27
    restart: unless-stopped
    ports:
      - "127.0.0.1:8080:80"
    volumes:
      # Kalıcı veri için adlandırılmış birim
      - web_data:/usr/share/nginx/html

volumes:
  web_data: {}
`;

// ---- Yardımcılar ----

/** Docker uçlarına yapılan değişikliklerde ortak davranış: başarıda önbelleği yeniler, hatada bildirim gösterir. */
function useDockerMutation<V, R>(mutationFn: (variables: V) => Promise<R>, onSuccess?: (result: R, variables: V) => void) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (result, variables) => {
      void queryClient.invalidateQueries({ queryKey: ["docker"] });
      onSuccess?.(result, variables);
    },
    onError: (error) => feedback.error(error),
  });
}

function shortId(id: string): string {
  return id.replace(/^sha256:/, "").slice(0, 12);
}

/** ISO zaman damgasını Unix saniyesine çevirir; geçersiz ya da "hiç başlamadı" değerleri için 0 döner. */
function isoToUnix(iso: string): number {
  const time = new Date(iso).getTime();
  if (!Number.isFinite(time) || new Date(time).getUTCFullYear() < 1990) return 0;
  return Math.floor(time / 1000);
}

function stateInfo(container: Container): { tone: Tone; label: string } {
  switch (container.state) {
    case "running":
      return { tone: "ok", label: t("Çalışıyor") };
    case "paused":
      return { tone: "warn", label: t("Duraklatıldı") };
    case "restarting":
      return { tone: "warn", label: t("Yeniden başlıyor") };
    case "created":
      return { tone: "idle", label: t("Oluşturuldu") };
    case "dead":
      return { tone: "bad", label: t("Ölü") };
    case "exited": {
      const code = /Exited \((-?\d+)\)/.exec(container.status_text)?.[1];
      // Sıfırdan farklı çıkış kodu, konteynerin hatayla kapandığını gösterir.
      return code !== undefined && code !== "0" ? { tone: "bad", label: t("Hatayla durdu") } : { tone: "idle", label: t("Durdu") };
    }
  }
}

function findStats(list: ContainerStats[] | undefined, id: string): ContainerStats | undefined {
  return list?.find((item) => item.id === id || id.startsWith(item.id) || item.id.startsWith(id));
}

/** Aynı portun IPv4/IPv6 kopyalarını teke indirir; yayımlanmamış (ana makine portu olmayan) portları atar. */
function publishedPorts(ports: PortBinding[]): PortBinding[] {
  const seen = new Set<string>();
  const result: PortBinding[] = [];
  for (const port of ports) {
    if (!port.host_port) continue;
    const key = `${port.host_port}/${port.protocol}/${port.container_port}`;
    if (seen.has(key)) continue;
    seen.add(key);
    result.push(port);
  }
  return result;
}

const LOG_ERROR_PATTERN = /\b(error|crit|critical|alert|emerg|fatal|failed|panic)\b/i;
const LOG_WARNING_PATTERN = /\b(warn|warning|notice)\b/i;
const LOG_TIMESTAMP_PATTERN = /^\d{4}-\d{2}-\d{2}T/;

function logTone(line: string): string {
  if (LOG_ERROR_PATTERN.test(line)) return "text-bad";
  if (LOG_WARNING_PATTERN.test(line)) return "text-warn";
  return "";
}

function updateRow<T extends { key: number }>(rows: T[], key: number, patch: Partial<T>): T[] {
  return rows.map((row) => (row.key === key ? { ...row, ...patch } : row));
}

// ---- Sayfa ----

interface JobRef {
  jobId: number;
  title: string;
}

export function DockerPage() {
  const queryClient = useQueryClient();
  const [searchParams, setSearchParams] = useSearchParams();
  const [job, setJob] = useState<JobRef | null>(null);

  const info = useQuery({
    queryKey: ["docker", "info"],
    queryFn: () => api.get<DockerInfo>("/docker"),
    refetchInterval: 10000,
  });

  const requested = searchParams.get("sekme");
  const tab: TabId = TAB_IDS.find((id) => id === requested) ?? "containers";

  const tabs: { id: TabId; label: string }[] = [
    { id: "containers", label: t("Konteynerler") },
    { id: "stacks", label: t("Yığınlar") },
    { id: "images", label: t("İmajlar") },
    { id: "volumes", label: t("Birimler") },
    { id: "networks", label: t("Ağlar") },
  ];

  function selectTab(next: TabId) {
    const params = new URLSearchParams(searchParams);
    params.set("sekme", next);
    setSearchParams(params, { replace: true });
  }

  function closeJob() {
    setJob(null);
    void queryClient.invalidateQueries({ queryKey: ["docker"] });
    void queryClient.invalidateQueries({ queryKey: ["jobs"] });
  }

  const handleJob: JobHandler = (jobId, title) => {
    setJob({ jobId, title });
    void queryClient.invalidateQueries({ queryKey: ["jobs"] });
  };

  let description: ReactNode = t("Konteynerleri, compose yığınlarını, imajları, birimleri ve ağları yönetin.");
  const data = info.data;
  if (data?.installed && data.running) {
    description = t("Sürüm {version} · Compose {compose} · {running}/{total} konteyner çalışıyor", {
      version: data.version ?? "—",
      compose: data.compose_version ?? t("yok"),
      running: data.containers_running ?? 0,
      total: data.containers ?? 0,
    });
  }

  let body: ReactNode;
  if (info.isPending) {
    body = <LoadingBlock />;
  } else if (info.isError) {
    body = <ErrorBlock message={errorMessage(info.error)} onRetry={() => void info.refetch()} />;
  } else if (!info.data.installed) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Docker kurulu değil")}
          description={t("Konteynerleri yönetebilmek için önce Docker'ı Bileşenler sayfasından kurun.")}
          action={
            <Link to="/components" className={LINK_BUTTON_CLASS}>
              {t("Bileşenlere git")}
            </Link>
          }
        />
      </Panel>
    );
  } else if (!info.data.running) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Docker servisi durmuş")}
          description={t("Docker kurulu ama servisi çalışmıyor. Servisler sayfasından docker servisini başlatın.")}
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
        <Tabs tabs={tabs} value={tab} onChange={selectTab} label={t("Docker bölümleri")} />
        {tab === "containers" && <ContainersTab onJob={handleJob} />}
        {tab === "stacks" && <StacksTab onJob={handleJob} />}
        {tab === "images" && <ImagesTab onJob={handleJob} />}
        {tab === "volumes" && <VolumesTab />}
        {tab === "networks" && <NetworksTab />}
      </>
    );
  }

  return (
    <>
      <PageHeader title="Docker" description={description} />
      {body}
      <Modal open={job !== null} onClose={closeJob} title={job?.title ?? ""} wide>
        {job && <JobLog jobId={job.jobId} onDone={() => void queryClient.invalidateQueries({ queryKey: ["docker"] })} />}
      </Modal>
    </>
  );
}

// ---- Ortak küçük bileşenler ----

function Toolbar({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-center gap-3 border-b border-line px-3 py-2">{children}</div>;
}

function ActionsTh() {
  return (
    <Th align="right">
      <span className="sr-only">{t("İşlemler")}</span>
    </Th>
  );
}

/** Yalnızca ad (ya da tek metin) isteyen küçük pencere: birim, ağ ve imaj indirme için. */
function TextPromptModal({
  title,
  description,
  label,
  hint,
  placeholder,
  submitLabel,
  validate,
  loading,
  onSubmit,
  onClose,
}: {
  title: string;
  description?: string;
  label: string;
  hint?: string;
  placeholder?: string;
  submitLabel: string;
  validate: (value: string) => string | null;
  loading: boolean;
  onSubmit: (value: string) => void;
  onClose: () => void;
}) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  function submit() {
    const trimmed = value.trim();
    const problem = validate(trimmed);
    setError(problem);
    if (!problem) onSubmit(trimmed);
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      description={description}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={loading} onClick={submit}>
            {submitLabel}
          </Button>
        </>
      }
    >
      <Input
        label={label}
        hint={hint}
        error={error ?? undefined}
        placeholder={placeholder}
        value={value}
        autoFocus
        autoComplete="off"
        spellCheck={false}
        onChange={(event) => setValue(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") submit();
        }}
      />
    </Modal>
  );
}

// ---- Konteynerler ----

function ContainersTab({ onJob }: { onJob: JobHandler }) {
  const feedback = useFeedback();
  const navigate = useNavigate();
  const [filter, setFilter] = useState("");
  const [logsId, setLogsId] = useState<string | null>(null);
  const [detailsId, setDetailsId] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);

  const containers = useQuery({
    queryKey: ["docker", "containers"],
    queryFn: () => api.get<Container[]>("/docker/containers"),
    refetchInterval: 5000,
  });
  const anyRunning = containers.data?.some((container) => container.state === "running") ?? false;
  const stats = useQuery({
    queryKey: ["docker", "stats"],
    queryFn: () => api.get<ContainerStats[]>("/docker/containers/stats"),
    enabled: anyRunning,
    refetchInterval: 10000,
  });

  const action = useDockerMutation((variables: { id: string; action: ContainerAction }) =>
    api.post<{ ok: boolean }>(`/docker/containers/${encodeURIComponent(variables.id)}/${variables.action}`),
  );
  const remove = useDockerMutation((variables: { id: string; volumes: boolean }) =>
    api.delete<{ ok: boolean }>(`/docker/containers/${encodeURIComponent(variables.id)}?volumes=${variables.volumes}`),
  );

  async function removeContainer(container: Container, withVolumes: boolean): Promise<boolean> {
    const confirmed = await feedback.confirm({
      title: t("“{name}” konteyneri silinsin mi?", { name: container.name }),
      description: withVolumes
        ? t("Konteyner zorla durdurulup silinir; ona bağlı anonim birimler de silinir. Bu işlem geri alınamaz.")
        : t("Konteyner çalışıyorsa zorla durdurulur. Adlandırılmış birimlerdeki veriler korunur."),
      confirmLabel: withVolumes ? t("Birimleriyle birlikte sil") : t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate({ id: container.id, volumes: withVolumes });
    return confirmed;
  }

  async function killContainer(container: Container) {
    const confirmed = await feedback.confirm({
      title: t("“{name}” zorla durdurulsun mu?", { name: container.name }),
      description: t("Konteynere SIGKILL gönderilir; uygulama verilerini kaydetmeye fırsat bulamaz."),
      confirmLabel: t("Zorla durdur"),
      danger: true,
    });
    if (confirmed) action.mutate({ id: container.id, action: "kill" });
  }

  const needle = filter.trim().toLowerCase();
  const all = containers.data ?? [];
  const visible = needle
    ? all.filter((container) => [container.name, container.image, container.stack ?? ""].some((text) => text.toLowerCase().includes(needle)))
    : all;

  const logsContainer = all.find((container) => container.id === logsId) ?? null;
  const detailsContainer = all.find((container) => container.id === detailsId) ?? null;

  return (
    <>
      <Panel flush>
        <Toolbar>
          <SearchInput
            placeholder={t("Konteyner ara")}
            aria-label={t("Konteyner ara")}
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
          <div className="ml-auto">
            <Button variant="primary" size="sm" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
              {t("Konteyner oluştur")}
            </Button>
          </div>
        </Toolbar>
        {containers.isPending && <LoadingBlock />}
        {containers.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(containers.error)} onRetry={() => void containers.refetch()} />
          </div>
        )}
        {containers.data && all.length === 0 && (
          <EmptyState
            title={t("Henüz konteyner yok")}
            description={t("“Konteyner oluştur” ile bir imajdan konteyner başlatın ya da Yığınlar sekmesinden bir compose dosyası ekleyin.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
                {t("Konteyner oluştur")}
              </Button>
            }
          />
        )}
        {containers.data && all.length > 0 && visible.length === 0 && <EmptyState title={t("Aramanıza uyan konteyner yok")} />}
        {visible.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("Ad")}</Th>
                <Th>{t("İmaj")}</Th>
                <Th>{t("Durum")}</Th>
                <Th>{t("Portlar")}</Th>
                <Th>{t("CPU")}</Th>
                <Th>{t("Bellek")}</Th>
                <ActionsTh />
              </tr>
            </thead>
            <tbody>
              {visible.map((container) => (
                <ContainerRow
                  key={container.id}
                  container={container}
                  stats={findStats(stats.data, container.id)}
                  busy={(action.isPending && action.variables?.id === container.id) || (remove.isPending && remove.variables?.id === container.id)}
                  onAction={(next) => action.mutate({ id: container.id, action: next })}
                  onLogs={() => setLogsId(container.id)}
                  onTerminal={() => void navigate(`/terminal?kapsam=${encodeURIComponent("container:" + container.name)}`)}
                  onDetails={() => setDetailsId(container.id)}
                  onRemove={() => void removeContainer(container, false)}
                />
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      <Modal open={logsContainer !== null} onClose={() => setLogsId(null)} title={t("Günlükler: {name}", { name: logsContainer?.name ?? "" })} wide>
        {logsContainer && <ContainerLogs containerId={logsContainer.id} />}
      </Modal>

      <Modal
        open={detailsContainer !== null}
        onClose={() => setDetailsId(null)}
        title={detailsContainer?.name ?? ""}
        description={detailsContainer?.image}
        wide
        footer={
          detailsContainer && (
            <>
              {(detailsContainer.state === "running" || detailsContainer.state === "paused") && (
                <Button
                  size="sm"
                  onClick={() => action.mutate({ id: detailsContainer.id, action: detailsContainer.state === "paused" ? "unpause" : "pause" })}
                >
                  {detailsContainer.state === "paused" ? t("Devam ettir") : t("Duraklat")}
                </Button>
              )}
              {detailsContainer.state !== "exited" && detailsContainer.state !== "created" && (
                <Button size="sm" variant="danger" onClick={() => void killContainer(detailsContainer)}>
                  {t("Zorla durdur")}
                </Button>
              )}
              <Button
                size="sm"
                variant="danger"
                onClick={() =>
                  void removeContainer(detailsContainer, true).then((done) => {
                    if (done) setDetailsId(null);
                  })
                }
              >
                {t("Birimleriyle birlikte sil")}
              </Button>
              <Button
                size="sm"
                variant="danger"
                onClick={() =>
                  void removeContainer(detailsContainer, false).then((done) => {
                    if (done) setDetailsId(null);
                  })
                }
              >
                {t("Sil")}
              </Button>
            </>
          )
        }
      >
        {detailsContainer && <ContainerDetails container={detailsContainer} />}
      </Modal>

      {creating && (
        <CreateContainerModal
          onClose={() => setCreating(false)}
          onJob={(jobId, title) => {
            setCreating(false);
            onJob(jobId, title);
          }}
        />
      )}
    </>
  );
}

function ContainerRow({
  container,
  stats,
  busy,
  onAction,
  onLogs,
  onTerminal,
  onDetails,
  onRemove,
}: {
  container: Container;
  stats: ContainerStats | undefined;
  busy: boolean;
  onAction: (action: ContainerAction) => void;
  onLogs: () => void;
  onTerminal: () => void;
  onDetails: () => void;
  onRemove: () => void;
}) {
  const state = stateInfo(container);
  const ports = publishedPorts(container.ports);
  const isRunning = container.state === "running";
  const isActive = isRunning || container.state === "paused" || container.state === "restarting";

  return (
    <tr>
      <Td>
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span className="font-medium">{container.name}</span>
          {container.stack && <Badge tone="accent">{container.stack}</Badge>}
        </div>
      </Td>
      <Td className="max-w-56">
        <Mono className="block truncate text-ink-2">
          <span title={container.image}>{container.image}</span>
        </Mono>
      </Td>
      <Td>
        <Status tone={state.tone}>{state.label}</Status>
        <p className="mt-0.5 text-[12.5px] text-ink-3">{container.status_text}</p>
      </Td>
      <Td>
        {ports.length === 0 ? (
          <span className="text-ink-3">—</span>
        ) : (
          <ul className="flex flex-col gap-1">
            {ports.map((port) => (
              <li key={`${port.host_port}/${port.protocol}/${port.container_port}`} className="flex flex-wrap items-center gap-x-2">
                <span title={port.host_ip}>
                  <Mono>
                    {port.host_port} → {port.container_port}
                    {port.protocol !== "tcp" && `/${port.protocol}`}
                  </Mono>
                </span>
                {(port.host_ip === "127.0.0.1" || port.host_ip === "::1") && <Badge>{t("yerel")}</Badge>}
                {port.protocol === "tcp" && (
                  <Link
                    to={`/sites?yeni=vekil&hedef=${encodeURIComponent("http://127.0.0.1:" + port.host_port)}`}
                    className="text-[13px] text-accent hover:underline"
                  >
                    {t("Alan adına bağla")}
                  </Link>
                )}
              </li>
            ))}
          </ul>
        )}
      </Td>
      <Td>{isRunning && stats ? <Mono>{stats.cpu}</Mono> : <span className="text-ink-3">—</span>}</Td>
      <Td>{isRunning && stats ? <Mono>{stats.memory}</Mono> : <span className="text-ink-3">—</span>}</Td>
      <Td align="right">
        <div className="flex justify-end gap-0.5">
          {isActive ? (
            <IconButton label={t("Durdur")} disabled={busy} onClick={() => onAction("stop")}>
              <Square className="size-4" />
            </IconButton>
          ) : (
            <IconButton label={t("Başlat")} disabled={busy} onClick={() => onAction("start")}>
              <Play className="size-4" />
            </IconButton>
          )}
          <IconButton label={t("Yeniden başlat")} disabled={busy || !isActive} onClick={() => onAction("restart")}>
            <RotateCw className="size-4" />
          </IconButton>
          <IconButton label={t("Günlükler")} onClick={onLogs}>
            <ScrollText className="size-4" />
          </IconButton>
          <IconButton label={t("Terminal")} disabled={!isRunning} onClick={onTerminal}>
            <SquareTerminal className="size-4" />
          </IconButton>
          <IconButton label={t("Ayrıntılar")} onClick={onDetails}>
            <Info className="size-4" />
          </IconButton>
          <IconButton label={t("Sil")} disabled={busy} onClick={onRemove} className="hover:text-bad">
            <Trash2 className="size-4" />
          </IconButton>
        </div>
      </Td>
    </tr>
  );
}

function ContainerLogs({ containerId }: { containerId: string }) {
  const [tail, setTail] = useState(300);
  const [live, setLive] = useState(true);
  const box = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  const logs = useQuery({
    queryKey: ["docker", "logs", containerId, tail],
    queryFn: () => api.get<{ lines: string[] }>(`/docker/containers/${encodeURIComponent(containerId)}/logs?tail=${tail}`),
    refetchInterval: live ? 3000 : false,
  });

  // Kullanıcı yukarı kaydırmadıysa yeni satırlar geldikçe en alta inilir.
  useEffect(() => {
    const element = box.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [logs.data]);

  const lines = logs.data?.lines ?? [];

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-3">
        <select
          aria-label={t("Satır sayısı")}
          value={tail}
          onChange={(event) => {
            setTail(Number(event.target.value));
            pinned.current = true;
          }}
          className="h-8 rounded-md border border-line-strong bg-surface px-2"
        >
          {[100, 300, 1000].map((value) => (
            <option key={value} value={value}>
              {t("Son {n} satır", { n: value })}
            </option>
          ))}
        </select>
        <Checkbox label={t("Canlı izle")} checked={live} onChange={(event) => setLive(event.target.checked)} />
        {logs.isFetching && <Spinner className="size-3.5 text-ink-3" />}
      </div>
      <div
        ref={box}
        tabIndex={0}
        role="log"
        aria-label={t("Konteyner günlük satırları")}
        onScroll={(event) => {
          const element = event.currentTarget;
          pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 32;
        }}
        className="h-[50vh] overflow-auto rounded-md border border-line bg-sunken/50 px-3 py-2 font-mono text-[13px] leading-relaxed"
      >
        {logs.isError && <ErrorBlock message={errorMessage(logs.error)} />}
        {logs.data && lines.length === 0 && <p className="font-sans text-ink-2">{t("Konteyner henüz günlük üretmedi.")}</p>}
        {lines.map((line, index) => {
          const split = LOG_TIMESTAMP_PATTERN.test(line) ? line.indexOf(" ") : -1;
          const stamp = split > 0 ? line.slice(0, split).slice(0, 19).replace("T", " ") : null;
          const text = split > 0 ? line.slice(split + 1) : line;
          return (
            <div key={index} className={classNames("break-all whitespace-pre-wrap", logTone(text))}>
              {stamp && <span className="mr-2 text-ink-3">{stamp}</span>}
              {text}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function DetailRow({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-[13.5px] text-ink-2">{label}</dt>
      <dd className="min-w-0 break-words">{children}</dd>
    </>
  );
}

function MonoList({ items }: { items: string[] | null | undefined }) {
  if (!items || items.length === 0) return <span className="text-ink-3">—</span>;
  return (
    <ul className="flex flex-col gap-0.5">
      {items.map((item, index) => (
        <li key={`${item}-${index}`}>
          <Mono className="break-all">{item}</Mono>
        </li>
      ))}
    </ul>
  );
}

function ContainerDetails({ container }: { container: Container }) {
  const [reveal, setReveal] = useState(false);
  const detail = useQuery({
    queryKey: ["docker", "container", container.id],
    queryFn: () => api.get<ContainerDetail>(`/docker/containers/${encodeURIComponent(container.id)}`),
  });

  if (detail.isPending) return <LoadingBlock />;
  if (detail.isError) return <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />;

  const info = detail.data;
  const state = stateInfo(container);
  const env = (info.env ?? []).map((entry) => {
    const separator = entry.indexOf("=");
    return separator < 0 ? { key: entry, value: "" } : { key: entry.slice(0, separator), value: entry.slice(separator + 1) };
  });

  return (
    <dl className="grid grid-cols-[8.5rem_minmax(0,1fr)] gap-x-4 gap-y-2.5">
      <DetailRow label={t("Kimlik")}>
        <Mono>{container.id.slice(0, 12)}</Mono>
      </DetailRow>
      <DetailRow label={t("İmaj")}>
        <Mono className="break-all">{container.image}</Mono>
      </DetailRow>
      <DetailRow label={t("Durum")}>
        <Status tone={state.tone}>{state.label}</Status>
        <span className="ml-2 text-[13.5px] text-ink-3">{container.status_text}</span>
      </DetailRow>
      {info.error && (
        <DetailRow label={t("Hata")}>
          <span className="text-bad">{info.error}</span>
        </DetailRow>
      )}
      {info.exit_code !== null && container.state === "exited" && (
        <DetailRow label={t("Çıkış kodu")}>
          <Mono>{info.exit_code}</Mono>
        </DetailRow>
      )}
      <DetailRow label={t("Oluşturulma")}>{formatDateTime(isoToUnix(container.created))}</DetailRow>
      <DetailRow label={t("Son başlatma")}>{formatDateTime(isoToUnix(container.started_at))}</DetailRow>
      <DetailRow label={t("Yeniden başlatma")}>
        <Mono>{container.restart_policy || "no"}</Mono>
      </DetailRow>
      <DetailRow label={t("Giriş noktası")}>
        <MonoList items={info.entrypoint} />
      </DetailRow>
      <DetailRow label={t("Komut")}>
        <MonoList items={info.command} />
      </DetailRow>
      <DetailRow label={t("Ağlar")}>
        <MonoList items={info.networks} />
      </DetailRow>
      <DetailRow label={t("Bağlantılar")}>
        <MonoList items={container.mounts} />
      </DetailRow>
      <DetailRow label={t("Ortam değişkenleri")}>
        {env.length === 0 ? (
          <span className="text-ink-3">—</span>
        ) : (
          <div className="flex flex-col gap-1.5">
            <div>
              <Button
                size="sm"
                icon={reveal ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
                onClick={() => setReveal((value) => !value)}
              >
                {reveal ? t("Gizle") : t("Göster")}
              </Button>
              <span className="ml-2 text-[13px] text-ink-3">{t("Değerler parola gibi gizli bilgiler içerebilir.")}</span>
            </div>
            <ul className="flex flex-col gap-0.5">
              {env.map((entry, index) => (
                <li key={`${entry.key}-${index}`}>
                  <Mono className="break-all">
                    <span className="text-ink-2">{entry.key}</span>={reveal ? entry.value : "••••••"}
                  </Mono>
                </li>
              ))}
            </ul>
          </div>
        )}
      </DetailRow>
    </dl>
  );
}

// ---- Konteyner oluşturma ----

interface PortRow {
  key: number;
  host: string;
  container: string;
  udp: boolean;
  open: boolean;
}

interface EnvRow {
  key: number;
  name: string;
  value: string;
}

interface VolumeRow {
  key: number;
  source: string;
  target: string;
  readOnly: boolean;
}

function FormSection({ title, hint, addLabel, onAdd, children }: { title: string; hint?: string; addLabel: string; onAdd: () => void; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-3">
        <h3 className="text-[13.5px] font-medium text-ink-2">{title}</h3>
        <Button size="sm" icon={<Plus className="size-3.5" />} onClick={onAdd}>
          {addLabel}
        </Button>
      </div>
      {hint && <p className="text-[13px] text-ink-3">{hint}</p>}
      {children}
    </section>
  );
}

function parsePort(text: string): number | null {
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= 1 && value <= 65535 ? value : null;
}

function CreateContainerModal({ onClose, onJob }: { onClose: () => void; onJob: JobHandler }) {
  const keyCounter = useRef(0);
  const nextKey = () => ++keyCounter.current;

  const [image, setImage] = useState("");
  const [name, setName] = useState("");
  const [restart, setRestart] = useState<NonNullable<CreateContainerRequest["restart"]>>("unless-stopped");
  const [memory, setMemory] = useState("");
  const [ports, setPorts] = useState<PortRow[]>([]);
  const [envRows, setEnvRows] = useState<EnvRow[]>([]);
  const [volumes, setVolumes] = useState<VolumeRow[]>([]);
  const [commandText, setCommandText] = useState("");
  const [formError, setFormError] = useState<string | null>(null);

  const create = useDockerMutation(
    (body: CreateContainerRequest) => api.post<JobStarted>("/docker/containers", body),
    (result, body) => onJob(result.job_id, t("Konteyner oluşturuluyor: {image}", { image: body.image })),
  );

  /** Formu doğrular ve API gövdesini üretir; hata varsa ileti döner. */
  function build(): CreateContainerRequest | string {
    const trimmedImage = image.trim();
    if (!trimmedImage) return t("İmaj adı gerekli.");
    if (/\s/.test(trimmedImage)) return t("İmaj adı boşluk içeremez.");

    const body: CreateContainerRequest = { image: trimmedImage, restart };

    const trimmedName = name.trim();
    if (trimmedName) {
      if (!/^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(trimmedName)) return t("Konteyner adı harf ya da rakamla başlamalı; yalnızca harf, rakam, _ . - içerebilir.");
      body.name = trimmedName;
    }

    if (memory.trim()) {
      const mb = Number(memory);
      if (!Number.isInteger(mb) || mb < 1) return t("Bellek sınırı MB cinsinden pozitif bir tam sayı olmalı.");
      body.memory_mb = mb;
    }

    const portBody: NonNullable<CreateContainerRequest["ports"]> = [];
    for (const row of ports) {
      if (!row.host.trim() && !row.container.trim()) continue;
      const host = parsePort(row.host.trim());
      const container = parsePort(row.container.trim());
      if (host === null || container === null) return t("Port numaraları 1-65535 arasında olmalı.");
      portBody.push({ host_port: host, container_port: container, udp: row.udp, public: row.open });
    }
    if (portBody.length > 0) body.ports = portBody;

    const envBody: NonNullable<CreateContainerRequest["env"]> = [];
    for (const row of envRows) {
      const key = row.name.trim();
      if (!key && !row.value) continue;
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) return t("Ortam değişkeni adı harf ya da _ ile başlamalı; yalnızca harf, rakam ve _ içerebilir.");
      envBody.push({ key, value: row.value });
    }
    if (envBody.length > 0) body.env = envBody;

    const volumeBody: NonNullable<CreateContainerRequest["volumes"]> = [];
    for (const row of volumes) {
      const source = row.source.trim();
      const target = row.target.trim();
      if (!source && !target) continue;
      if (!source || !target) return t("Her birim satırında hem kaynak hem hedef yazılmalı.");
      if (!target.startsWith("/")) return t("Birim hedefi konteyner içinde mutlak bir yol olmalı (örn. /data).");
      volumeBody.push({ source, target, read_only: row.readOnly });
    }
    if (volumeBody.length > 0) body.volumes = volumeBody;

    const command = commandText
      .split("\n")
      .map((line) => line.trim())
      .filter((line) => line !== "");
    if (command.length > 0) body.command = command;

    return body;
  }

  function submit() {
    const result = build();
    if (typeof result === "string") {
      setFormError(result);
      return;
    }
    setFormError(null);
    create.mutate(result);
  }

  return (
    <Modal
      open
      extraWide
      onClose={onClose}
      title={t("Konteyner oluştur")}
      description={t("İmaj gerekirse indirilir, ardından konteyner başlatılır.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={create.isPending} onClick={submit}>
            {t("Oluştur ve başlat")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        {formError && <ErrorBlock message={formError} />}

        <div className="grid gap-4 sm:grid-cols-2">
          <Input
            label={t("İmaj")}
            placeholder="nginx:1.27"
            value={image}
            autoFocus
            spellCheck={false}
            onChange={(event) => setImage(event.target.value)}
          />
          <Input
            label={t("Ad (isteğe bağlı)")}
            placeholder="web"
            value={name}
            spellCheck={false}
            onChange={(event) => setName(event.target.value)}
          />
          <Select label={t("Yeniden başlatma ilkesi")} value={restart} onChange={(event) => setRestart(event.target.value as typeof restart)}>
            <option value="no">{t("Hiçbir zaman")}</option>
            <option value="unless-stopped">{t("Elle durdurulmadıkça")}</option>
            <option value="always">{t("Her zaman")}</option>
            <option value="on-failure">{t("Yalnızca hata olursa")}</option>
          </Select>
          <Input
            label={t("Bellek sınırı (MB, isteğe bağlı)")}
            inputMode="numeric"
            placeholder="512"
            value={memory}
            onChange={(event) => setMemory(event.target.value)}
          />
        </div>

        <FormSection
          title={t("Portlar")}
          hint={t(
            "“İnternete aç” işaretlenmezse port yalnızca 127.0.0.1 üzerinde dinler; bu porta ancak bir sitenin ters vekili üzerinden erişilir.",
          )}
          addLabel={t("Port ekle")}
          onAdd={() => setPorts((rows) => [...rows, { key: nextKey(), host: "", container: "", udp: false, open: false }])}
        >
          {ports.map((row) => (
            <div key={row.key} className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <input
                className={classNames(ROW_INPUT_CLASS, "w-36 font-mono text-[13px]")}
                placeholder={t("Ana makine portu")}
                aria-label={t("Ana makine portu")}
                inputMode="numeric"
                value={row.host}
                onChange={(event) => setPorts((rows) => updateRow(rows, row.key, { host: event.target.value }))}
              />
              <span aria-hidden className="text-ink-3">
                →
              </span>
              <input
                className={classNames(ROW_INPUT_CLASS, "w-36 font-mono text-[13px]")}
                placeholder={t("Konteyner portu")}
                aria-label={t("Konteyner portu")}
                inputMode="numeric"
                value={row.container}
                onChange={(event) => setPorts((rows) => updateRow(rows, row.key, { container: event.target.value }))}
              />
              <Checkbox label="UDP" checked={row.udp} onChange={(event) => setPorts((rows) => updateRow(rows, row.key, { udp: event.target.checked }))} />
              <Checkbox
                label={t("İnternete aç")}
                checked={row.open}
                onChange={(event) => setPorts((rows) => updateRow(rows, row.key, { open: event.target.checked }))}
              />
              <IconButton label={t("Satırı kaldır")} onClick={() => setPorts((rows) => rows.filter((item) => item.key !== row.key))}>
                <X className="size-4" />
              </IconButton>
            </div>
          ))}
        </FormSection>

        <FormSection
          title={t("Ortam değişkenleri")}
          addLabel={t("Değişken ekle")}
          onAdd={() => setEnvRows((rows) => [...rows, { key: nextKey(), name: "", value: "" }])}
        >
          {envRows.map((row) => (
            <div key={row.key} className="flex items-center gap-2">
              <input
                className={classNames(ROW_INPUT_CLASS, "w-2/5 font-mono text-[13px]")}
                placeholder="DEGISKEN_ADI"
                aria-label={t("Değişken adı")}
                spellCheck={false}
                value={row.name}
                onChange={(event) => setEnvRows((rows) => updateRow(rows, row.key, { name: event.target.value }))}
              />
              <input
                className={classNames(ROW_INPUT_CLASS, "flex-1 font-mono text-[13px]")}
                placeholder={t("Değer")}
                aria-label={t("Değer")}
                spellCheck={false}
                value={row.value}
                onChange={(event) => setEnvRows((rows) => updateRow(rows, row.key, { value: event.target.value }))}
              />
              <IconButton label={t("Satırı kaldır")} onClick={() => setEnvRows((rows) => rows.filter((item) => item.key !== row.key))}>
                <X className="size-4" />
              </IconButton>
            </div>
          ))}
        </FormSection>

        <FormSection
          title={t("Birimler")}
          hint={t("Kaynak, sunucudaki mutlak bir yol ya da bir birim adı olabilir.")}
          addLabel={t("Birim ekle")}
          onAdd={() => setVolumes((rows) => [...rows, { key: nextKey(), source: "", target: "", readOnly: false }])}
        >
          {volumes.map((row) => (
            <div key={row.key} className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <input
                className={classNames(ROW_INPUT_CLASS, "min-w-40 flex-1 font-mono text-[13px]")}
                placeholder="/srv/veri"
                aria-label={t("Kaynak")}
                spellCheck={false}
                value={row.source}
                onChange={(event) => setVolumes((rows) => updateRow(rows, row.key, { source: event.target.value }))}
              />
              <span aria-hidden className="text-ink-3">
                →
              </span>
              <input
                className={classNames(ROW_INPUT_CLASS, "min-w-40 flex-1 font-mono text-[13px]")}
                placeholder="/data"
                aria-label={t("Hedef")}
                spellCheck={false}
                value={row.target}
                onChange={(event) => setVolumes((rows) => updateRow(rows, row.key, { target: event.target.value }))}
              />
              <Checkbox
                label={t("Salt okunur")}
                checked={row.readOnly}
                onChange={(event) => setVolumes((rows) => updateRow(rows, row.key, { readOnly: event.target.checked }))}
              />
              <IconButton label={t("Satırı kaldır")} onClick={() => setVolumes((rows) => rows.filter((item) => item.key !== row.key))}>
                <X className="size-4" />
              </IconButton>
            </div>
          ))}
        </FormSection>

        <Textarea
          label={t("Komut (isteğe bağlı)")}
          hint={t("Her satır tek bir argümandır; boşluklarla bölünmez. Boş bırakırsanız imajın varsayılan komutu çalışır.")}
          rows={3}
          spellCheck={false}
          placeholder={"--port\n8080"}
          value={commandText}
          onChange={(event) => setCommandText(event.target.value)}
        />
      </div>
    </Modal>
  );
}

// ---- Yığınlar (compose) ----

function stackStatus(status: string): ReactNode {
  const text = status.trim();
  if (text === "") return <Status tone="idle">{t("Durdu")}</Status>;
  if (text.startsWith("running")) {
    const count = /\((\d+)\)/.exec(text)?.[1];
    return <Status tone="ok">{count ? t("Çalışıyor ({n})", { n: count }) : t("Çalışıyor")}</Status>;
  }
  return (
    <Status tone="warn">
      <Mono>{text}</Mono>
    </Status>
  );
}

type StackAction = "up" | "down" | "restart" | "update";

function StacksTab({ onJob }: { onJob: JobHandler }) {
  const feedback = useFeedback();
  const [editor, setEditor] = useState<{ mode: "create" } | { mode: "edit"; name: string } | null>(null);

  const stacks = useQuery({
    queryKey: ["docker", "stacks"],
    queryFn: () => api.get<Stack[]>("/docker/stacks"),
    refetchInterval: 8000,
  });

  const run = useDockerMutation(
    (variables: { name: string; action: StackAction }) =>
      api.post<JobStarted>(`/docker/stacks/${encodeURIComponent(variables.name)}/${variables.action}`),
    (result, variables) => {
      const titles: Record<StackAction, string> = {
        up: t("Yığın başlatılıyor: {name}", { name: variables.name }),
        down: t("Yığın durduruluyor: {name}", { name: variables.name }),
        restart: t("Yığın yeniden başlatılıyor: {name}", { name: variables.name }),
        update: t("Yığın güncelleniyor: {name}", { name: variables.name }),
      };
      onJob(result.job_id, titles[variables.action]);
    },
  );
  const remove = useDockerMutation((name: string) => api.delete<{ ok: boolean }>(`/docker/stacks/${encodeURIComponent(name)}`));

  async function stopStack(stack: Stack) {
    const confirmed = await feedback.confirm({
      title: t("“{name}” yığını durdurulsun mu?", { name: stack.name }),
      description: t("Yığının konteynerleri durdurulup kaldırılır; birimlerdeki veriler korunur."),
      confirmLabel: t("Durdur"),
      danger: true,
    });
    if (confirmed) run.mutate({ name: stack.name, action: "down" });
  }

  async function deleteStack(stack: Stack) {
    const confirmed = await feedback.confirm({
      title: t("“{name}” yığını silinsin mi?", { name: stack.name }),
      description: t("Yığının konteynerleri durdurulup kaldırılır ve compose dosyası silinir. Birimlerdeki veriler korunur."),
      confirmLabel: t("Yığını sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(stack.name);
  }

  const list = stacks.data ?? [];
  const busyName = run.isPending ? run.variables?.name : remove.isPending ? remove.variables : undefined;

  return (
    <>
      <Panel flush>
        <Toolbar>
          <p className="text-ink-2">{t("Compose dosyalarıyla tanımlanan çok konteynerli uygulamalar.")}</p>
          <div className="ml-auto">
            <Button variant="primary" size="sm" icon={<Plus className="size-3.5" />} onClick={() => setEditor({ mode: "create" })}>
              {t("Yığın oluştur")}
            </Button>
          </div>
        </Toolbar>
        {stacks.isPending && <LoadingBlock />}
        {stacks.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(stacks.error)} onRetry={() => void stacks.refetch()} />
          </div>
        )}
        {stacks.data && list.length === 0 && (
          <EmptyState
            title={t("Henüz yığın yok")}
            description={t("Birden fazla konteynerden oluşan bir uygulama için bir compose dosyası ekleyin; yığın başlatılıp durdurulurken tüm konteynerleri birlikte yönetilir.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setEditor({ mode: "create" })}>
                {t("Yığın oluştur")}
              </Button>
            }
          />
        )}
        {list.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("Ad")}</Th>
                <Th>{t("Durum")}</Th>
                <ActionsTh />
              </tr>
            </thead>
            <tbody>
              {list.map((stack) => {
                const busy = busyName === stack.name;
                return (
                  <tr key={stack.name}>
                    <Td>
                      <span className="font-medium">{stack.name}</span>
                    </Td>
                    <Td>{stackStatus(stack.status)}</Td>
                    <Td align="right">
                      <div className="flex justify-end gap-0.5">
                        <IconButton label={t("Başlat")} disabled={busy} onClick={() => run.mutate({ name: stack.name, action: "up" })}>
                          <Play className="size-4" />
                        </IconButton>
                        <IconButton label={t("Durdur")} disabled={busy} onClick={() => void stopStack(stack)}>
                          <Square className="size-4" />
                        </IconButton>
                        <IconButton label={t("Yeniden başlat")} disabled={busy} onClick={() => run.mutate({ name: stack.name, action: "restart" })}>
                          <RotateCw className="size-4" />
                        </IconButton>
                        <IconButton label={t("Güncelle (imajları indir ve yeniden başlat)")} disabled={busy} onClick={() => run.mutate({ name: stack.name, action: "update" })}>
                          <Download className="size-4" />
                        </IconButton>
                        <IconButton label={t("Düzenle")} onClick={() => setEditor({ mode: "edit", name: stack.name })}>
                          <Pencil className="size-4" />
                        </IconButton>
                        <IconButton label={t("Sil")} disabled={busy} className="hover:text-bad" onClick={() => void deleteStack(stack)}>
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
      </Panel>

      {editor && (
        <StackEditorModal
          key={editor.mode === "edit" ? editor.name : "new"}
          mode={editor.mode}
          stackName={editor.mode === "edit" ? editor.name : null}
          onClose={() => setEditor(null)}
          onJob={(jobId, title) => {
            setEditor(null);
            onJob(jobId, title);
          }}
        />
      )}
    </>
  );
}

function StackEditorModal({
  mode,
  stackName,
  onClose,
  onJob,
}: {
  mode: "create" | "edit";
  stackName: string | null;
  onClose: () => void;
  onJob: JobHandler;
}) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [edited, setEdited] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [nameError, setNameError] = useState<string | null>(null);
  // Oluşturma başarılı olduktan sonra başlatma adımı hata verirse yeniden denemede aynı yığın tekrar oluşturulmaz.
  const created = useRef(false);

  const loaded = useQuery({
    queryKey: ["docker", "stack", stackName],
    queryFn: () => api.get<{ name: string; content: string }>(`/docker/stacks/${encodeURIComponent(stackName ?? "")}`),
    enabled: mode === "edit" && stackName !== null,
  });

  const ready = mode === "create" || loaded.data !== undefined;
  const content = edited ?? loaded.data?.content ?? EXAMPLE_COMPOSE;
  const effectiveName = mode === "edit" ? (stackName ?? "") : name.trim();
  const dark = document.documentElement.dataset.theme === "dark";

  const save = useMutation({
    mutationFn: async (variables: { start: boolean }): Promise<JobStarted | null> => {
      if (mode === "create" && !created.current) {
        await api.post<{ ok: boolean }>("/docker/stacks", { name: effectiveName, content });
        created.current = true;
      } else {
        await api.put<{ ok: boolean }>(`/docker/stacks/${encodeURIComponent(effectiveName)}`, { content });
      }
      return variables.start ? api.post<JobStarted>(`/docker/stacks/${encodeURIComponent(effectiveName)}/up`) : null;
    },
    onSuccess: (job) => {
      void queryClient.invalidateQueries({ queryKey: ["docker"] });
      if (job) {
        onJob(job.job_id, t("Yığın başlatılıyor: {name}", { name: effectiveName }));
      } else {
        feedback.success(t("Yığın kaydedildi."));
        onClose();
      }
    },
    // Geçersiz YAML gibi hatalar pencerenin içinde, düzenleyicinin üstünde gösterilir.
    onError: (error) => setSaveError(errorMessage(error)),
  });

  function submit(start: boolean) {
    setSaveError(null);
    if (mode === "create" && !/^[a-z0-9][a-z0-9_-]*$/.test(effectiveName)) {
      setNameError(t("Ad küçük harf, rakam, - ve _ içerebilir; harf ya da rakamla başlamalıdır."));
      return;
    }
    setNameError(null);
    if (!content.trim()) {
      setSaveError(t("Compose içeriği boş olamaz."));
      return;
    }
    save.mutate({ start });
  }

  return (
    <Modal
      open
      extraWide
      onClose={onClose}
      title={mode === "create" ? t("Yığın oluştur") : t("Yığını düzenle: {name}", { name: stackName ?? "" })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button disabled={!ready} loading={save.isPending && save.variables?.start === false} onClick={() => submit(false)}>
            {t("Kaydet")}
          </Button>
          <Button variant="primary" disabled={!ready} loading={save.isPending && save.variables?.start === true} onClick={() => submit(true)}>
            {t("Kaydet ve başlat")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        {mode === "create" && (
          <Input
            label={t("Yığın adı")}
            hint={t("Küçük harf, rakam, - ve _ kullanılabilir.")}
            error={nameError ?? undefined}
            placeholder="web-uygulamasi"
            value={name}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setName(event.target.value)}
          />
        )}
        {loaded.isError && <ErrorBlock message={errorMessage(loaded.error)} onRetry={() => void loaded.refetch()} />}
        {mode === "edit" && loaded.isPending && <LoadingBlock />}
        {saveError && <ErrorBlock message={saveError} />}
        {ready && (
          <div className="flex flex-col gap-1.5">
            <span className="text-[13.5px] font-medium text-ink-2">{t("Compose dosyası (YAML)")}</span>
            <div className="h-80 overflow-hidden rounded-md border border-line-strong">
              <Suspense fallback={<LoadingBlock />}>
                <CodeEditor filename="compose.yaml" initialValue={content} dark={dark} onChange={setEdited} onSave={() => submit(false)} />
              </Suspense>
            </div>
            <p className="text-[13px] text-ink-3">{t("Ctrl+S ile kaydedebilirsiniz. Kaydetmek çalışan yığını etkilemez; değişiklikler “Kaydet ve başlat” ile uygulanır.")}</p>
          </div>
        )}
      </div>
    </Modal>
  );
}

// ---- İmajlar ----

function ImagesTab({ onJob }: { onJob: JobHandler }) {
  const feedback = useFeedback();
  const [pulling, setPulling] = useState(false);

  const images = useQuery({
    queryKey: ["docker", "images"],
    queryFn: () => api.get<DockerImage[]>("/docker/images"),
  });

  const pull = useDockerMutation(
    (image: string) => api.post<JobStarted>("/docker/images/pull", { image }),
    (result, image) => {
      setPulling(false);
      onJob(result.job_id, t("İmaj indiriliyor: {image}", { image }));
    },
  );
  const remove = useDockerMutation((id: string) => api.delete<{ ok: boolean }>(`/docker/images/${encodeURIComponent(id)}`));
  const prune = useDockerMutation(
    () => api.post<{ ok: boolean; summary: string }>("/docker/images/prune"),
    (result) => feedback.success(result.summary || t("Temizlenecek kullanılmayan imaj yoktu.")),
  );

  async function removeImage(image: DockerImage) {
    const label = image.repository === "<none>" ? shortId(image.id) : `${image.repository}:${image.tag}`;
    const confirmed = await feedback.confirm({
      title: t("“{name}” imajı silinsin mi?", { name: label }),
      description: t("Bu imajı kullanan bir konteyner varsa silme başarısız olur."),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(image.id);
  }

  async function pruneImages() {
    const confirmed = await feedback.confirm({
      title: t("Kullanılmayan imajlar temizlensin mi?"),
      description: t("Hiçbir konteynerin kullanmadığı ve etiketsiz imajlar silinir. Gerekirse yeniden indirilebilirler."),
      confirmLabel: t("Temizle"),
      danger: true,
    });
    if (confirmed) prune.mutate(undefined);
  }

  const list = images.data ?? [];

  return (
    <>
      <Panel flush>
        <Toolbar>
          <div className="ml-auto flex items-center gap-2">
            <Button size="sm" icon={<Eraser className="size-3.5" />} loading={prune.isPending} onClick={() => void pruneImages()}>
              {t("Kullanılmayanları temizle")}
            </Button>
            <Button variant="primary" size="sm" icon={<Download className="size-3.5" />} onClick={() => setPulling(true)}>
              {t("İmaj indir")}
            </Button>
          </div>
        </Toolbar>
        {images.isPending && <LoadingBlock />}
        {images.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(images.error)} onRetry={() => void images.refetch()} />
          </div>
        )}
        {images.data && list.length === 0 && (
          <EmptyState
            title={t("Henüz imaj yok")}
            description={t("“İmaj indir” ile bir kayıt defterinden imaj çekin (örn. nginx:1.27). Konteyner oluştururken de imaj otomatik indirilir.")}
            action={
              <Button variant="primary" icon={<Download className="size-4" />} onClick={() => setPulling(true)}>
                {t("İmaj indir")}
              </Button>
            }
          />
        )}
        {list.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("İmaj")}</Th>
                <Th>{t("Kimlik")}</Th>
                <Th>{t("Boyut")}</Th>
                <Th>{t("Oluşturulma")}</Th>
                <ActionsTh />
              </tr>
            </thead>
            <tbody>
              {list.map((image, index) => (
                <tr key={`${image.id}-${image.repository}-${image.tag}-${index}`}>
                  <Td>
                    <Mono>{image.repository === "<none>" ? t("(etiketsiz)") : `${image.repository}:${image.tag}`}</Mono>
                  </Td>
                  <Td>
                    <Mono className="text-ink-2">{shortId(image.id)}</Mono>
                  </Td>
                  <Td>{image.size}</Td>
                  <Td className="text-ink-2">{image.created}</Td>
                  <Td align="right">
                    <IconButton
                      label={t("Sil")}
                      className="hover:text-bad"
                      disabled={remove.isPending && remove.variables === image.id}
                      onClick={() => void removeImage(image)}
                    >
                      <Trash2 className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      {pulling && (
        <TextPromptModal
          title={t("İmaj indir")}
          label={t("İmaj adı")}
          hint={t("Etiketle birlikte yazın; etiket verilmezse latest indirilir.")}
          placeholder="nginx:1.27"
          submitLabel={t("İndir")}
          validate={(value) => (!value ? t("İmaj adı gerekli.") : /\s/.test(value) ? t("İmaj adı boşluk içeremez.") : null)}
          loading={pull.isPending}
          onSubmit={(value) => pull.mutate(value)}
          onClose={() => setPulling(false)}
        />
      )}
    </>
  );
}

// ---- Birimler ----

function VolumesTab() {
  const feedback = useFeedback();
  const [creating, setCreating] = useState(false);

  const volumes = useQuery({
    queryKey: ["docker", "volumes"],
    queryFn: () => api.get<DockerVolume[]>("/docker/volumes"),
  });

  const create = useDockerMutation(
    (name: string) => api.post<{ ok: boolean }>("/docker/volumes", { name }),
    () => setCreating(false),
  );
  const remove = useDockerMutation((name: string) => api.delete<{ ok: boolean }>(`/docker/volumes/${encodeURIComponent(name)}`));

  async function removeVolume(volume: DockerVolume) {
    const confirmed = await feedback.confirm({
      title: t("“{name}” birimi silinsin mi?", { name: volume.name }),
      description: t("Birimdeki tüm veri kalıcı olarak silinir. Bu işlem geri alınamaz."),
      confirmLabel: t("Birimi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(volume.name);
  }

  const list = volumes.data ?? [];

  return (
    <>
      <Panel flush>
        <Toolbar>
          <p className="text-ink-2">{t("Konteyner verilerinin kalıcı olarak saklandığı Docker birimleri.")}</p>
          <div className="ml-auto">
            <Button variant="primary" size="sm" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
              {t("Birim oluştur")}
            </Button>
          </div>
        </Toolbar>
        {volumes.isPending && <LoadingBlock />}
        {volumes.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(volumes.error)} onRetry={() => void volumes.refetch()} />
          </div>
        )}
        {volumes.data && list.length === 0 && (
          <EmptyState
            title={t("Henüz birim yok")}
            description={t("Veritabanı gibi verisi kalıcı olması gereken konteynerler için bir birim oluşturup konteynere bağlayın.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
                {t("Birim oluştur")}
              </Button>
            }
          />
        )}
        {list.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("Ad")}</Th>
                <Th>{t("Sürücü")}</Th>
                <Th>{t("Konum")}</Th>
                <ActionsTh />
              </tr>
            </thead>
            <tbody>
              {list.map((volume) => (
                <tr key={volume.name}>
                  <Td>
                    <Mono className="break-all">{volume.name}</Mono>
                  </Td>
                  <Td className="text-ink-2">{volume.driver}</Td>
                  <Td>
                    <Mono className="break-all text-ink-2">{volume.mountpoint}</Mono>
                  </Td>
                  <Td align="right">
                    <IconButton
                      label={t("Sil")}
                      className="hover:text-bad"
                      disabled={remove.isPending && remove.variables === volume.name}
                      onClick={() => void removeVolume(volume)}
                    >
                      <Trash2 className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>

      {creating && (
        <TextPromptModal
          title={t("Birim oluştur")}
          label={t("Birim adı")}
          hint={t("Harf, rakam, _ . - kullanılabilir.")}
          placeholder="veritabani_verisi"
          submitLabel={t("Oluştur")}
          validate={(value) =>
            /^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(value) ? null : t("Ad harf ya da rakamla başlamalı; yalnızca harf, rakam, _ . - içerebilir.")
          }
          loading={create.isPending}
          onSubmit={(value) => create.mutate(value)}
          onClose={() => setCreating(false)}
        />
      )}
    </>
  );
}

// ---- Ağlar ----

function NetworksTab() {
  const feedback = useFeedback();
  const [creating, setCreating] = useState(false);

  const networks = useQuery({
    queryKey: ["docker", "networks"],
    queryFn: () => api.get<DockerNetwork[]>("/docker/networks"),
  });

  const create = useDockerMutation(
    (name: string) => api.post<{ ok: boolean }>("/docker/networks", { name }),
    () => setCreating(false),
  );
  const remove = useDockerMutation((name: string) => api.delete<{ ok: boolean }>(`/docker/networks/${encodeURIComponent(name)}`));

  async function removeNetwork(network: DockerNetwork) {
    const confirmed = await feedback.confirm({
      title: t("“{name}” ağı silinsin mi?", { name: network.name }),
      description: t("Ağı kullanan bir konteyner varsa silme başarısız olur."),
      confirmLabel: t("Ağı sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(network.name);
  }

  const list = networks.data ?? [];

  return (
    <>
      <Panel flush>
        <Toolbar>
          <p className="text-ink-2">{t("Aynı ağdaki konteynerler birbirine adlarıyla erişebilir.")}</p>
          <div className="ml-auto">
            <Button variant="primary" size="sm" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
              {t("Ağ oluştur")}
            </Button>
          </div>
        </Toolbar>
        {networks.isPending && <LoadingBlock />}
        {networks.isError && (
          <div className="p-3">
            <ErrorBlock message={errorMessage(networks.error)} onRetry={() => void networks.refetch()} />
          </div>
        )}
        {networks.data && list.length === 0 && (
          <EmptyState
            title={t("Gösterilecek ağ yok")}
            description={t("Konteynerlerinizi yalıtmak için kendi ağınızı oluşturabilirsiniz.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
                {t("Ağ oluştur")}
              </Button>
            }
          />
        )}
        {list.length > 0 && (
          <Table>
            <thead>
              <tr>
                <Th>{t("Ad")}</Th>
                <Th>{t("Sürücü")}</Th>
                <Th>{t("Kapsam")}</Th>
                <Th>{t("Kimlik")}</Th>
                <ActionsTh />
              </tr>
            </thead>
            <tbody>
              {list.map((network) => {
                const builtin = BUILTIN_NETWORKS.includes(network.name);
                return (
                  <tr key={network.id}>
                    <Td>
                      <div className="flex items-center gap-2">
                        <Mono>{network.name}</Mono>
                        {builtin && <Badge>{t("yerleşik")}</Badge>}
                      </div>
                    </Td>
                    <Td className="text-ink-2">{network.driver}</Td>
                    <Td className="text-ink-2">{network.scope}</Td>
                    <Td>
                      <Mono className="text-ink-2">{network.id.slice(0, 12)}</Mono>
                    </Td>
                    <Td align="right">
                      {!builtin && (
                        <IconButton
                          label={t("Sil")}
                          className="hover:text-bad"
                          disabled={remove.isPending && remove.variables === network.name}
                          onClick={() => void removeNetwork(network)}
                        >
                          <Trash2 className="size-4" />
                        </IconButton>
                      )}
                    </Td>
                  </tr>
                );
              })}
            </tbody>
          </Table>
        )}
      </Panel>

      {creating && (
        <TextPromptModal
          title={t("Ağ oluştur")}
          label={t("Ağ adı")}
          hint={t("Harf, rakam, _ . - kullanılabilir.")}
          placeholder="uygulama_agi"
          submitLabel={t("Oluştur")}
          validate={(value) =>
            /^[a-zA-Z0-9][a-zA-Z0-9_.-]*$/.test(value) ? null : t("Ad harf ya da rakamla başlamalı; yalnızca harf, rakam, _ . - içerebilir.")
          }
          loading={create.isPending}
          onSubmit={(value) => create.mutate(value)}
          onClose={() => setCreating(false)}
        />
      )}
    </>
  );
}
