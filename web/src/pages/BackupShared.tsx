// Yedekler sayfasının ortak türleri, yardımcıları ve küçük bileşenleri.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, Copy } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, Modal, Mono } from "../components/ui";
import { api } from "../lib/api";
import { classNames, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

// ---- Türler ----

export type TargetKind = "local" | "sftp" | "s3";
export type Frequency = "manual" | "daily" | "weekly" | "monthly";
export type SnapshotKind = "site" | "db" | "mail" | "panel";

export interface TargetConfig {
  path: string;
  host: string;
  port: number;
  user: string;
  endpoint: string;
  bucket: string;
  region: string;
  access_key: string;
}

export interface Target {
  id: number;
  name: string;
  kind: TargetKind;
  config: TargetConfig;
  has_secret_key: boolean;
  /** Yalnızca SFTP yerlerinde dolu. */
  public_key: string;
  last_check_at: number | null;
  last_check_ok: boolean;
  last_check_error: string;
  created_at: number;
  plan_count: number;
}

export interface Scope {
  all_sites: boolean;
  site_ids: number[];
  all_databases: boolean;
  database_ids: number[];
  all_mail: boolean;
  mail_domain_ids: number[];
  panel: boolean;
}

export interface Plan {
  id: number;
  user_id: number;
  name: string;
  target_id: number;
  scope: Scope;
  frequency: Frequency;
  /** Gece yarısından sonraki dakika (sunucunun yerel saati). */
  at_minute: number;
  /** 1 = Pazartesi … 7 = Pazar. */
  weekday: number;
  month_day: number;
  keep_daily: number;
  keep_weekly: number;
  keep_monthly: number;
  enabled: boolean;
  next_run_at: number | null;
  last_run_at: number | null;
  last_status: "" | "running" | "success" | "failed";
  last_job_id: number | null;
  created_at: number;
}

export interface SourceSite {
  id: number;
  domain: string;
  owner: string;
}

export interface SourceDatabase {
  id: number;
  engine: "mariadb" | "postgresql";
  name: string;
  owner: string;
}

export interface SourceMailDomain {
  id: number;
  domain: string;
  owner: string;
}

export interface BackupSources {
  sites: SourceSite[];
  databases: SourceDatabase[];
  mail_domains: SourceMailDomain[];
}

export interface BackupOverview {
  installed: boolean;
  /** `false`: kurulu bileşen çok eski. */
  supported: boolean;
  version: string;
  targets: Target[];
  plans: Plan[];
  sources: BackupSources;
}

export interface Snapshot {
  id: string;
  short_id: string;
  /** Unix saniyesi. */
  time: number;
  kind: SnapshotKind;
  name: string;
  /** Yalnızca veritabanı yedeklerinde dolu. */
  engine: string;
  owner: string;
  /** `null`: elle alınmış. */
  plan_id: number | null;
  paths: string[];
  size: number | null;
}

export interface Entry {
  name: string;
  path: string;
  kind: "dir" | "file" | "symlink";
  size: number;
  mtime: number;
}

export interface JobStarted {
  job_id: number;
}

/** Açık bir görev penceresinin bilgisi. */
export interface JobRef {
  id: number;
  title: string;
}

// ---- Sorgu anahtarları ve ortak kancalar ----

export const OVERVIEW_KEY = ["backup", "overview"] as const;
export const SNAPSHOTS_KEY = ["backup", "snapshots"] as const;

const OVERVIEW_POLL_MS = 10_000;

/** Yedekleme özeti; çalışan bir plan varken 10 saniyede bir yenilenir. */
export function useBackupOverview() {
  return useQuery({
    queryKey: OVERVIEW_KEY,
    queryFn: () => api.get<BackupOverview>("/backup"),
    refetchInterval: (query) => (query.state.data?.plans.some((plan) => plan.last_status === "running") ? OVERVIEW_POLL_MS : false),
  });
}

/** Hedef/plan işlemlerinin döndürdüğü taze özeti önbelleğe koyan işlev. */
export function useOverviewSetter(): (next: BackupOverview) => void {
  const queryClient = useQueryClient();
  return (next) => queryClient.setQueryData(OVERVIEW_KEY, next);
}

// ---- Kapsam yardımcıları ----

export function fullScope(): Scope {
  return { all_sites: true, site_ids: [], all_databases: true, database_ids: [], all_mail: true, mail_domain_ids: [], panel: true };
}

export function isScopeEmpty(scope: Scope): boolean {
  return (
    !scope.all_sites &&
    scope.site_ids.length === 0 &&
    !scope.all_databases &&
    scope.database_ids.length === 0 &&
    !scope.all_mail &&
    scope.mail_domain_ids.length === 0 &&
    !scope.panel
  );
}

/** "Bütün siteler, 2 veritabanı, panel ayarları" gibi kısa bir döküm. */
export function describeScope(scope: Scope): string {
  const parts: string[] = [];
  if (scope.all_sites) parts.push(t("bütün siteler"));
  else if (scope.site_ids.length > 0) parts.push(t("{n} site", { n: scope.site_ids.length }));
  if (scope.all_databases) parts.push(t("bütün veritabanları"));
  else if (scope.database_ids.length > 0) parts.push(t("{n} veritabanı", { n: scope.database_ids.length }));
  if (scope.all_mail) parts.push(t("bütün posta alanları"));
  else if (scope.mail_domain_ids.length > 0) parts.push(t("{n} posta alanı", { n: scope.mail_domain_ids.length }));
  if (scope.panel) parts.push(t("panel ayarları"));
  if (parts.length === 0) return t("Hiçbir şey seçilmemiş");
  const text = parts.join(", ");
  return text.charAt(0).toLocaleUpperCase("tr-TR") + text.slice(1);
}

// ---- Etiketler ----

export function kindLabel(kind: SnapshotKind): string {
  switch (kind) {
    case "site":
      return t("Site");
    case "db":
      return t("Veritabanı");
    case "mail":
      return t("Posta");
    case "panel":
      return t("Panel");
  }
}

export function engineLabel(engine: string): string {
  if (engine === "mariadb") return "MariaDB";
  if (engine === "postgresql") return "PostgreSQL";
  return engine;
}

/** Bir yedeğin neyin yedeği olduğunu söyleyen ad: "blog (MariaDB)", "Panel ayarları". */
export function snapshotSubject(snapshot: Pick<Snapshot, "kind" | "name" | "engine">): string {
  if (snapshot.kind === "panel") return t("Panel ayarları");
  if (snapshot.kind === "db") return `${snapshot.name} (${engineLabel(snapshot.engine)})`;
  return snapshot.name;
}

export const WEEKDAYS = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];

