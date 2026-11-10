// "Yedekler" sekmesi: neler yedeklenmiş (konuya göre gruplu) ve geri yükleme.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, Database, Globe, Mail, Server, Trash2 } from "lucide-react";
import type { ReactNode } from "react";
import { useMemo, useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, LoadingBlock, Modal, Panel, SearchInput, Select, Spinner, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { PanelRestoreHelp, RestoreWizard } from "./BackupRestore";
import { ScopePicker } from "./BackupScope";
import type { BackupOverview, JobRef, JobStarted, Scope, Snapshot, SnapshotKind, Target } from "./BackupShared";
import {
  COMPACT_SELECT_CLASS,
  fullScope,
  isScopeEmpty,
  JobModal,
  kindLabel,
  Notice,
  OVERVIEW_KEY,
  Segmented,
  SNAPSHOTS_KEY,
  snapshotSubject,
} from "./BackupShared";

// ---- Gruplama ----

interface Group {
  key: string;
  kind: SnapshotKind;
  subject: string;
  owner: string;
  /** Yeniden eskiye sıralı. */
  snapshots: Snapshot[];
}

const KIND_ORDER: SnapshotKind[] = ["site", "db", "mail", "panel"];

/** Yedekleri "neyin yedeği" olduğuna göre gruplar: her (tür, ad) için bir satır. */
function groupSnapshots(snapshots: Snapshot[]): Group[] {
  const groups = new Map<string, Group>();
  for (const snapshot of [...snapshots].sort((a, b) => b.time - a.time)) {
    const key = `${snapshot.kind}|${snapshot.engine}|${snapshot.name}`;
    const existing = groups.get(key);
    if (existing) existing.snapshots.push(snapshot);
    else groups.set(key, { key, kind: snapshot.kind, subject: snapshotSubject(snapshot), owner: snapshot.owner, snapshots: [snapshot] });
  }
  return [...groups.values()].sort((a, b) => KIND_ORDER.indexOf(a.kind) - KIND_ORDER.indexOf(b.kind) || a.subject.localeCompare(b.subject, "tr"));
}

function KindIcon({ kind }: { kind: SnapshotKind }) {
  const className = "size-4 shrink-0 text-ink-3";
  switch (kind) {
    case "site":
      return <Globe aria-hidden className={className} />;
    case "db":
      return <Database aria-hidden className={className} />;
    case "mail":
      return <Mail aria-hidden className={className} />;
    case "panel":
      return <Server aria-hidden className={className} />;
  }
}

type FilterId = "all" | SnapshotKind;

// ---- "Şimdi yedek al" penceresi ----

function RunNowModal({ targets, sources, initialTargetId, onClose, onStarted }: { targets: Target[]; sources: BackupOverview["sources"]; initialTargetId: number; onClose: () => void; onStarted: (jobId: number) => void }) {
  const [targetId, setTargetId] = useState(initialTargetId);
  const [scope, setScope] = useState<Scope>(fullScope());

  const run = useMutation({
    mutationFn: () => api.post<JobStarted>("/backup/run", { target_id: targetId, scope }),
    onSuccess: (started) => onStarted(started.job_id),
  });

  const empty = isScopeEmpty(scope);

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={t("Şimdi yedek al")}
      description={t("Elle alınan yedekler kendiliğinden silinmez.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={run.isPending} disabled={empty} onClick={() => run.mutate()}>
            {t("Yedek al")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        {targets.length > 1 && (
          <div className="max-w-sm">
            <Select label={t("Nereye?")} value={targetId} onChange={(event) => setTargetId(Number(event.target.value))}>
              {targets.map((target) => (
                <option key={target.id} value={target.id}>
                  {target.name}
                </option>
              ))}
            </Select>
          </div>
        )}
        <section className="flex flex-col gap-2">
          <h3 className="font-semibold">{t("Neler yedeklensin?")}</h3>
          <ScopePicker value={scope} onChange={setScope} sources={sources} />
          {empty && <p className="text-[12.5px] text-ink-3">{t("Yedeklenecek en az bir şey seçin.")}</p>}
        </section>
        {run.isError && <Notice tone="bad">{errorMessage(run.error)}</Notice>}
      </div>
    </Modal>
  );
}

// ---- Satırlar ----

function SnapshotList({
  group,
  plans,
  deletingId,
  onRestore,
  onDelete,
  onHelp,
}: {
  group: Group;
  plans: Map<number, string>;
  deletingId: string | null;
  onRestore: (snapshot: Snapshot) => void;
  onDelete: (snapshot: Snapshot) => void;
  onHelp: () => void;
}) {
  return (
    <ul className="border-t border-line bg-sunken/40">
      {group.snapshots.map((snapshot) => {
        const source = snapshot.plan_id === null ? t("Elle alındı") : t("Plan: {name}", { name: plans.get(snapshot.plan_id) ?? t("silinmiş plan") });
        return (
          <li key={snapshot.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 border-b border-line py-2 pr-4 pl-11 last:border-b-0">
            <span className="w-36 shrink-0 tabular-nums">{formatDateTime(snapshot.time)}</span>
            <span className="min-w-0 flex-1 text-ink-2">{source}</span>
            <span className="w-20 shrink-0 text-right text-ink-2 tabular-nums">{snapshot.size === null ? "—" : formatBytes(snapshot.size)}</span>
            <div className="flex items-center gap-1">
              {snapshot.kind === "panel" ? (
                <Button size="sm" variant="ghost" onClick={onHelp}>
                  {t("Nasıl geri yüklenir?")}
                </Button>
              ) : (
                <Button size="sm" variant="primary" onClick={() => onRestore(snapshot)}>
                  {t("Geri yükle")}
                </Button>
              )}
              <IconButton label={t("Bu yedeği sil")} disabled={deletingId === snapshot.id} onClick={() => onDelete(snapshot)}>
                {deletingId === snapshot.id ? <Spinner /> : <Trash2 className="size-4" />}
              </IconButton>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

function GroupTable({
  groups,
  plans,
  deletingId,
  onRestore,
  onDelete,
  onHelp,
}: {
  groups: Group[];
  plans: Map<number, string>;
  deletingId: string | null;
  onRestore: (snapshot: Snapshot) => void;
  onDelete: (snapshot: Snapshot) => void;
  onHelp: () => void;
}) {
  const [open, setOpen] = useState<Set<string>>(new Set());

  function toggle(key: string) {
    setOpen((current) => {
      const next = new Set(current);
      if (!next.delete(key)) next.add(key);
      return next;
    });
  }

  return (
    <Table>
      <thead>
        <tr>
          <Th>{t("Ne yedeklendi")}</Th>
          <Th>{t("Son yedek")}</Th>
          <Th align="right">{t("Yedek sayısı")}</Th>
          <Th align="right">{t("Son yedeğin boyutu")}</Th>
        </tr>
      </thead>
      <tbody>
        {groups.map((group) => {
          const newest = group.snapshots[0];
          const expanded = open.has(group.key);
          const Chevron = expanded ? ChevronDown : ChevronRight;
          return (
            <GroupRows
              key={group.key}
              expanded={expanded}
              onToggle={() => toggle(group.key)}
              row={
                <>
                  <Td>
                    <button
                      type="button"
                      aria-expanded={expanded}
                      className="flex min-w-0 items-center gap-2 text-left focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
                    >
                      <Chevron aria-hidden className="size-4 shrink-0 text-ink-3" />
                      <KindIcon kind={group.kind} />
                      <span className="font-medium">{group.subject}</span>
                      <Badge>{kindLabel(group.kind)}</Badge>
                      {group.owner && <span className="text-ink-3">{group.owner}</span>}
                    </button>
                  </Td>
                  <Td className="whitespace-nowrap">{formatRelative(newest.time)}</Td>
                  <Td align="right" className="tabular-nums">
                    {group.snapshots.length}
                  </Td>
                  <Td align="right" className="whitespace-nowrap tabular-nums">
                    {newest.size === null ? "—" : formatBytes(newest.size)}
                  </Td>
                </>
              }
              details={<SnapshotList group={group} plans={plans} deletingId={deletingId} onRestore={onRestore} onDelete={onDelete} onHelp={onHelp} />}
            />
          );
        })}
      </tbody>
    </Table>
  );
}

/** Bir grup satırı ve (açıksa) altındaki yedek listesi. */
function GroupRows({ row, details, expanded, onToggle }: { row: ReactNode; details: ReactNode; expanded: boolean; onToggle: () => void }) {
  return (
    <>
      <tr onClick={onToggle} className={classNames("cursor-pointer hover:bg-sunken/50", expanded && "bg-sunken/40")}>
        {row}
      </tr>
      {expanded && (
        <tr>
          <td colSpan={4} className="p-0">
            {details}
          </td>
        </tr>
      )}
    </>
  );
}

// ---- Sekme ----

export function SnapshotsTab({ overview }: { overview: BackupOverview }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  const [chosenId, setChosenId] = useState<number | null>(null);
  const target = overview.targets.find((item) => item.id === chosenId) ?? overview.targets[0];

  const [filter, setFilter] = useState<FilterId>("all");
  const [search, setSearch] = useState("");
  const [running, setRunning] = useState(false);
  const [restoring, setRestoring] = useState<Snapshot | null>(null);
  const [panelHelp, setPanelHelp] = useState(false);
  const [job, setJob] = useState<JobRef | null>(null);

  const query = useQuery({
    queryKey: [...SNAPSHOTS_KEY, target.id],
    queryFn: () => api.get<{ snapshots: Snapshot[] }>(`/backup/targets/${target.id}/snapshots`),
    retry: false,
  });

  const remove = useMutation({
    mutationFn: (snapshot: Snapshot) => api.delete<JobStarted>(`/backup/targets/${target.id}/snapshots/${encodeURIComponent(snapshot.id)}`),
    onSuccess: (started) => setJob({ id: started.job_id, title: t("Yedek siliniyor") }),
    onError: (error) => feedback.error(error),
  });

  async function askDelete(snapshot: Snapshot) {
    const confirmed = await feedback.confirm({
      title: t("Bu yedek silinsin mi?"),
      description: t("{subject} için {when} tarihli yedek silinecek. Bu işlem geri alınamaz.", { subject: snapshotSubject(snapshot), when: formatDateTime(snapshot.time) }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(snapshot);
  }

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: SNAPSHOTS_KEY });
    void queryClient.invalidateQueries({ queryKey: OVERVIEW_KEY });
  }

  const snapshots = query.data?.snapshots;
  const groups = useMemo(() => (snapshots ? groupSnapshots(snapshots) : []), [snapshots]);
  const plans = useMemo(() => new Map(overview.plans.map((plan) => [plan.id, plan.name])), [overview.plans]);

  const visible = useMemo(() => {
    const needle = search.trim().toLocaleLowerCase("tr-TR");
    return groups.filter((group) => (filter === "all" || group.kind === filter) && (needle === "" || `${group.subject} ${group.owner}`.toLocaleLowerCase("tr-TR").includes(needle)));
  }, [groups, filter, search]);

  const counts = (kind: FilterId) => (kind === "all" ? groups.length : groups.filter((group) => group.kind === kind).length);
  const filters: { id: FilterId; label: string }[] = [
    { id: "all", label: `${t("Tümü")} · ${counts("all")}` },
    { id: "site", label: `${t("Siteler")} · ${counts("site")}` },
    { id: "db", label: `${t("Veritabanları")} · ${counts("db")}` },
    { id: "mail", label: `${t("Posta")} · ${counts("mail")}` },
    { id: "panel", label: `${t("Panel")} · ${counts("panel")}` },
  ];

  const newestTime = snapshots && snapshots.length > 0 ? Math.max(...snapshots.map((snapshot) => snapshot.time)) : null;
  const summary = !snapshots ? t("Yedekler okunuyor…") : t("Son yedek: {when} · {n} yedek", { when: formatRelative(newestTime), n: snapshots.length });

  let body: ReactNode;
  if (!snapshots) {
    body = query.isError ? (
      <div className="flex flex-col gap-2">
        <ErrorBlock message={errorMessage(query.error)} onRetry={() => void query.refetch()} />
        <p className="px-1 text-[13px] text-ink-3">{t("Yedek yerine ulaşılamıyor olabilir; “Yedek yerleri” sekmesinden bağlantıyı deneyin.")}</p>
      </div>
    ) : (
      <Panel flush>
        <LoadingBlock />
      </Panel>
    );
  } else if (snapshots.length === 0) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Henüz yedek yok")}
          description={t("Bu yerde henüz yedek bulunmuyor. Şimdi elle bir yedek alabilir ya da “Planlar” sekmesinden düzenli yedek planı oluşturabilirsiniz.")}
          action={
            <Button variant="primary" onClick={() => setRunning(true)}>
              {t("Şimdi yedek al")}
            </Button>
          }
        />
      </Panel>
    );
  } else {
    body = (
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          <Segmented label={t("Neye göre süz")} options={filters} value={filter} onChange={setFilter} />
          <SearchInput aria-label={t("Yedeklerde ara")} placeholder={t("Ara")} value={search} onChange={(event) => setSearch(event.target.value)} />
        </div>
        <Panel flush>
          <div className="max-h-[62vh] overflow-y-auto">
            {visible.length === 0 ? (
              <p className="px-4 py-6 text-ink-3">{t("Eşleşen yedek yok.")}</p>
            ) : (
              <GroupTable
                groups={visible}
                plans={plans}
                deletingId={remove.isPending ? remove.variables.id : null}
                onRestore={setRestoring}
                onDelete={(snapshot) => void askDelete(snapshot)}
                onHelp={() => setPanelHelp(true)}
              />
            )}
          </div>
        </Panel>
      </div>
    );
  }

  return (
    <>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
        <p className="text-ink-2">{summary}</p>
        <div className="flex flex-wrap items-center gap-2">
          {overview.targets.length > 1 && (
            <select aria-label={t("Yedek yeri")} className={COMPACT_SELECT_CLASS} value={target.id} onChange={(event) => setChosenId(Number(event.target.value))}>
              {overview.targets.map((item) => (
                <option key={item.id} value={item.id}>
                  {item.name}
                </option>
              ))}
            </select>
          )}
          <Button variant="primary" onClick={() => setRunning(true)}>
            {t("Şimdi yedek al")}
          </Button>
        </div>
      </div>

      {body}

      {running && (
        <RunNowModal
          targets={overview.targets}
          sources={overview.sources}
          initialTargetId={target.id}
          onClose={() => setRunning(false)}
          onStarted={(jobId) => {
            setRunning(false);
            setJob({ id: jobId, title: t("Yedek alınıyor") });
          }}
        />
      )}
      {restoring && (
        <RestoreWizard
          target={target}
          snapshot={restoring}
          sources={overview.sources}
          onClose={() => {
            setRestoring(null);
            refresh();
          }}
        />
      )}
      {panelHelp && <PanelRestoreHelp onClose={() => setPanelHelp(false)} />}
      <JobModal
        job={job}
        onClose={() => {
          setJob(null);
          refresh();
        }}
        onDone={refresh}
      />
    </>
  );
}
