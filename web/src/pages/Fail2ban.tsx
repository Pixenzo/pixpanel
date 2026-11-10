// Saldırı engelleme sayfası (fail2ban): art arda hatalı parola deneyen adresleri bir süreliğine engeller.
// Ayarlar yerel taslakta düzenlenir; "Kaydet" onay beklemeden doğrudan uygular.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { FormEvent } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { Button, Checkbox, EmptyState, ErrorBlock, LoadingBlock, Mono, PageHeader, Panel, Status, Table, Td, Textarea, Th } from "../components/ui";
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

/** Taslağı, mevcut ayarlardan türetilmiş bir değişiklik işleviyle günceller. */
type UpdateSettings = (change: (current: Settings) => Settings) => void;

const QUERY_KEY = ["fail2ban"] as const;
const POLL_INTERVAL_MS = 10_000;

const UNIT_SECONDS: Record<DurationUnit, number> = { second: 1, minute: 60, hour: 3600, day: 86400 };
/** Büyükten küçüğe; en uygun birim seçilirken bu sırayla denenir. */
const UNITS_DESC: DurationUnit[] = ["day", "hour", "minute", "second"];

const DEFAULT_JAIL: Omit<JailSettings, "key"> = { enabled: false, max_retry: 5, find_time: 600, ban_time: 3600 };

const CELL_CLASS = "h-8 min-w-0 rounded-md border border-line-strong bg-surface px-2 text-[13px] tabular-nums placeholder:text-ink-3 disabled:bg-sunken disabled:text-ink-3";
const SELECT_CLASS = "h-8 rounded-md border border-line-strong bg-surface px-1.5 text-[13px] disabled:bg-sunken disabled:text-ink-3";

// ---- Yardımcılar ----

/** Boşlukları temizler ve boş satırları atar; karşılaştırma ve gönderim bu biçimle yapılır. */
function normalizeSettings(settings: Settings): Settings {
  return {
    jails: settings.jails,
    escalate: settings.escalate,
    whitelist: settings.whitelist.map((line) => line.trim()).filter((line) => line !== ""),
  };
}