// ---- Zaman yardımcıları ----

const clockFormat = new Intl.DateTimeFormat("tr-TR", { hour: "2-digit", minute: "2-digit" });
const longDateTimeFormat = new Intl.DateTimeFormat("tr-TR", { day: "numeric", month: "long", year: "numeric", hour: "2-digit", minute: "2-digit" });

/** "10 Kasım 2026 03:00" */
export function formatLongDateTime(unixSeconds: number): string {
  return longDateTimeFormat.format(new Date(unixSeconds * 1000));
}

/** Dakika sayısını (gece yarısından beri) "HH:MM" yapar. */
export function minutesToClock(minutes: number): string {
  const hours = Math.floor(minutes / 60) % 24;
  return `${String(hours).padStart(2, "0")}:${String(minutes % 60).padStart(2, "0")}`;
}

/** Yaklaşan zamanı "bugün 03:00", "yarın 03:00" ya da tam tarih olarak yazar. */
export function formatUpcoming(unixSeconds: number): string {
  const date = new Date(unixSeconds * 1000);
  const now = new Date();
  const dayStart = (value: Date) => new Date(value.getFullYear(), value.getMonth(), value.getDate()).getTime();
  const days = Math.round((dayStart(date) - dayStart(now)) / 86_400_000);
  const clock = clockFormat.format(date);
  if (days === 0) return t("bugün {time}", { time: clock });
  if (days === 1) return t("yarın {time}", { time: clock });
  return formatDateTime(unixSeconds);
}

