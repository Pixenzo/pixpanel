import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { OctagonX } from "lucide-react";
import { useMemo, useState } from "react";
import { useFeedback } from "../components/feedback";
import { ErrorBlock, IconButton, LoadingBlock, PageHeader, Panel, SearchInput, Table, Td, Th } from "../components/ui";
import type { Process } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDuration, formatPercent } from "../lib/format";
import { t } from "../lib/i18n";

type SortKey = "cpu" | "memory" | "pid" | "user";

const sortLabels: Record<SortKey, string> = {
  cpu: "İşlemci",
  memory: "Bellek",
  pid: "PID",
  user: "Kullanıcı",
};

export function ProcessesPage() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [sortKey, setSortKey] = useState<SortKey>("cpu");

  const processes = useQuery({
    queryKey: ["processes"],
    queryFn: () => api.get<Process[]>("/system/processes"),
    refetchInterval: 3000,
  });

  const signal = useMutation({
    mutationFn: ({ pid, signal }: { pid: number; signal: string }) => api.post(`/system/processes/${pid}/signal`, { signal }),
    onSuccess: () => feedback.success(t("Süreç sonlandırıldı")),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["processes"] }),
  });

  const rows = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const list = (processes.data ?? []).filter(
      (process) =>
        !needle ||
        process.command.toLowerCase().includes(needle) ||
        process.user.toLowerCase().includes(needle) ||
        String(process.pid) === needle,
    );
    const compare: Record<SortKey, (a: Process, b: Process) => number> = {
      cpu: (a, b) => b.cpu - a.cpu || b.memory - a.memory,
      memory: (a, b) => b.memory - a.memory,
      pid: (a, b) => a.pid - b.pid,
      user: (a, b) => a.user.localeCompare(b.user) || b.cpu - a.cpu,
    };
    return [...list].sort(compare[sortKey]);
  }, [processes.data, search, sortKey]);

  async function terminate(process: Process) {
    const confirmed = await feedback.confirm({
      title: t("{name} sonlandırılsın mı?", { name: process.name }),
      description: t("PID {pid}. Süreç kaydetmediği verileri kaybedebilir.", { pid: process.pid }),
      confirmLabel: t("Sonlandır"),
      danger: true,
    });
    if (confirmed) signal.mutate({ pid: process.pid, signal: "TERM" });
  }

  const sortButton = (key: SortKey) => (
    <button
      type="button"
      onClick={() => setSortKey(key)}
      aria-pressed={sortKey === key}
      className={classNames("font-medium hover:text-ink", sortKey === key && "text-accent underline underline-offset-4")}
    >
      {t(sortLabels[key])}
    </button>
  );

  return (
    <>
      <PageHeader title={t("Süreçler")} description={t("Liste 3 saniyede bir yenilenir. Sıralamak için sütun başlığına tıklayın.")} />
      {processes.isError && <ErrorBlock message={errorMessage(processes.error)} onRetry={() => void processes.refetch()} />}
      {processes.isPending && <LoadingBlock />}
      {processes.data && (
        <Panel
          flush
          title={
            <SearchInput
              placeholder={t("Komut, kullanıcı ya da PID ara")}
              aria-label={t("Süreç ara")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
            />
          }
          actions={<span className="tabular text-[13px] text-ink-3">{t("{count} süreç", { count: rows.length })}</span>}
        >
          <Table>
            <thead>
              <tr>
                <Th align="right">{sortButton("pid")}</Th>
                <Th>{sortButton("user")}</Th>
                <Th className="w-full">{t("Komut")}</Th>
                <Th align="right">{sortButton("cpu")}</Th>
                <Th align="right">{sortButton("memory")}</Th>
                <Th align="right">{t("Süre")}</Th>
                <Th />
              </tr>
            </thead>
            <tbody>
              {rows.map((process) => (
                <tr key={process.pid} className="hover:bg-sunken/50">
                  <Td align="right" className="font-mono text-[12.5px] text-ink-2">
                    {process.pid}
                  </Td>
                  <Td className="whitespace-nowrap">{process.user}</Td>
                  <Td className="max-w-0">
                    <span className="block truncate font-mono text-[12.5px]" title={process.command}>
                      {process.command}
                    </span>
                  </Td>
                  <Td align="right" className={classNames(process.cpu >= 50 && "font-medium text-warn")}>
                    {formatPercent(process.cpu, 1)}
                  </Td>
                  <Td align="right" className="whitespace-nowrap">
                    {formatBytes(process.memory)}
                  </Td>
                  <Td align="right" className="whitespace-nowrap text-ink-2">
                    {formatDuration(process.elapsed_secs)}
                  </Td>
                  <Td align="right" className="py-0.5">
                    <IconButton label={t("Sonlandır")} disabled={process.pid === 1} onClick={() => void terminate(process)}>
                      <OctagonX className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        </Panel>
      )}
    </>
  );
}
