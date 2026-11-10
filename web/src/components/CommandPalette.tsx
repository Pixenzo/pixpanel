// Hızlı arama (Ctrl+K): sayfalara, sitelere ve bir sitenin araçlarına klavyeden atlama.

import * as Dialog from "@radix-ui/react-dialog";
import { useQuery } from "@tanstack/react-query";
import type { LucideIcon } from "lucide-react";
import { CornerDownLeft, Globe, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router";
import { navigation } from "../layout/navigation";
import type { Site } from "../lib/api";
import { api } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { siteTools } from "../lib/siteTools";

interface Entry {
  key: string;
  label: string;
  /** Sitenin aracı ise sitenin adı. */
  context?: string;
  icon: LucideIcon;
  to: string;
  /** Aramada eşleştirilen, küçük harfe çevrilmiş metin. */
  haystack: string;
}

const MAX_RESULTS = 12;
const fold = (text: string) => text.toLocaleLowerCase("tr");

export function CommandPalette({ open, onClose }: { open: boolean; onClose: () => void }) {
  const me = useMe();
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const canSites = me.role !== "member" || me.permissions.includes("sites");
  const sites = useQuery({ queryKey: ["sites"], queryFn: () => api.get<Site[]>("/sites"), enabled: open && canSites });

  const entries = useMemo<Entry[]>(() => {
    const pages: Entry[] = navigation
      .flatMap((group) => group.items)
      .filter((item) => item.visible(me))
      .map((item) => ({ key: item.to, label: t(item.label), icon: item.icon, to: item.to, haystack: fold(t(item.label)) }));
    const siteEntries: Entry[] = (sites.data ?? []).flatMap((site) => [
      { key: `site-${site.id}`, label: site.domain, icon: Globe, to: `/sites/${site.id}`, haystack: fold(`${site.domain} ${site.aliases.join(" ")}`) },
      ...siteTools(site, me).map((tool) => ({
        key: `site-${site.id}-${tool.id}`,
        label: tool.label,
        context: site.domain,
        icon: tool.icon,
        to: tool.to,
        haystack: fold(`${site.domain} ${tool.label}`),
      })),
    ]);
    return [...pages, ...siteEntries];
  }, [me, sites.data]);

  const results = useMemo(() => {
    const words = fold(query).split(/\s+/).filter(Boolean);
    // Boş aramada yalnızca sayfalar ve siteler listelenir; araçlar yazmaya başlayınca gelir.
    const pool = words.length === 0 ? entries.filter((entry) => !entry.context) : entries;
    return pool.filter((entry) => words.every((word) => entry.haystack.includes(word))).slice(0, MAX_RESULTS);
  }, [entries, query]);

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);
  useEffect(() => {
    list.current?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [active]);

  function go(entry: Entry | undefined) {
    if (!entry) return;
    onClose();
    void navigate(entry.to);
  }

  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 animate-[fade-in_100ms_ease-out] bg-[rgb(10_16_22/0.45)]" />
        <Dialog.Content
          aria-describedby={undefined}
          className="fixed top-[14vh] left-1/2 z-50 w-[calc(100vw-2rem)] max-w-xl -translate-x-1/2 animate-[pop-in_120ms_ease-out] overflow-hidden rounded-xl border border-line bg-surface shadow-pop"
        >
          <Dialog.Title className="sr-only">{t("Hızlı arama")}</Dialog.Title>
          <div className="flex items-center gap-2.5 border-b border-line px-4">
            <Search aria-hidden className="size-4 shrink-0 text-ink-3" />
            <input
              role="combobox"
              aria-expanded
              aria-controls="palette-results"
              aria-label={t("Site, araç ya da sayfa ara")}
              placeholder={t("Site, araç ya da sayfa ara")}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "ArrowDown") {
                  event.preventDefault();
                  setActive((index) => Math.min(index + 1, results.length - 1));
                } else if (event.key === "ArrowUp") {
                  event.preventDefault();
                  setActive((index) => Math.max(index - 1, 0));
                } else if (event.key === "Enter") {
                  event.preventDefault();
                  go(results[active]);
                }
              }}
              className="h-12 min-w-0 flex-1 bg-transparent text-[15px] outline-none placeholder:text-ink-3"
              autoFocus
              spellCheck={false}
            />
          </div>
          <ul ref={list} id="palette-results" role="listbox" className="max-h-[52vh] overflow-y-auto p-1.5">
            {results.length === 0 && <li className="px-3 py-6 text-center text-ink-2">{t("Aramanıza uyan bir şey yok.")}</li>}
            {results.map((entry, index) => (
              <li
                key={entry.key}
                role="option"
                aria-selected={index === active}
                onMouseEnter={() => setActive(index)}
                onClick={() => go(entry)}
                className={classNames("flex h-10 cursor-pointer items-center gap-2.5 rounded-md px-2.5", index === active && "bg-accent-soft")}
              >
                <entry.icon aria-hidden className={classNames("size-4 shrink-0", index === active ? "text-accent" : "text-ink-3")} />
                {entry.context && <span className="shrink-0 font-mono text-[12.5px] text-ink-2">{entry.context}</span>}
                <span className={classNames("min-w-0 truncate", !entry.context && entry.key.startsWith("site-") && "font-mono text-[13px]")}>{entry.label}</span>
                {index === active && <CornerDownLeft aria-hidden className="ml-auto size-3.5 shrink-0 text-ink-3" />}
              </li>
            ))}
          </ul>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