// ---- Ortak sınıflar ----

export const COMPACT_SELECT_CLASS =
  "h-8 max-w-full rounded-md border border-line-strong bg-surface px-2 pr-7 text-[13px] text-ink " +
  "transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 disabled:bg-sunken disabled:text-ink-3";

// ---- Küçük bileşenler ----

type NoticeTone = "warn" | "bad" | "accent";

const noticeClass: Record<NoticeTone, string> = {
  warn: "border-warn/50 bg-warn-soft text-warn",
  bad: "border-bad/40 bg-bad-soft text-bad",
  accent: "border-transparent bg-accent-soft text-ink",
};

/** Dikkat çeken kısa not; hata ve uyarılar için `alert` rolüyle okunur. */
export function Notice({ tone, children, className }: { tone: NoticeTone; children: ReactNode; className?: string }) {
  return (
    <div role={tone === "accent" ? "note" : "alert"} className={classNames("rounded-lg border px-3.5 py-2.5", noticeClass[tone], className)}>
      {children}
    </div>
  );
}

/** Erişilebilir açma/kapama anahtarı. */
export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={classNames(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors",
        "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-55",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span aria-hidden className={classNames("size-4 rounded-full bg-white transition-transform", checked ? "translate-x-[1.125rem]" : "translate-x-0.5")} />
    </button>
  );
}

/** Bölmeli seçim (radyo grubu gibi çalışır); süzgeç ve sıklık seçimlerinde kullanılır. */
export function Segmented<T extends string>({
  options,
  value,
  onChange,
  label,
  disabled,
}: {
  options: { id: T; label: string }[];
  value: T;
  onChange: (next: T) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex max-w-full flex-wrap overflow-hidden rounded-md border border-line-strong">
      {options.map((option, index) => (
        <button
          key={option.id}
          type="button"
          role="radio"
          aria-checked={value === option.id}
          disabled={disabled}
          onClick={() => onChange(option.id)}
          className={classNames(
            "h-8 px-3 text-[13px] font-medium whitespace-nowrap transition-colors disabled:cursor-not-allowed disabled:opacity-55",
            index > 0 && "border-l border-line-strong",
            value === option.id ? "bg-accent-soft text-accent" : "bg-surface text-ink-2 hover:bg-sunken hover:text-ink",
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** Metindeki `{ad}` yer tutucularını eş aralıklı değerle değiştirir. */
export function MonoSentence({ template, values }: { template: string; values: Record<string, string> }) {
  const parts = template.split(/(\{\w+\})/);
  return (
    <>
      {parts.map((part, index) => {
        const key = part.slice(1, -1);
        return /^\{\w+\}$/.test(part) && key in values ? <Mono key={index}>{values[key]}</Mono> : <span key={index}>{part}</span>;
      })}
    </>
  );
}

/** Kopyalanabilir eş aralıklı metin kutusu (anahtar, parola vb.). */
export function CopyBox({ value, label }: { value: string; label: string }) {
  const feedback = useFeedback();
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      feedback.error(new Error(t("Kopyalanamadı; metni elle seçip kopyalayın")));
    }
  }

  return (
    <div className="flex items-start gap-2">
      <div
        tabIndex={0}
        aria-label={label}
        className="max-h-32 min-w-0 flex-1 overflow-auto rounded-md border border-line bg-sunken px-3 py-2 font-mono text-[12.5px] leading-relaxed break-all select-all"
      >
        {value}
      </div>
      <Button size="sm" icon={copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />} onClick={() => void copy()}>
        {copied ? t("Kopyalandı") : t("Kopyala")}
      </Button>
    </div>
  );
}

/** Bir görevin canlı günlüğünü gösteren pencere. */
export function JobModal({ job, onClose, onDone }: { job: JobRef | null; onClose: () => void; onDone?: (status: string) => void }) {
  return (
    <Modal open={job !== null} onClose={onClose} title={job?.title ?? ""} wide footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      {job && <JobLog jobId={job.id} onDone={onDone} />}
    </Modal>
  );
}
