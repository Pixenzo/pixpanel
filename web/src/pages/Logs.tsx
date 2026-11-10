import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Checkbox, EmptyState, ErrorBlock, LoadingBlock, PageHeader, Panel, SearchInput, Spinner } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

interface Source {
  id: string;
  label: string;
  group: "site" | "system";
}

interface LogView {
  lines: string[];
  exists: boolean;
}

const ERROR_PATTERN = /\b(error|crit|critical|alert|emerg|fatal|failed|panic)\b|" 5\d\d /i;
const WARNING_PATTERN = /\b(warn|warning|notice)\b|" 4\d\d /i;

function lineTone(line: string): string {
  if (ERROR_PATTERN.test(line)) return "text-bad";
  if (WARNING_PATTERN.test(line)) return "text-warn";
  return "";
}

/** Arama kutusuna yazılan değeri, yazma bitince (kısa bir bekleyişle) döner. */
function useDebounced(value: string, delay = 350): string {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = window.setTimeout(() => setDebounced(value), delay);
    return () => window.clearTimeout(timer);
  }, [value, delay]);
  return debounced;
}

export function LogsPage() {
  const sources = useQuery({ queryKey: ["log-sources"], queryFn: () => api.get<Source[]>("/logs/sources") });
  const [choice, setChoice] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [count, setCount] = useState(300);
  const [live, setLive] = useState(true);
  const query = useDebounced(filter);
  const box = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  const source = sources.data?.find((item) => item.id === choice)?.id ?? sources.data?.[0]?.id ?? "";

  const log = useQuery({
    queryKey: ["log", source, count, query],
    queryFn: () => api.get<LogView>(`/logs/read?source=${encodeURIComponent(source)}&lines=${count}&q=${encodeURIComponent(query)}`),
    enabled: source !== "",
    refetchInterval: live ? 3000 : false,
    // Kaynak ya da süzgeç değişirken eski satırlar ekranda kalır; içerik zıplamaz.
    placeholderData: keepPreviousData,
  });

  // Kullanıcı yukarı kaydırmadıysa yeni satırlar geldikçe en alta inilir.
  useEffect(() => {
    const element = box.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [log.data]);

  if (sources.isPending) return <LoadingBlock />;
  if (sources.isError) return <ErrorBlock message={errorMessage(sources.error)} />;

  if (sources.data.length === 0) {
    return (
      <>
        <PageHeader title={t("Günlükler")} />
        <Panel flush>
          <EmptyState title={t("Gösterilecek günlük yok")} description={t("Bir site oluşturduğunuzda erişim ve hata günlükleri burada görünür.")} />
        </Panel>
      </>
    );
  }

  const groups: [string, Source[]][] = [
    [t("Siteler"), sources.data.filter((item) => item.group === "site")],
    [t("Sunucu"), sources.data.filter((item) => item.group === "system")],
  ];
  const lines = log.data?.lines ?? [];

  return (
    <div className="flex h-[calc(100dvh-6.5rem)] flex-col">
      <PageHeader title={t("Günlükler")} description={t("Sitelerin erişim ve hata kayıtları ile sunucu servislerinin günlükleri.")} />
      <Panel flush className="flex min-h-0 flex-1 flex-col">
        <div className="flex flex-wrap items-center gap-3 border-b border-line px-3 py-2">
          <select
            aria-label={t("Günlük kaynağı")}
            value={source}
            onChange={(event) => {
              setChoice(event.target.value);
              pinned.current = true;
            }}
            className="h-8 max-w-64 rounded-md border border-line-strong bg-surface px-2"
          >
            {groups.map(
              ([label, items]) =>
                items.length > 0 && (
                  <optgroup key={label} label={label}>
                    {items.map((item) => (
                      <option key={item.id} value={item.id}>
                        {item.label}
                      </option>
                    ))}
                  </optgroup>
                ),
            )}
          </select>
          <SearchInput placeholder={t("Satırlarda ara")} aria-label={t("Satırlarda ara")} value={filter} onChange={(event) => setFilter(event.target.value)} />
          <select aria-label={t("Satır sayısı")} value={count} onChange={(event) => setCount(Number(event.target.value))} className="h-8 rounded-md border border-line-strong bg-surface px-2">
            {[100, 300, 1000, 2000].map((value) => (
              <option key={value} value={value}>
                {t("Son {n} satır", { n: value })}
              </option>
            ))}
          </select>
          <Checkbox label={t("Canlı izle")} checked={live} onChange={(event) => setLive(event.target.checked)} />
          {log.isFetching && <Spinner className="size-3.5 text-ink-3" />}
        </div>
        <div
          ref={box}
          tabIndex={0}
          role="log"
          aria-label={t("Günlük satırları")}
          onScroll={(event) => {
            const element = event.currentTarget;
            pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 32;
          }}
          className="min-h-0 flex-1 overflow-auto bg-sunken/50 px-3 py-2 font-mono text-[12.5px] leading-relaxed"
        >
          {log.isError && <ErrorBlock message={errorMessage(log.error)} />}
          {log.data && !log.data.exists && <p className="font-sans text-ink-2">{t("Bu günlük henüz oluşmadı; ilk kayıt düştüğünde burada görünür.")}</p>}
          {log.data?.exists && lines.length === 0 && <p className="font-sans text-ink-2">{query ? t("Aramanıza uyan satır yok.") : t("Günlük boş.")}</p>}
          {lines.map((line, index) => (
            <div key={index} className={classNames("break-all whitespace-pre-wrap", lineTone(line))}>
              {line}
            </div>
          ))}
        </div>
      </Panel>
    </div>
  );
}
