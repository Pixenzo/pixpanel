// "Neler yedeklensin?" seçici: plan oluştururken ve "Şimdi yedek al" penceresinde ortak kullanılır.

import { useId, useMemo, useState } from "react";
import { Checkbox, SearchInput } from "../components/ui";
import { t } from "../lib/i18n";
import type { BackupSources, Scope } from "./BackupShared";
import { engineLabel } from "./BackupShared";

type GroupMode = "all" | "some" | "none";

interface GroupItem {
  id: number;
  label: string;
  hint: string;
}

function initialMode(all: boolean, ids: number[]): GroupMode {
  if (all) return "all";
  return ids.length > 0 ? "some" : "none";
}

/** Bir kaynak grubu (siteler, veritabanları, posta): hepsi / seçtiklerim / hiçbiri. */
function SourceGroup({
  title,
  items,
  mode,
  onModeChange,
  selected,
  onSelectedChange,
  disabled,
}: {
  title: string;
  items: GroupItem[];
  mode: GroupMode;
  onModeChange: (next: GroupMode) => void;
  selected: number[];
  onSelectedChange: (next: number[]) => void;
  disabled?: boolean;
}) {
  const name = useId();
  const [query, setQuery] = useState("");

  const visible = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase("tr-TR");
    if (needle === "") return items;
    return items.filter((item) => `${item.label} ${item.hint}`.toLocaleLowerCase("tr-TR").includes(needle));
  }, [items, query]);

  const modes: { id: GroupMode; label: string }[] = [
    { id: "all", label: t("Hepsi (yeni eklenenler dahil)") },
    { id: "some", label: t("Seçtiklerim") },
    { id: "none", label: t("Hiçbiri") },
  ];

  function toggle(id: number, checked: boolean) {
    onSelectedChange(checked ? [...selected, id] : selected.filter((value) => value !== id));
  }

  return (
    <fieldset disabled={disabled} className="flex min-w-0 flex-col gap-2 p-3">
      <legend className="sr-only">{title}</legend>
      <div className="font-medium">{title}</div>
      <div className="flex flex-wrap gap-x-5 gap-y-1">
        {modes.map((option) => (
          <label key={option.id} className="flex items-center gap-2">
            <input type="radio" name={name} className="accent-(--accent)" checked={mode === option.id} onChange={() => onModeChange(option.id)} />
            {option.label}
          </label>
        ))}
      </div>

      {mode === "some" && (
        <div className="flex flex-col gap-2">
          {items.length === 0 ? (
            <p className="text-ink-3">{t("Henüz seçilebilecek bir şey yok.")}</p>
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <SearchInput aria-label={t("{title} içinde ara", { title })} placeholder={t("Ara")} value={query} onChange={(event) => setQuery(event.target.value)} />
                <span className="text-[12.5px] text-ink-3">{t("{n} seçili", { n: selected.length })}</span>
              </div>
              <ul className="max-h-48 overflow-y-auto rounded-md border border-line">
                {visible.length === 0 && <li className="px-3 py-2 text-ink-3">{t("Eşleşen sonuç yok.")}</li>}
                {visible.map((item) => (
                  <li key={item.id} className="border-b border-line px-3 py-1.5 last:border-b-0">
                    <label className="flex items-center gap-2.5">
                      <input type="checkbox" className="size-4 accent-(--accent)" checked={selected.includes(item.id)} onChange={(event) => toggle(item.id, event.target.checked)} />
                      <span className="min-w-0 flex-1 truncate">{item.label}</span>
                      <span className="shrink-0 text-[12.5px] text-ink-3">{item.hint}</span>
                    </label>
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
    </fieldset>
  );
}

/** Nelerin yedekleneceğini seçtiren bileşen. */
export function ScopePicker({ value, onChange, sources, disabled }: { value: Scope; onChange: (next: Scope) => void; sources: BackupSources; disabled?: boolean }) {
  const [modes, setModes] = useState(() => ({
    sites: initialMode(value.all_sites, value.site_ids),
    databases: initialMode(value.all_databases, value.database_ids),
    mail: initialMode(value.all_mail, value.mail_domain_ids),
  }));

  const siteItems = useMemo<GroupItem[]>(() => sources.sites.map((site) => ({ id: site.id, label: site.domain, hint: site.owner })), [sources.sites]);
  const databaseItems = useMemo<GroupItem[]>(
    () => sources.databases.map((database) => ({ id: database.id, label: database.name, hint: `${engineLabel(database.engine)} · ${database.owner}` })),
    [sources.databases],
  );
  const mailItems = useMemo<GroupItem[]>(() => sources.mail_domains.map((domain) => ({ id: domain.id, label: domain.domain, hint: domain.owner })), [sources.mail_domains]);

  return (
    <div className="flex flex-col divide-y divide-line rounded-lg border border-line">
      <SourceGroup
        title={t("Siteler")}
        items={siteItems}
        mode={modes.sites}
        disabled={disabled}
        onModeChange={(mode) => {
          setModes((current) => ({ ...current, sites: mode }));
          onChange({ ...value, all_sites: mode === "all", site_ids: mode === "some" ? value.site_ids : [] });
        }}
        selected={value.site_ids}
        onSelectedChange={(ids) => onChange({ ...value, site_ids: ids })}
      />
      <SourceGroup
        title={t("Veritabanları")}
        items={databaseItems}
        mode={modes.databases}
        disabled={disabled}
        onModeChange={(mode) => {
          setModes((current) => ({ ...current, databases: mode }));
          onChange({ ...value, all_databases: mode === "all", database_ids: mode === "some" ? value.database_ids : [] });
        }}
        selected={value.database_ids}
        onSelectedChange={(ids) => onChange({ ...value, database_ids: ids })}
      />
      <SourceGroup
        title={t("Posta")}
        items={mailItems}
        mode={modes.mail}
        disabled={disabled}
        onModeChange={(mode) => {
          setModes((current) => ({ ...current, mail: mode }));
          onChange({ ...value, all_mail: mode === "all", mail_domain_ids: mode === "some" ? value.mail_domain_ids : [] });
        }}
        selected={value.mail_domain_ids}
        onSelectedChange={(ids) => onChange({ ...value, mail_domain_ids: ids })}
      />
      <div className="p-3">
        <Checkbox
          label={t("Panel ayarları")}
          hint={t("Hesaplar, site ayarları, sertifikalar. Sunucu tümüyle kaybedilirse paneli geri kurmak için gerekir.")}
          checked={value.panel}
          disabled={disabled}
          onChange={(event) => onChange({ ...value, panel: event.target.checked })}
        />
      </div>
    </div>
  );
}
