// Saldırı engelleme sayfası (fail2ban): parola tahmin etmeye çalışan adresleri bir süreliğine uzak tutar.
// Sayfa iki işe odaklanır: engellenenleri görüp kaldırmak ve kendi adresini güvenli listeye almak.
// Bölüm 1–5'teki her denetim değişince hemen kaydeder; taslak çubuğu yoktur. Ayrıntılar "Gelişmiş" altındadır.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, ChevronRight, X } from "lucide-react";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { SecurityTabs } from "../components/SecurityTabs";
import { Badge, Button, Checkbox, EmptyState, ErrorBlock, IconButton, LoadingBlock, Mono, PageHeader, Panel, Spinner, Status, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Türler ----

interface JailSettings {
  key: string;
  enabled: boolean;
  max_retry: number;
  /** Saniye. */
  find_time: number;
  /** Saniye. */
  ban_time: number;
}

interface Settings {
  jails: JailSettings[];
  /** Hiçbir zaman engellenmeyen adresler ve ağlar. */
  whitelist: string[];
  /** Tekrar eden adreslerin engel süresi giderek uzar. */
  escalate: boolean;
}

interface JailStatus {
  key: string;
  label: string;
  description: string;
  /** İzlenen bileşen kurulu mu. */
  available: boolean;
  /** fail2ban içinde şu an çalışıyor mu. */
  active: boolean;
  failed_now: number;
  failed_total: number;
  banned_total: number;
  banned: string[];
}

interface Fail2banView {
  installed: boolean;
  running: boolean;
  settings: Settings;
  jails: JailStatus[];
  your_ip: string;
}

interface BannedRow {
  ip: string;
  jailKey: string;
  jailLabel: string;
}

interface UnbanRequest {
  ip: string;
  jail?: string;
}

interface BanRequest {
  ip: string;
  jail: string;
}

type DurationUnit = "second" | "minute" | "hour" | "day";

/** Bir korumanın kullanıcı tarafından düzenlenebilen sayıları. */
type JailNumbers = Pick<JailSettings, "max_retry" | "find_time" | "ban_time">;

type LevelId = "loose" | "balanced" | "strict";

interface Level {
  id: LevelId;
  title: string;
  description: string;
  recommended?: boolean;
  numbers: JailNumbers;
}

/** Hangi bölümün kaydettiği; döner simge yalnızca o bölümde görünür. */
type SaveScope = "self" | "level" | "whitelist" | "jails" | "advanced";

interface SaveOptions {
  scope: SaveScope;
  /** Başarı bildirimi; verilmezse "Kaydedildi". */
  message?: string;
  onSuccess?: () => void;
  /** Verilirse hata bildirim yerine bu işlevle (ör. satır içi) gösterilir. */
  onError?: (error: unknown) => void;
}

/** Ayarları sunucuya yazan ortak işlev ve durumu. */
interface Saver {
  pending: boolean;
  scope: SaveScope | null;
  run: (next: Settings, options: SaveOptions) => void;
}

const QUERY_KEY = ["fail2ban"] as const;
const POLL_INTERVAL_MS = 10_000;

const UNIT_SECONDS: Record<DurationUnit, number> = { second: 1, minute: 60, hour: 3600, day: 86400 };
/** Büyükten küçüğe; en uygun birim seçilirken bu sırayla denenir. */
const UNITS_DESC: DurationUnit[] = ["day", "hour", "minute", "second"];

const DEFAULT_JAIL: Omit<JailSettings, "key"> = { enabled: false, max_retry: 5, find_time: 600, ban_time: 3600 };

const FIND_TIME_SECONDS = 600;

const LEVELS: Level[] = [
  { id: "loose", title: "Gevşek", description: "10 hatalı denemede 15 dakika engelle", numbers: { max_retry: 10, find_time: FIND_TIME_SECONDS, ban_time: 900 } },
  { id: "balanced", title: "Dengeli", description: "5 hatalı denemede 1 saat engelle", recommended: true, numbers: { max_retry: 5, find_time: FIND_TIME_SECONDS, ban_time: 3600 } },
  { id: "strict", title: "Sıkı", description: "3 hatalı denemede 1 gün engelle", numbers: { max_retry: 3, find_time: FIND_TIME_SECONDS, ban_time: 86400 } },
];

const CELL_CLASS = "h-8 min-w-0 rounded-md border border-line-strong bg-surface px-2 text-[13.5px] tabular-nums placeholder:text-ink-3 disabled:bg-sunken disabled:text-ink-3";
const SELECT_CLASS = "h-8 rounded-md border border-line-strong bg-surface px-1.5 text-[13.5px] disabled:bg-sunken disabled:text-ink-3";
const TEXT_INPUT_CLASS =
  "h-9 min-w-0 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px] text-ink placeholder:font-sans placeholder:text-ink-3 " +
  "transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 disabled:bg-sunken disabled:text-ink-3";

// ---- Yardımcılar ----

/** Boşlukları temizler ve boş satırları atar; gönderim bu biçimle yapılır. */
function normalizeSettings(settings: Settings): Settings {
  return {
    jails: settings.jails,
    escalate: settings.escalate,
    whitelist: cleanWhitelist(settings.whitelist),
  };
}

function cleanWhitelist(entries: string[]): string[] {
  return entries.map((line) => line.trim()).filter((line) => line !== "");
}

function jailSettingsOf(settings: Settings, key: string): JailSettings {
  return settings.jails.find((jail) => jail.key === key) ?? { key, ...DEFAULT_JAIL };
}

/** Bir korumanın ayarını günceller; ayarlarda yoksa varsayılanlarla ekler. */
function withJail(settings: Settings, key: string, changes: Partial<JailSettings>): Settings {
  const exists = settings.jails.some((jail) => jail.key === key);
  const jails = exists
    ? settings.jails.map((jail) => (jail.key === key ? { ...jail, ...changes } : jail))
    : [...settings.jails, { ...jailSettingsOf(settings, key), ...changes }];
  return { ...settings, jails };
}

/** Ayarlarda ya da durum listesinde geçen bütün koruma anahtarları. */
function allJailKeys(view: Fail2banView): string[] {
  return Array.from(new Set([...view.settings.jails.map((jail) => jail.key), ...view.jails.map((jail) => jail.key)]));
}

/** Bütün korumaların sayıları aynı seviyeye uyuyorsa onu, uymuyorsa "custom" döndürür. */
function detectLevel(view: Fail2banView): LevelId | "custom" | null {
  const keys = allJailKeys(view);
  if (keys.length === 0) return null;
  const jails = keys.map((key) => jailSettingsOf(view.settings, key));
  const match = LEVELS.find((level) =>
    jails.every((jail) => jail.max_retry === level.numbers.max_retry && jail.find_time === level.numbers.find_time && jail.ban_time === level.numbers.ban_time),
  );
  return match ? match.id : "custom";
}

function unitLabel(unit: DurationUnit): string {
  switch (unit) {
    case "second":
      return t("saniye");
    case "minute":
      return t("dakika");
    case "hour":
      return t("saat");
    case "day":
      return t("gün");
  }
}

function parseUnit(value: string): DurationUnit {
  return value === "day" || value === "hour" || value === "minute" ? value : "second";
}

/** Değeri tam bölen en büyük birimi bulur. */
function bestUnit(seconds: number): DurationUnit {
  if (seconds <= 0) return "second";
  return UNITS_DESC.find((unit) => seconds % UNIT_SECONDS[unit] === 0) ?? "second";
}

/** Metin alanındaki sayıyı okur; boş alan 0 sayılır, geçersiz alan NaN olur. */
function parseAmount(text: string): number {
  const trimmed = text.trim();
  return trimmed === "" ? 0 : Number(trimmed);
}

function flattenBanned(jails: JailStatus[]): BannedRow[] {
  return jails.flatMap((jail) => jail.banned.map((ip) => ({ ip, jailKey: jail.key, jailLabel: jail.label })));
}

function sameNumbers(a: JailNumbers, b: JailNumbers): boolean {
  return a.max_retry === b.max_retry && a.find_time === b.find_time && a.ban_time === b.ban_time;
}

function validNumbers(numbers: JailNumbers): boolean {
  return [numbers.max_retry, numbers.find_time, numbers.ban_time].every((value) => Number.isFinite(value) && value > 0);
}

// ---- Küçük bileşenler ----

/** Erişilebilir açma/kapama anahtarı. */
function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
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

/** Çevrilmiş cümlenin `{ip}` yerine eş aralıklı adres koyar. */
function SentenceWithIp({ template, ip }: { template: string; ip: string }) {
  const [before, ...rest] = template.split("{ip}");
  return (
    <>
      {before}
      <Mono>{ip}</Mono>
      {rest.join("{ip}")}
    </>
  );
}

/** Kaydederken bölüm başlığının yanında görünen küçük döner simge. */
function SavingSpinner({ saver, scope }: { saver: Saver; scope: SaveScope }) {
  if (!saver.pending || saver.scope !== scope) return null;
  return <Spinner className="size-3.5 text-ink-3" />;
}

/** Tam sayı girişi; yazarken geçici olarak boş kalabilir, dışarıdan gelen değer değişirse metin yenilenir. */
function CountInput({ value, onChange, label, disabled }: { value: number; onChange: (next: number) => void; label: string; disabled?: boolean }) {
  const [text, setText] = useState(() => String(value));

  // Değer dışarıdan değiştiyse metin yeni değere uyar.
  if (parseAmount(text) !== value && !(Number.isNaN(value) && Number.isNaN(parseAmount(text)))) setText(String(value));

  return (
    <input
      type="number"
      inputMode="numeric"
      min={1}
      max={100}
      step={1}
      aria-label={label}
      value={text}
      disabled={disabled}
      onChange={(event) => {
        setText(event.target.value);
        onChange(Math.round(parseAmount(event.target.value)));
      }}
      className={classNames(CELL_CLASS, "w-16")}
    />
  );
}

/** Süre girişi: sayı ve birim seçimi; değer her zaman saniye olarak yukarı iletilir. */
function DurationInput({ seconds, onChange, label, disabled }: { seconds: number; onChange: (next: number) => void; label: string; disabled?: boolean }) {
  const [unit, setUnit] = useState<DurationUnit>(() => bestUnit(seconds));
  const [text, setText] = useState(() => String(seconds / UNIT_SECONDS[bestUnit(seconds)]));

  // Değer dışarıdan değiştiyse sayı ve birim yeni değere göre yeniden seçilir.
  const typed = Math.round(parseAmount(text) * UNIT_SECONDS[unit]);
  if (typed !== seconds && !(Number.isNaN(typed) && Number.isNaN(seconds))) {
    const next = bestUnit(seconds);
    setUnit(next);
    setText(String(seconds / UNIT_SECONDS[next]));
  }

  return (
    <div className="flex items-center gap-1">
      <input
        type="number"
        inputMode="decimal"
        min={0}
        step="any"
        aria-label={label}
        value={text}
        disabled={disabled}
        onChange={(event) => {
          setText(event.target.value);
          onChange(Math.round(parseAmount(event.target.value) * UNIT_SECONDS[unit]));
        }}
        className={classNames(CELL_CLASS, "w-16")}
      />
      <select
        aria-label={t("{label} birimi", { label })}
        value={unit}
        disabled={disabled}
        onChange={(event) => {
          const next = parseUnit(event.target.value);
          setUnit(next);
          onChange(Math.round(parseAmount(text) * UNIT_SECONDS[next]));
        }}
        className={SELECT_CLASS}
      >
        {UNITS_DESC.slice()
          .reverse()
          .map((option) => (
            <option key={option} value={option}>
              {unitLabel(option)}
            </option>
          ))}
      </select>
    </div>
  );
}

// ---- 1. Durum satırı ----

function StatusPanel({ view, saver }: { view: Fail2banView; saver: Saver }) {
  const bannedNow = view.jails.reduce((sum, jail) => sum + jail.banned.length, 0);
  const bannedTotal = view.jails.reduce((sum, jail) => sum + jail.banned_total, 0);
  const protectionOn = view.running && view.jails.some((jail) => jail.active);

  const yourIp = view.your_ip;
  const whitelist = cleanWhitelist(view.settings.whitelist);
  const safe = yourIp !== "" && whitelist.includes(yourIp);

  function addYourIp() {
    saver.run(
      { ...view.settings, whitelist: [...whitelist, yourIp] },
      { scope: "self", message: t("Adresiniz güvenli listeye eklendi") },
    );
  }

  return (
    <div className="flex flex-col gap-2">
      <Panel>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Status tone={protectionOn ? "ok" : "warn"}>{protectionOn ? t("Koruma açık") : t("Koruma kapalı")}</Status>
          <p className="text-ink-2">{t("Şu an {n} adres engelli. Bugüne kadar {total} adres engellendi.", { n: bannedNow, total: bannedTotal })}</p>
        </div>
      </Panel>

      {yourIp !== "" &&
        (safe ? (
          <p className="px-1 text-[13.5px] text-ink-3">
            <SentenceWithIp template={t("Adresiniz ({ip}) güvenli listede; engellenmezsiniz.")} ip={yourIp} />
          </p>
        ) : (
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg bg-accent-soft px-4 sm:px-6 py-3">
            <p className="min-w-0 flex-1 text-ink">
              <SentenceWithIp template={t("Kendi adresiniz ({ip}) güvenli listede değil. Parolanızı birkaç kez yanlış girerseniz siz de engellenebilirsiniz.")} ip={yourIp} />
            </p>
            <Button variant="primary" size="sm" loading={saver.pending && saver.scope === "self"} disabled={saver.pending} onClick={addYourIp}>
              {t("Adresimi güvenli listeye ekle")}
            </Button>
          </div>
        ))}
    </div>
  );
}

// ---- 2. Engellenen adresler ----

function BannedPanel({ view, onUnban, unbanning }: { view: Fail2banView; onUnban: (request: UnbanRequest) => void; unbanning: UnbanRequest | null }) {
  const rows = useMemo(() => flattenBanned(view.jails), [view.jails]);

  return (
    <Panel title={t("Engellenen adresler")} flush>
      {rows.length === 0 ? (
        <p className="px-4 sm:px-6 py-4 text-ink-2">{t("Şu an engelli adres yok.")}</p>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{t("Adres")}</Th>
              <Th>{t("Nereye saldırdı")}</Th>
              <Th align="right">
                <span className="sr-only">{t("İşlemler")}</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={`${row.jailKey}|${row.ip}`}>
                <Td>
                  <Mono>{row.ip}</Mono>
                </Td>
                <Td>{row.jailLabel}</Td>
                <Td align="right">
                  <Button
                    size="sm"
                    loading={unbanning !== null && unbanning.ip === row.ip && unbanning.jail === row.jailKey}
                    disabled={unbanning !== null}
                    onClick={() => onUnban({ ip: row.ip, jail: row.jailKey })}
                  >
                    {t("Engeli kaldır")}
                  </Button>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </Panel>
  );
}

// ---- 3. Ne kadar sıkı olsun? ----

function StrictnessPanel({ view, saver }: { view: Fail2banView; saver: Saver }) {
  const current = detectLevel(view);
  const settings = view.settings;

  function choose(level: Level) {
    const next = allJailKeys(view).reduce((acc, key) => withJail(acc, key, level.numbers), settings);
    saver.run(next, { scope: "level" });
  }

  const cardBase = "flex flex-col gap-1 rounded-lg border p-3 transition-colors";

  return (
    <Panel title={t("Ne kadar sıkı olsun?")} actions={<SavingSpinner saver={saver} scope="level" />}>
      <div className="flex flex-col gap-5">
        <fieldset disabled={saver.pending} className="min-w-0">
          <legend className="sr-only">{t("Koruma seviyesi")}</legend>
          <div className={classNames("grid gap-2", current === "custom" ? "sm:grid-cols-2 lg:grid-cols-4" : "sm:grid-cols-3")}>
            {LEVELS.map((level) => (
              <label
                key={level.id}
                className={classNames(
                  cardBase,
                  saver.pending ? "cursor-not-allowed opacity-70" : "cursor-pointer",
                  current === level.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                )}
              >
                <span className="flex items-center gap-2 font-medium">
                  <input type="radio" name="strictness" className="accent-(--accent)" checked={current === level.id} onChange={() => choose(level)} />
                  {t(level.title)}
                  {level.recommended && <Badge tone="accent">{t("önerilen")}</Badge>}
                </span>
                <span className="text-[13px] text-ink-2">{t(level.description)}</span>
              </label>
            ))}
            {current === "custom" && (
              <div aria-current="true" className={classNames(cardBase, "border-accent bg-accent-soft")}>
                <span className="font-medium">{t("Özel")}</span>
                <span className="text-[13px] text-ink-2">{t("Ayarlar koruma başına ayrı ayrı belirlenmiş")}</span>
              </div>
            )}
          </div>
        </fieldset>

        <Checkbox
          label={t("Tekrar tekrar deneyenleri her seferinde daha uzun süre engelle")}
          checked={settings.escalate}
          disabled={saver.pending}
          onChange={(event) => saver.run({ ...settings, escalate: event.target.checked }, { scope: "level" })}
        />
      </div>
    </Panel>
  );
}

// ---- 4. Güvenli liste ----

function WhitelistPanel({ view, saver }: { view: Fail2banView; saver: Saver }) {
  const settings = view.settings;
  const entries = cleanWhitelist(settings.whitelist);

  // Yazılan metin yerel tutulur; 10 saniyelik yoklama yazmayı bozmaz.
  const [ip, setIp] = useState("");
  const [error, setError] = useState<string | null>(null);

  function add(event: FormEvent) {
    event.preventDefault();
    const value = ip.trim();
    if (value === "") return;
    if (entries.includes(value)) {
      setError(t("Bu adres zaten listede."));
      return;
    }
    setError(null);
    saver.run(
      { ...settings, whitelist: [...entries, value] },
      {
        scope: "whitelist",
        message: t("Adres güvenli listeye eklendi"),
        onSuccess: () => setIp(""),
        onError: (failure) => setError(errorMessage(failure)),
      },
    );
  }

  function remove(entry: string) {
    saver.run({ ...settings, whitelist: entries.filter((line) => line !== entry) }, { scope: "whitelist", message: t("Adres listeden çıkarıldı") });
  }

  return (
    <Panel title={t("Güvenli liste")} actions={<SavingSpinner saver={saver} scope="whitelist" />}>
      <div className="flex max-w-xl flex-col gap-3">
        <p className="text-ink-2">{t("Bu adresler hiçbir zaman engellenmez. Ofisinizin ya da evinizin sabit adresini ekleyin.")}</p>

        {entries.length > 0 && (
          <ul className="flex flex-col rounded-md border border-line">
            {entries.map((entry) => (
              <li key={entry} className="flex items-center justify-between gap-2 border-b border-line py-0.5 pr-1 pl-3 last:border-b-0">
                <Mono className="text-[13.5px]">{entry}</Mono>
                <IconButton label={t("{entry} adresini listeden çıkar", { entry })} disabled={saver.pending} onClick={() => remove(entry)}>
                  <X className="size-4" />
                </IconButton>
              </li>
            ))}
          </ul>
        )}

        <form onSubmit={add} className="flex flex-col gap-1.5">
          <div className="flex flex-wrap items-center gap-2">
            <input
              aria-label={t("Güvenli listeye eklenecek adres")}
              aria-invalid={error ? true : undefined}
              placeholder={t("203.0.113.5 ya da 203.0.113.0/24")}
              value={ip}
              onChange={(event) => {
                setIp(event.target.value);
                setError(null);
              }}
              spellCheck={false}
              autoComplete="off"
              disabled={saver.pending}
              className={classNames(TEXT_INPUT_CLASS, "w-full max-w-72 flex-1", error && "border-bad")}
            />
            <Button type="submit" loading={saver.pending && saver.scope === "whitelist" && ip.trim() !== ""} disabled={saver.pending || ip.trim() === ""}>
              {t("Ekle")}
            </Button>
          </div>
          {error && (
            <p role="alert" className="text-[13px] text-bad">
              {error}
            </p>
          )}
        </form>
      </div>
    </Panel>
  );
}

// ---- 5. Neler korunuyor? ----

function ProtectionsPanel({ view, saver }: { view: Fail2banView; saver: Saver }) {
  const available = view.jails.filter((jail) => jail.available);
  const missing = view.jails.filter((jail) => !jail.available);

  function toggle(key: string, enabled: boolean) {
    saver.run(withJail(view.settings, key, { enabled }), { scope: "jails" });
  }

  return (
    <Panel title={t("Neler korunuyor?")} actions={<SavingSpinner saver={saver} scope="jails" />} flush>
      <ul>
        {available.map((jail) => (
          <li key={jail.key} className="flex items-center gap-3 border-b border-line px-4 sm:px-6 py-3 last:border-b-0">
            <Switch checked={jailSettingsOf(view.settings, jail.key).enabled} label={jail.label} disabled={saver.pending} onChange={(next) => toggle(jail.key, next)} />
            <div className="min-w-0 flex-1">
              <div className="font-medium">{jail.label}</div>
              <div className="text-[13.5px] text-ink-2">{jail.description}</div>
            </div>
            <span className="text-[13px] whitespace-nowrap text-ink-3 tabular-nums">{t("{n} engelleme", { n: jail.banned_total })}</span>
          </li>
        ))}
        {missing.map((jail) => (
          <li key={jail.key} className="flex items-center gap-3 border-b border-line px-4 sm:px-6 py-3 text-ink-3 last:border-b-0">
            <span aria-hidden className="w-9 shrink-0" />
            <div className="min-w-0 flex-1">{jail.label}</div>
            <span className="text-[13px] whitespace-nowrap">{t("kurulu değil")}</span>
          </li>
        ))}
      </ul>
    </Panel>
  );
}

// ---- 6. Gelişmiş ----

function AdvancedNumbersPanel({ view, saver }: { view: Fail2banView; saver: Saver }) {
  const jails = view.jails.filter((jail) => jail.available);

  // Yerel düzenlemeler; yoklama bunlara dokunmadığı için yazılanlar ezilmez.
  const [edits, setEdits] = useState<Record<string, JailNumbers>>({});
  const [error, setError] = useState<string | null>(null);

  const numbersOf = (key: string): JailNumbers => edits[key] ?? jailSettingsOf(view.settings, key);

  const changedKeys = jails.map((jail) => jail.key).filter((key) => key in edits && !sameNumbers(edits[key], jailSettingsOf(view.settings, key)));
  const dirty = changedKeys.length > 0;
  const valid = changedKeys.every((key) => validNumbers(edits[key]));

  function patch(key: string, changes: Partial<JailNumbers>) {
    setError(null);
    setEdits((current) => ({ ...current, [key]: { ...(current[key] ?? jailSettingsOf(view.settings, key)), ...changes } }));
  }

  function submit() {
    const next = changedKeys.reduce((acc, key) => withJail(acc, key, edits[key]), view.settings);
    setError(null);
    saver.run(next, {
      scope: "advanced",
      onSuccess: () => setEdits({}),
      onError: (failure) => setError(errorMessage(failure)),
    });
  }

  return (
    <Panel title={t("Koruma başına ayarlar")} flush>
      <Table>
        <thead>
          <tr>
            <Th>{t("Koruma")}</Th>
            <Th>{t("Deneme sayısı")}</Th>
            <Th>{t("Sayım süresi")}</Th>
            <Th>{t("Engel süresi")}</Th>
          </tr>
        </thead>
        <tbody>
          {jails.map((jail) => {
            const numbers = numbersOf(jail.key);
            return (
              <tr key={jail.key}>
                <Td className="font-medium">{jail.label}</Td>
                <Td>
                  <CountInput value={numbers.max_retry} label={t("Deneme sayısı: {label}", { label: jail.label })} disabled={saver.pending} onChange={(next) => patch(jail.key, { max_retry: next })} />
                </Td>
                <Td>
                  <DurationInput seconds={numbers.find_time} label={t("Sayım süresi: {label}", { label: jail.label })} disabled={saver.pending} onChange={(next) => patch(jail.key, { find_time: next })} />
                </Td>
                <Td>
                  <DurationInput seconds={numbers.ban_time} label={t("Engel süresi: {label}", { label: jail.label })} disabled={saver.pending} onChange={(next) => patch(jail.key, { ban_time: next })} />
                </Td>
              </tr>
            );
          })}
        </tbody>
      </Table>
      <div className="flex flex-col gap-2 border-t border-line px-4 sm:px-6 py-3">
        <p className="text-[13px] text-ink-3">{t("Bir adres, sayım süresi içinde belirtilen sayıda hatalı deneme yaparsa engel süresi boyunca engellenir.")}</p>
        {dirty && !valid && <p className="text-[13px] text-bad">{t("Sayılar sıfırdan büyük olmalıdır.")}</p>}
        {error && (
          <p role="alert" className="text-[13px] text-bad">
            {error}
          </p>
        )}
        <div>
          <Button variant="primary" loading={saver.pending && saver.scope === "advanced"} disabled={saver.pending || !dirty || !valid} onClick={submit}>
            {t("Kaydet")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

function ManualBanPanel({ jails, onSubmit, busy, error }: { jails: JailStatus[]; onSubmit: (request: BanRequest, done: () => void) => void; busy: boolean; error: string | null }) {
  const options = jails.filter((jail) => jail.active);
  const [ip, setIp] = useState("");
  const [jailKey, setJailKey] = useState("");
  const selected = options.some((jail) => jail.key === jailKey) ? jailKey : (options[0]?.key ?? "");

  function submit(event: FormEvent) {
    event.preventDefault();
    const trimmed = ip.trim();
    if (trimmed === "" || selected === "") return;
    onSubmit({ ip: trimmed, jail: selected }, () => setIp(""));
  }

  return (
    <Panel title={t("Bir adresi elle engelle")}>
      <form onSubmit={submit} className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2">
          <input
            aria-label={t("Engellenecek adres")}
            placeholder={t("203.0.113.5")}
            value={ip}
            onChange={(event) => setIp(event.target.value)}
            spellCheck={false}
            autoComplete="off"
            disabled={options.length === 0}
            className={classNames(TEXT_INPUT_CLASS, "w-48")}
          />
          <select
            aria-label={t("Hangi korumada engellensin")}
            value={selected}
            onChange={(event) => setJailKey(event.target.value)}
            disabled={options.length === 0}
            className={classNames(SELECT_CLASS, "h-9")}
          >
            {options.map((jail) => (
              <option key={jail.key} value={jail.key}>
                {jail.label}
              </option>
            ))}
          </select>
          <Button type="submit" loading={busy} disabled={ip.trim() === "" || selected === ""}>
            {t("Engelle")}
          </Button>
        </div>
        {options.length === 0 && <p className="text-[13px] text-ink-3">{t("Açık bir koruma olmadığı için elle engellenemez.")}</p>}
        {error && (
          <p role="alert" className="text-[13px] text-bad">
            {error}
          </p>
        )}
      </form>
    </Panel>
  );
}

function AdvancedSection({
  view,
  saver,
  onBan,
  banning,
  banError,
}: {
  view: Fail2banView;
  saver: Saver;
  onBan: (request: BanRequest, done: () => void) => void;
  banning: boolean;
  banError: string | null;
}) {
  const [open, setOpen] = useState(false);
  const Chevron = open ? ChevronDown : ChevronRight;

  return (
    <div className="flex flex-col gap-5">
      <div>
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((current) => !current)}
          className="inline-flex items-center gap-1.5 rounded-md py-1 font-medium text-ink-2 hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
        >
          <Chevron aria-hidden className="size-4" />
          {open ? t("Gelişmiş ayarları gizle") : t("Gelişmiş ayarları göster")}
        </button>
      </div>
      {open && (
        <>
          <AdvancedNumbersPanel view={view} saver={saver} />
          <ManualBanPanel jails={view.jails} onSubmit={onBan} busy={banning} error={banError} />
        </>
      )}
    </div>
  );
}

// ---- Sayfa içeriği ----

function Fail2banContent({ view }: { view: Fail2banView }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  const store = (next: Fail2banView) => queryClient.setQueryData(QUERY_KEY, next);

  const [scope, setScope] = useState<SaveScope | null>(null);

  const save = useMutation({
    mutationFn: (settings: Settings) => api.put<Fail2banView>("/fail2ban", { settings: normalizeSettings(settings) }),
    onSuccess: store,
  });

  const saver: Saver = {
    pending: save.isPending,
    scope: save.isPending ? scope : null,
    run: (next, options) => {
      setScope(options.scope);
      save.mutate(next, {
        onSuccess: () => {
          feedback.success(options.message ?? t("Kaydedildi"));
          options.onSuccess?.();
        },
        onError: (error) => (options.onError ? options.onError(error) : feedback.error(error)),
      });
    },
  };

  const unban = useMutation({
    mutationFn: (request: UnbanRequest) => api.post<Fail2banView>("/fail2ban/unban", request),
    onSuccess: (next) => {
      store(next);
      feedback.success(t("Engel kaldırıldı"));
    },
    onError: (error) => feedback.error(error),
  });

  const ban = useMutation({
    mutationFn: (request: BanRequest) => api.post<Fail2banView>("/fail2ban/ban", request),
    onSuccess: (next) => {
      store(next);
      feedback.success(t("Adres engellendi"));
    },
  });

  return (
    <>
      {!view.running && (
        <div role="alert" className="mb-4 rounded-lg border border-warn/50 bg-warn-soft px-4 sm:px-6 py-3 text-warn">
          {t("Saldırı engelleme şu an çalışmıyor; adresler engellenmiyor.")}{" "}
          <Link to="/services" className="font-medium underline">
            {t("Servisler sayfasından başlatın")}
          </Link>
        </div>
      )}

      <div className="flex min-w-0 flex-col gap-4">
        <StatusPanel view={view} saver={saver} />
        <BannedPanel view={view} onUnban={(request) => unban.mutate(request)} unbanning={unban.isPending ? unban.variables : null} />
        <StrictnessPanel view={view} saver={saver} />
        <WhitelistPanel view={view} saver={saver} />
        <ProtectionsPanel view={view} saver={saver} />
        <AdvancedSection
          view={view}
          saver={saver}
          onBan={(request, done) => ban.mutate(request, { onSuccess: done })}
          banning={ban.isPending}
          banError={ban.isError ? errorMessage(ban.error) : null}
        />
        <p className="text-[13px] text-ink-3">{t("Engeller bütün portlar için geçerlidir ve güvenlik duvarından bağımsız çalışır.")}</p>
      </div>
    </>
  );
}

// ---- Sayfa ----

export function Fail2banPage() {
  const query = useQuery({
    queryKey: QUERY_KEY,
    queryFn: () => api.get<Fail2banView>("/fail2ban"),
    refetchInterval: POLL_INTERVAL_MS,
  });

  const view = query.data;
  let body;
  if (!view) {
    body = query.isError ? <ErrorBlock message={errorMessage(query.error)} onRetry={() => void query.refetch()} /> : <LoadingBlock />;
  } else if (!view.installed) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("Saldırı engelleme bu sunucuda kurulu değil")}
          description={t("Kullanabilmek için önce ilgili bileşeni kurmanız gerekir.")}
          action={
            <Link to="/components" className="font-medium text-accent hover:underline">
              {t("Bileşenler sayfasından kurun")}
            </Link>
          }
        />
      </Panel>
    );
  } else {
    body = <Fail2banContent view={view} />;
  }

  return (
    <>
      <PageHeader
        title={t("Saldırı engelleme")}
        description={t("Parolanızı tahmin etmeye çalışan adresler birkaç hatalı denemeden sonra bir süreliğine sunucudan uzak tutulur.")}
      />
      <SecurityTabs />
      {body}
    </>
  );
}