function jailSettingsOf(settings: Settings, key: string): JailSettings {
  return settings.jails.find((jail) => jail.key === key) ?? { key, ...DEFAULT_JAIL };
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

/** Ayarlardaki her koruma, fail2ban'dan gelen koruma listesiyle aynı anahtarlara sahiptir. */
function flattenBanned(jails: JailStatus[]): BannedRow[] {
  return jails.flatMap((jail) => jail.banned.map((ip) => ({ ip, jailKey: jail.key, jailLabel: jail.label })));
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

function LinkButton({ children, onClick, disabled }: { children: string; onClick: () => void; disabled?: boolean }) {
  return (
    <button type="button" onClick={onClick} disabled={disabled} className="text-[12.5px] whitespace-nowrap text-accent hover:underline disabled:cursor-not-allowed disabled:opacity-55">
      {children}
    </button>
  );
}

/** Tam sayı girişi; yazarken geçici olarak boş kalabilir, dışarıdan gelen değer değişirse metin yenilenir. */
function CountInput({ value, onChange, label, disabled }: { value: number; onChange: (next: number) => void; label: string; disabled?: boolean }) {
  const [text, setText] = useState(() => String(value));

  // Taslak dışarıdan değiştiyse (Vazgeç gibi) metin yeni değere uyar.
  if (parseAmount(text) !== value) setText(String(value));

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

  // Taslak dışarıdan değiştiyse (Vazgeç gibi) sayı ve birim yeni değere göre yeniden seçilir.
  if (Math.round(parseAmount(text) * UNIT_SECONDS[unit]) !== seconds) {
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

// ---- Engelli adresler ----

function BanForm({ jails, onSubmit, busy, error }: { jails: JailStatus[]; onSubmit: (request: BanRequest) => void; busy: boolean; error: string | null }) {
  const options = jails.filter((jail) => jail.active);
  const [ip, setIp] = useState("");
  const [jailKey, setJailKey] = useState("");
  const selected = options.some((jail) => jail.key === jailKey) ? jailKey : (options[0]?.key ?? "");

  function submit(event: FormEvent) {
    event.preventDefault();
    const trimmed = ip.trim();
    if (trimmed === "" || selected === "") return;
    onSubmit({ ip: trimmed, jail: selected });
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-2 border-t border-line px-4 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <span className="text-[13px] font-medium text-ink-2">{t("Adres engelle")}</span>
        <input
          aria-label={t("Engellenecek adres")}
          placeholder={t("203.0.113.5")}
          value={ip}
          onChange={(event) => setIp(event.target.value)}
          className={classNames(CELL_CLASS, "h-9 w-48 font-mono text-[13px]")}
          spellCheck={false}
          autoComplete="off"
          disabled={options.length === 0}
        />
        <select
          aria-label={t("Hangi korumada engellensin")}
          value={selected}
          onChange={(event) => setJailKey(event.target.value)}
          className={classNames(SELECT_CLASS, "h-9")}
          disabled={options.length === 0}
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
      {error && (
        <p role="alert" className="text-[12.5px] text-bad">
          {error}
        </p>
      )}
    </form>
  );
}

function BannedPanel({
  view,
  draftWhitelist,
  onAddToWhitelist,
  onUnban,
  unbanning,
  onBan,
  banning,
  banError,
}: {
  view: Fail2banView;
  draftWhitelist: string[];
  onAddToWhitelist: (ip: string) => void;
  onUnban: (request: UnbanRequest) => void;
  unbanning: UnbanRequest | null;
  onBan: (request: BanRequest) => void;
  banning: boolean;
  banError: string | null;
}) {
  const rows = useMemo(() => flattenBanned(view.jails), [view.jails]);
  const yourIp = view.your_ip;
  const whitelisted = draftWhitelist.some((line) => line.trim() === yourIp);

  return (
    <Panel title={t("Engelli adresler")} actions={rows.length > 0 ? <span className="text-[12.5px] text-ink-3 tabular-nums">{t("{n} kayıt", { n: rows.length })}</span> : undefined} flush>
      {rows.length === 0 ? (
        <p className="px-4 py-4 text-ink-2">{t("Şu an engelli adres yok.")}</p>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{t("Adres")}</Th>
              <Th>{t("Koruma")}</Th>
              <Th align="right">
                <span className="sr-only">{t("İşlemler")}</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={`${row.jailKey}|${row.ip}`}>
                <Td>
                  <Mono className="text-[13px]">{row.ip}</Mono>
                </Td>
                <Td>{row.jailLabel}</Td>
                <Td align="right">
                  <Button
                    size="sm"
                    loading={unbanning !== null && unbanning.ip === row.ip && unbanning.jail === row.jailKey}
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
      <BanForm jails={view.jails} onSubmit={onBan} busy={banning} error={banError} />
      {yourIp !== "" && (
        <p className="flex flex-wrap items-center gap-x-2 gap-y-1 border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">
          <span>
            {t("Sizin adresiniz:")} <Mono>{yourIp}</Mono> {t("— kendinizi yanlışlıkla engellememek için beyaz listeye ekleyebilirsiniz")}
          </span>
          {whitelisted ? <span className="text-ok">{t("Beyaz listede")}</span> : <LinkButton onClick={() => onAddToWhitelist(yourIp)}>{t("Beyaz listeye ekle")}</LinkButton>}
        </p>
      )}
    </Panel>
  );
}

// ---- Korumalar ----

function JailStateLabel({ jail }: { jail: JailStatus }) {
  if (!jail.available) return <Status tone="idle">{t("Kurulu değil")}</Status>;
  if (jail.active) return <Status tone="ok">{t("İzliyor")}</Status>;
  return <Status tone="idle">{t("Kapalı")}</Status>;
}

function JailsPanel({ view, draft, onUpdate }: { view: Fail2banView; draft: Settings; onUpdate: UpdateSettings }) {
  const patch = (key: string, changes: Partial<JailSettings>) =>
    onUpdate((current) => {
      const exists = current.jails.some((jail) => jail.key === key);
      const jails = exists
        ? current.jails.map((jail) => (jail.key === key ? { ...jail, ...changes } : jail))
        : [...current.jails, { ...jailSettingsOf(current, key), ...changes }];
      return { ...current, jails };
    });

  return (
    <Panel title={t("Korumalar")} flush>
      <Table>
        <thead>
          <tr>
            <Th>
              <span className="sr-only">{t("Açık")}</span>
            </Th>
            <Th>{t("Koruma")}</Th>
            <Th>{t("Durum")}</Th>
            <Th align="right">{t("Şu an engelli")}</Th>
            <Th align="right">{t("Toplam engel")}</Th>
            <Th align="right">{t("Hatalı deneme")}</Th>
            <Th>{t("Deneme")}</Th>
            <Th>{t("Sayım süresi")}</Th>
            <Th>{t("Engel süresi")}</Th>
          </tr>
        </thead>
        <tbody>
          {view.jails.map((jail) => {
            const state = jailSettingsOf(draft, jail.key);
            const unavailable = !jail.available;
            return (
              <tr key={jail.key}>
                <Td className="w-14">
                  <Switch
                    checked={state.enabled && !unavailable}
                    disabled={unavailable}
                    label={unavailable ? t("{label} (kurulu değil)", { label: jail.label }) : jail.label}
                    onChange={(next) => patch(jail.key, { enabled: next })}
                  />
                </Td>
                <Td>
                  <div className="font-medium">{jail.label}</div>
                  <div className="max-w-[44ch] text-[13px] text-ink-2">{jail.description}</div>
                </Td>
                <Td>
                  <JailStateLabel jail={jail} />
                </Td>
                <Td align="right" className="tabular-nums">
                  {jail.banned.length}
                </Td>
                <Td align="right" className="tabular-nums">
                  {jail.banned_total}
                </Td>
                <Td align="right" className="tabular-nums">
                  {jail.failed_total}
                </Td>
                <Td>
                  <CountInput value={state.max_retry} label={t("Deneme: {label}", { label: jail.label })} disabled={unavailable} onChange={(next) => patch(jail.key, { max_retry: next })} />
                </Td>
                <Td>
                  <DurationInput seconds={state.find_time} label={t("Sayım süresi: {label}", { label: jail.label })} disabled={unavailable} onChange={(next) => patch(jail.key, { find_time: next })} />
                </Td>
                <Td>
                  <DurationInput seconds={state.ban_time} label={t("Engel süresi: {label}", { label: jail.label })} disabled={unavailable} onChange={(next) => patch(jail.key, { ban_time: next })} />
                </Td>
              </tr>
            );
          })}
        </tbody>
      </Table>
      <p className="border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">
        {t("Bir adres, sayım süresi içinde belirtilen sayıda hatalı deneme yaparsa engel süresi boyunca engellenir.")}
      </p>
    </Panel>
  );
}

// ---- Genel ayarlar ----

function GeneralPanel({ draft, onUpdate }: { draft: Settings; onUpdate: UpdateSettings }) {
  return (
    <Panel title={t("Genel ayarlar")}>
      <div className="flex max-w-xl flex-col gap-5">
        <Checkbox
          label={t("Tekrar engellenen adreslerin süresi her seferinde uzasın")}
          hint={t("Sürekli deneyen adresler giderek daha uzun süre engellenir, en çok 30 gün.")}
          checked={draft.escalate}
          onChange={(event) => onUpdate((current) => ({ ...current, escalate: event.target.checked }))}
        />
        <Textarea
          label={t("Beyaz liste")}
          hint={t("Her satıra bir adres ya da ağ yazın. Bu adresler hiçbir zaman engellenmez; kendi ofis ya da ev adresinizi ekleyin. Örnek: 203.0.113.0/24")}
          value={draft.whitelist.join("\n")}
          onChange={(event) => onUpdate((current) => ({ ...current, whitelist: event.target.value.split("\n") }))}
          spellCheck={false}
          autoComplete="off"
          rows={4}
        />
      </div>
    </Panel>
  );
}

// ---- Sayfa içeriği ----

function Fail2banContent({ view }: { view: Fail2banView }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  // Kullanıcının düzenlediği taslak; null ise sunucudaki ayarlar gösterilir.
  // Yoklama bu değere dokunmadığı için kaydedilmemiş değişiklikler ezilmez.
  const [edits, setEdits] = useState<Settings | null>(null);
  const base = view.settings;
  const draft = edits ?? base;
  const dirty = edits !== null && JSON.stringify(normalizeSettings(edits)) !== JSON.stringify(normalizeSettings(base));

  const update: UpdateSettings = (change) => setEdits((current) => change(current ?? base));

  const store = (next: Fail2banView) => queryClient.setQueryData(QUERY_KEY, next);

  const save = useMutation({
    mutationFn: (settings: Settings) => api.put<Fail2banView>("/fail2ban", { settings: normalizeSettings(settings) }),
    onSuccess: (next) => {
      store(next);
      setEdits(null);
      feedback.success(t("Ayarlar kaydedildi"));
    },
  });

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

  function discard() {
    setEdits(null);
    save.reset();
  }

  const addToWhitelist = (ip: string) =>
    update((current) => (current.whitelist.some((line) => line.trim() === ip) ? current : { ...current, whitelist: [...current.whitelist.filter((line) => line.trim() !== ""), ip] }));

  return (
    <>
      {!view.running && (
        <div role="alert" className="mb-4 rounded-lg border border-warn/50 bg-warn-soft px-4 py-3 text-warn">
          {t("fail2ban şu an çalışmıyor; adresler engellenmiyor.")}{" "}
          <Link to="/services" className="font-medium underline">
            {t("Servisler sayfasından başlatın")}
          </Link>
        </div>
      )}

      {dirty && (
        <div className="sticky top-0 z-10 mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg border border-line-strong bg-surface px-4 py-2.5 shadow-pop">
          <p className="font-medium">{t("Kaydedilmemiş değişiklikler")}</p>
          <div className="flex items-center gap-2">
            <Button onClick={discard} disabled={save.isPending}>
              {t("Vazgeç")}
            </Button>
            <Button variant="primary" loading={save.isPending} onClick={() => save.mutate(draft)}>
              {t("Kaydet")}
            </Button>
          </div>
        </div>
      )}

      {save.isError && (
        <div className="mb-4">
          <ErrorBlock message={errorMessage(save.error)} />
        </div>
      )}

      <div className="flex min-w-0 flex-col gap-4">
        <BannedPanel
          view={view}
          draftWhitelist={draft.whitelist}
          onAddToWhitelist={addToWhitelist}
          onUnban={(request) => unban.mutate(request)}
          unbanning={unban.isPending ? unban.variables : null}
          onBan={(request) => ban.mutate(request)}
          banning={ban.isPending}
          banError={ban.isError ? errorMessage(ban.error) : null}
        />
        <JailsPanel view={view} draft={draft} onUpdate={update} />
        <GeneralPanel draft={draft} onUpdate={update} />
        <p className="text-[12.5px] text-ink-3">{t("Engeller bütün portlar için geçerlidir ve güvenlik duvarı kurallarından bağımsız çalışır.")}</p>
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
          title={t("fail2ban bu sunucuda kurulu değil")}
          description={t("Saldırı engellemeyi kullanabilmek için fail2ban bileşeninin kurulu olması gerekir.")}
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
        description={t("Art arda hatalı parola deneyen adresleri bir süreliğine bütün portlardan engeller (fail2ban).")}
      />
      {body}
    </>
  );
}
