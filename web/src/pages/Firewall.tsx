// Güvenlik duvarı sayfası. Tek soruya yanıt verir: sunucunun hangi kapısı kime açık?
// Değişiklikler toplu uygulanır; süre dolmadan onaylanmazsa sunucu önceki kurallara kendiliğinden döner.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, ChevronDown, ChevronRight, Plus, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { useFeedback } from "../components/feedback";
import { SecurityTabs } from "../components/SecurityTabs";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, PageHeader, Panel, Select, Status, Table, Td, Th } from "../components/ui";
import type { Tone } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Türler ----

type RuleProtocol = "tcp" | "udp" | "both";
type RuleAction = "accept" | "drop";

interface Rule {
  label: string;
  protocol: RuleProtocol;
  ports: string;
  /** Boş = her adres. */
  source: string;
  action: RuleAction;
}

interface PresetState {
  key: string;
  enabled: boolean;
  /** Boş = her adres. */
  source: string;
}

interface Settings {
  enabled: boolean;
  presets: PresetState[];
  rules: Rule[];
  blocked: string[];
}

interface Preset {
  key: string;
  label: string;
  description: string;
  tcp: string[];
  udp: string[];
  critical: boolean;
}

interface Listener {
  protocol: "tcp" | "udp";
  port: number;
  address: string;
  process: string;
  local_only: boolean;
  verdict: "open" | "restricted" | "closed";
}

interface FirewallView {
  available: boolean;
  /** Onaylanmış, kalıcı ayarlar. */
  settings: Settings;
  presets: Preset[];
  pending: { settings: Settings; seconds_left: number } | null;
  confirm_seconds: number;
  your_ip: string;
  listeners: Listener[];
}

/** Bir kapının kime açık olduğu. */
type Access = "everyone" | "restricted" | "closed";

/** Taslağı, mevcut ayarlardan türetilmiş bir değişiklik işleviyle günceller. */
type UpdateSettings = (change: (current: Settings) => Settings) => void;

const QUERY_KEY = ["firewall"] as const;
const POLL_INTERVAL_MS = 5000;

const CELL_CLASS = "h-9 min-w-0 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13px] placeholder:font-sans placeholder:text-ink-3 disabled:bg-sunken disabled:text-ink-3";
const SELECT_CLASS = "h-9 rounded-md border border-line-strong bg-surface px-2 text-[13px] disabled:bg-sunken disabled:text-ink-3";

const EMPTY_RULE: Rule = { label: "", protocol: "tcp", ports: "", source: "", action: "accept" };

// ---- Yardımcılar ----

/** Boşlukları temizler ve boş satırları atar; karşılaştırma ve gönderim bu biçimle yapılır. */
function normalizeSettings(settings: Settings): Settings {
  return {
    enabled: settings.enabled,
    presets: settings.presets.map((preset) => ({ ...preset, source: preset.source.trim() })),
    rules: settings.rules.map((rule) => ({ ...rule, label: rule.label.trim(), ports: rule.ports.trim(), source: rule.source.trim() })),
    blocked: settings.blocked.map((line) => line.trim()).filter((line) => line !== ""),
  };
}

function parseProtocol(value: string): RuleProtocol {
  return value === "udp" ? "udp" : value === "both" ? "both" : "tcp";
}

function parseAction(value: string): RuleAction {
  return value === "drop" ? "drop" : "accept";
}

function stateOf(settings: Settings, key: string): PresetState {
  return settings.presets.find((state) => state.key === key) ?? { key, enabled: false, source: "" };
}

/** `80,443,3000-3010` biçimindeki listenin verilen portu kapsayıp kapsamadığı. */
function coversPort(ports: string, port: number): boolean {
  return ports.split(",").some((part) => {
    const [from, to] = part.split("-").map((piece) => Number(piece.trim()));
    if (from === undefined || !Number.isInteger(from)) return false;
    return to === undefined ? from === port : from <= port && port <= to;
  });
}

/** Dinleyen portun, taslaktaki açık bir servis ya da izin kuralıyla zaten listede olup olmadığı. */
function isListed(listener: Listener, settings: Settings, presets: Preset[]): boolean {
  const inPreset = presets.some((preset) => (listener.protocol === "tcp" ? preset.tcp : preset.udp).some((ports) => coversPort(ports, listener.port)));
  if (inPreset) return true;
  return settings.rules.some((rule) => rule.action === "accept" && (rule.protocol === "both" || rule.protocol === listener.protocol) && coversPort(rule.ports, listener.port));
}

function presetPorts(preset: Preset): string {
  const tcp = preset.tcp.join(", ");
  const udp = preset.udp.join(", ");
  if (tcp !== "" && udp !== "") return tcp === udp ? tcp : `${tcp} · UDP ${udp}`;
  return tcp !== "" ? tcp : udp !== "" ? `UDP ${udp}` : "";
}

function rulePorts(rule: Rule): string {
  return rule.protocol === "udp" ? `UDP ${rule.ports}` : rule.protocol === "both" ? `${rule.ports} · TCP + UDP` : rule.ports;
}

function disableWarning(preset: Preset): string {
  if (preset.key === "ssh") return t("SSH kapatılırsa sunucuya uzaktan bağlanamazsınız.");
  if (preset.key === "panel") return t("Panel kapatılırsa bu panele erişemezsiniz.");
  return t("Bu servis kritik bir servistir; kapatılırsa ona dışarıdan erişilemez.");
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

const ACCESS_OPTIONS: { value: Access; label: string }[] = [
  { value: "everyone", label: "Herkes" },
  { value: "restricted", label: "Tek adres" },
  { value: "closed", label: "Kapalı" },
];

/** Bir kapının kime açık olacağını seçtiren üçlü düğme. */
function AccessPicker({ value, onChange, label }: { value: Access; onChange: (next: Access) => void; label: string }) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex shrink-0 rounded-md border border-line-strong bg-sunken p-0.5">
      {ACCESS_OPTIONS.map((option) => {
        const selected = option.value === value;
        return (
          <button
            key={option.value}
            type="button"
            role="radio"
            aria-checked={selected}
            onClick={() => !selected && onChange(option.value)}
            className={classNames(
              "h-7 rounded-[5px] px-2.5 text-[13px] font-medium whitespace-nowrap transition-colors disabled:cursor-not-allowed",
              "focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-accent",
              !selected && "text-ink-2 hover:text-ink",
              selected && option.value === "everyone" && "bg-surface text-ok shadow-sm",
              selected && option.value === "restricted" && "bg-surface text-accent shadow-sm",
              selected && option.value === "closed" && "bg-surface text-ink shadow-sm",
            )}
          >
            {t(option.label)}
          </button>
        );
      })}
    </div>
  );
}

// ---- Kapılar ----

interface Door {
  id: string;
  label: string;
  description: string;
  ports: string;
  access: Access;
  source: string;
  critical: boolean;
  onAccess: (next: Access) => void;
  onSource: (source: string) => void;
}

function DoorRow({ door, yourIp, onSourceBlur }: { door: Door; yourIp: string; onSourceBlur: () => void }) {
  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
      <div className="min-w-0 flex-1 basis-56">
        <div className="flex flex-wrap items-baseline gap-x-2">
          <span className="font-medium">{door.label}</span>
          {door.ports !== "" && <Mono className="text-ink-3">{door.ports}</Mono>}
        </div>
        {door.description !== "" && <div className="text-[13px] text-ink-2">{door.description}</div>}
      </div>
      <div className="flex flex-wrap items-center justify-end gap-2">
        {door.access === "restricted" && (
          <>
            {yourIp !== "" && door.source !== yourIp && (
              <button type="button" onClick={() => door.onSource(yourIp)} className="text-[12.5px] whitespace-nowrap text-accent hover:underline disabled:opacity-55">
                {t("Benim adresim")}
              </button>
            )}
            <input
              aria-label={t("İzin verilen adres: {label}", { label: door.label })}
              placeholder={t("203.0.113.5 ya da ağ")}
              value={door.source}
              onChange={(event) => door.onSource(event.target.value)}
              onBlur={onSourceBlur}
              className={classNames(CELL_CLASS, "h-8 w-44")}
              spellCheck={false}
              autoComplete="off"
            />
          </>
        )}
        <AccessPicker value={door.access} onChange={door.onAccess} label={t("Kimler erişebilir: {label}", { label: door.label })} />
      </div>
    </li>
  );
}

function DoorsPanel({ view, draft, onUpdate, onAddPort }: { view: FirewallView; draft: Settings; onUpdate: UpdateSettings; onAddPort: () => void }) {
  const feedback = useFeedback();
  // "Tek adres" seçilip adresi henüz yazılmamış kapılar; adres boşken seçim "Herkes"e dönmesin diye tutulur.
  const [awaitingSource, setAwaitingSource] = useState<string[]>([]);
  const dropAwaiting = (id: string) => setAwaitingSource((current) => current.filter((item) => item !== id));

  const patchPreset = (key: string, changes: Partial<PresetState>) =>
    onUpdate((current) => {
      const exists = current.presets.some((state) => state.key === key);
      const presets = exists
        ? current.presets.map((state) => (state.key === key ? { ...state, ...changes } : state))
        : [...current.presets, { ...stateOf(current, key), ...changes }];
      return { ...current, presets };
    });

  const patchRule = (index: number, changes: Partial<Rule>) =>
    onUpdate((current) => ({ ...current, rules: current.rules.map((rule, i) => (i === index ? { ...rule, ...changes } : rule)) }));

  const accessOf = (id: string, enabled: boolean, source: string): Access => (!enabled ? "closed" : source.trim() !== "" || awaitingSource.includes(id) ? "restricted" : "everyone");

  /** "Tek adres"e geçerken kullanıcının kendi adresi önerilir; bilinmiyorsa kutu boş açılır. */
  const startRestricted = (id: string): string => {
    if (view.your_ip === "") setAwaitingSource((current) => [...current.filter((item) => item !== id), id]);
    return view.your_ip;
  };

  const doors: Door[] = [];

  for (const preset of view.presets) {
    const state = stateOf(draft, preset.key);
    const id = `preset:${preset.key}`;
    doors.push({
      id,
      label: preset.label,
      description: preset.description,
      ports: presetPorts(preset),
      access: accessOf(id, state.enabled, state.source),
      source: state.source,
      critical: preset.critical,
      onSource: (source) => patchPreset(preset.key, { source }),
      onAccess: async (next) => {
        if (next === "closed" && preset.critical) {
          const confirmed = await feedback.confirm({
            title: t("“{label}” kapatılsın mı?", { label: preset.label }),
            description: disableWarning(preset),
            confirmLabel: t("Kapat"),
            danger: true,
          });
          if (!confirmed) return;
        }
        dropAwaiting(id);
        if (next === "closed") patchPreset(preset.key, { enabled: false });
        else if (next === "everyone") patchPreset(preset.key, { enabled: true, source: "" });
        else patchPreset(preset.key, { enabled: true, source: state.source.trim() !== "" ? state.source : startRestricted(id) });
      },
    });
  }

  draft.rules.forEach((rule, index) => {
    if (rule.action !== "accept") return;
    const id = `rule:${index}`;
    doors.push({
      id,
      label: rule.label.trim() !== "" ? rule.label : t("Port {ports}", { ports: rule.ports }),
      description: t("Sizin açtığınız port."),
      ports: rule.label.trim() !== "" ? rulePorts(rule) : rule.protocol === "tcp" ? "" : rulePorts(rule),
      access: accessOf(id, true, rule.source),
      source: rule.source,
      critical: false,
      onSource: (source) => patchRule(index, { source }),
      onAccess: (next) => {
        setAwaitingSource((current) => current.filter((item) => !item.startsWith("rule:")));
        if (next === "closed") onUpdate((current) => ({ ...current, rules: current.rules.filter((_, i) => i !== index) }));
        else if (next === "everyone") patchRule(index, { source: "" });
        else patchRule(index, { source: startRestricted(id) });
      },
    });
  });

  // Sunucuda çalışıp dışarıyı dinleyen ama hiçbir satırda yer almayan portlar: kapalı olarak gösterilir.
  const seen = new Set<string>();
  const unlisted = [...view.listeners]
    .sort((a, b) => a.port - b.port || a.protocol.localeCompare(b.protocol))
    .filter((listener) => {
      const key = `${listener.protocol}:${listener.port}`;
      if (listener.local_only || seen.has(key) || isListed(listener, draft, view.presets)) return false;
      seen.add(key);
      return true;
    });

  for (const listener of unlisted) {
    const name = listener.process !== "" ? listener.process : t("Port {ports}", { ports: String(listener.port) });
    doors.push({
      id: `listener:${listener.protocol}:${listener.port}`,
      label: name,
      description: t("Sunucuda çalışıyor ama dışarıya kapalı."),
      ports: listener.protocol === "udp" ? `UDP ${listener.port}` : String(listener.port),
      access: "closed",
      source: "",
      critical: false,
      onSource: () => undefined,
      onAccess: (next) => {
        if (next === "closed") return;
        onUpdate((current) => ({
          ...current,
          rules: [...current.rules, { label: listener.process, protocol: listener.protocol, ports: String(listener.port), source: next === "restricted" ? view.your_ip : "", action: "accept" }],
        }));
        if (next === "restricted" && view.your_ip === "") setAwaitingSource((current) => [...current, `rule:${draft.rules.length}`]);
      },
    });
  }

  const openCount = doors.filter((door) => door.access !== "closed").length;
  const blockRules = draft.rules.filter((rule) => rule.action === "drop").length;

  return (
    <Panel
      title={t("Hangi kapı kime açık?")}
      flush
      actions={
        <Button size="sm" icon={<Plus className="size-4" />} onClick={onAddPort}>
          {t("Port aç")}
        </Button>
      }
    >
      <p className="border-b border-line px-4 py-2.5 text-[13px] text-ink-2">
        {draft.enabled
          ? t("{open} kapı açık. Bu listede “Kapalı” görünen ya da listede hiç olmayan her porta dışarıdan erişim reddedilir.", { open: openCount })
          : t("Güvenlik duvarı kapalıyken bu seçimler uygulanmaz; açtığınızda geçerli olurlar.")}
      </p>
      <ul className="divide-y divide-line">
        {doors.map((door) => (
          <DoorRow key={door.id} door={door} yourIp={view.your_ip} onSourceBlur={() => door.source.trim() === "" && dropAwaiting(door.id)} />
        ))}
      </ul>
      <p className="border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">
        {t("“Tek adres” seçtiğinizde yalnızca yazdığınız adres ya da ağ (örneğin 203.0.113.0/24) bağlanabilir.")}{" "}
        {blockRules > 0 && t("Ayrıca {n} engelleme kuralı var; gelişmiş bölümde görebilirsiniz.", { n: blockRules })}
      </p>
    </Panel>
  );
}

// ---- Port açma penceresi ----

function AddPortModal({ open, yourIp, onClose, onAdd }: { open: boolean; yourIp: string; onClose: () => void; onAdd: (rule: Rule) => void }) {
  const [label, setLabel] = useState("");
  const [ports, setPorts] = useState("");
  const [protocol, setProtocol] = useState<RuleProtocol>("tcp");
  const [restricted, setRestricted] = useState(false);
  const [source, setSource] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    if (!open) return;
    setLabel("");
    setPorts("");
    setProtocol("tcp");
    setRestricted(false);
    setSource("");
    setError("");
  }, [open]);

  function submit(event: FormEvent) {
    event.preventDefault();
    const cleaned = ports.replace(/\s+/g, "");
    const valid = cleaned !== "" && cleaned.split(",").every((part) => /^\d{1,5}(-\d{1,5})?$/.test(part));
    if (!valid) {
      setError(t("Port numarası yazın; örneğin 8080, 3000-3010 ya da 80,443."));
      return;
    }
    if (restricted && source.trim() === "") {
      setError(t("İzin verilecek adresi yazın ya da “Herkes”i seçin."));
      return;
    }
    onAdd({ label: label.trim(), protocol, ports: cleaned, source: restricted ? source.trim() : "", action: "accept" });
    onClose();
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("Port aç")}
      description={t("Kendi kurduğunuz bir uygulamaya dışarıdan erişilebilmesi için.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="add-port-form">
            {t("Listeye ekle")}
          </Button>
        </>
      }
    >
      <form id="add-port-form" onSubmit={submit} className="flex flex-col gap-4">
        <div className="grid grid-cols-[1fr_8.5rem] gap-3">
          <Input label={t("Port")} placeholder="8080" value={ports} onChange={(event) => setPorts(event.target.value)} className="font-mono" autoFocus spellCheck={false} autoComplete="off" />
          <Select label={t("Tür")} value={protocol} onChange={(event) => setProtocol(parseProtocol(event.target.value))}>
            <option value="tcp">TCP</option>
            <option value="udp">UDP</option>
            <option value="both">{t("İkisi de")}</option>
          </Select>
        </div>
        <Input label={t("Ne için? (isteğe bağlı)")} placeholder={t("Örneğin: oyun sunucusu")} value={label} onChange={(event) => setLabel(event.target.value)} autoComplete="off" />
        <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
          <legend className="mb-1.5 text-[13px] font-medium text-ink-2">{t("Kimler bağlanabilsin?")}</legend>
          <label className="flex items-center gap-2.5">
            <input type="radio" name="port-access" className="size-4 accent-(--accent)" checked={!restricted} onChange={() => setRestricted(false)} />
            <span>{t("Herkes")}</span>
          </label>
          <label className="flex items-center gap-2.5">
            <input
              type="radio"
              name="port-access"
              className="size-4 accent-(--accent)"
              checked={restricted}
              onChange={() => {
                setRestricted(true);
                if (source === "") setSource(yourIp);
              }}
            />
            <span>{t("Yalnızca tek adres ya da ağ")}</span>
          </label>
          {restricted && (
            <input
              aria-label={t("İzin verilen adres")}
              placeholder="203.0.113.5"
              value={source}
              onChange={(event) => setSource(event.target.value)}
              className={classNames(CELL_CLASS, "ml-6.5 w-56")}
              spellCheck={false}
              autoComplete="off"
            />
          )}
        </fieldset>
        {error !== "" && <p className="text-[13px] text-bad">{error}</p>}
        <p className="text-[12.5px] text-ink-3">{t("Port, sayfanın üstündeki “Uygula” düğmesine bastığınızda açılır.")}</p>
      </form>
    </Modal>
  );
}

// ---- Engelli adresler ----

function BlockedPanel({ draft, onUpdate }: { draft: Settings; onUpdate: UpdateSettings }) {
  const [value, setValue] = useState("");
  const entries = draft.blocked.filter((entry) => entry.trim() !== "");

  function add(event: FormEvent) {
    event.preventDefault();
    const address = value.trim();
    if (address === "") return;
    onUpdate((current) => (current.blocked.includes(address) ? current : { ...current, blocked: [...current.blocked, address] }));
    setValue("");
  }

  return (
    <Panel title={t("Hiç giremeyecek adresler")}>
      <p className="mb-3 max-w-[75ch] text-ink-2">{t("Buraya eklediğiniz adres ya da ağlardan gelen bütün bağlantılar, kapı açık olsa bile reddedilir.")}</p>
      {entries.length > 0 && (
        <ul className="mb-3 flex flex-wrap gap-2">
          {entries.map((entry) => (
            <li key={entry} className="flex items-center gap-1 rounded-md border border-line bg-sunken py-0.5 pr-0.5 pl-2.5">
              <Mono className="text-[13px]">{entry}</Mono>
              <IconButton label={t("{address} adresinin engelini kaldır", { address: entry })} onClick={() => onUpdate((current) => ({ ...current, blocked: current.blocked.filter((item) => item !== entry) }))}>
                <X className="size-3.5" />
              </IconButton>
            </li>
          ))}
        </ul>
      )}
      <form onSubmit={add} className="flex flex-wrap items-center gap-2">
        <input
          aria-label={t("Engellenecek adres ya da ağ")}
          placeholder={t("203.0.113.5 ya da 198.51.100.0/24")}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          className={classNames(CELL_CLASS, "w-72")}
          spellCheck={false}
          autoComplete="off"
        />
        <Button type="submit" disabled={value.trim() === ""}>
          {t("Engelle")}
        </Button>
      </form>
    </Panel>
  );
}

// ---- Gelişmiş: kural sırası ----

function RulesPanel({ draft, onUpdate }: { draft: Settings; onUpdate: UpdateSettings }) {
  const patch = (index: number, changes: Partial<Rule>) =>
    onUpdate((current) => ({ ...current, rules: current.rules.map((rule, i) => (i === index ? { ...rule, ...changes } : rule)) }));

  const move = (index: number, offset: -1 | 1) =>
    onUpdate((current) => {
      const target = index + offset;
      if (target < 0 || target >= current.rules.length) return current;
      const rules = [...current.rules];
      const [moved] = rules.splice(index, 1);
      if (moved === undefined) return current;
      rules.splice(target, 0, moved);
      return { ...current, rules };
    });

  const remove = (index: number) => onUpdate((current) => ({ ...current, rules: current.rules.filter((_, i) => i !== index) }));
  const add = () => onUpdate((current) => ({ ...current, rules: [...current.rules, { ...EMPTY_RULE, action: "drop" }] }));

  return (
    <Panel title={t("Kurallar ve sıraları")}>
      <p className="mb-4 max-w-[75ch] text-ink-2">
        {t("Açtığınız portlar burada kural olarak görünür. Bir portu belirli bir adrese kapatmak için “Engelle” kuralı ekleyip izin kuralının üstüne taşıyın: kurallar sırayla değerlendirilir, ilk uyan geçerlidir ve hepsi yukarıdaki servislerden önce gelir.")}
      </p>
      <div className="flex flex-col gap-2">
        {draft.rules.length === 0 && <p className="rounded-md bg-sunken/70 px-3 py-2.5 text-[13px] text-ink-2">{t("Henüz kural yok.")}</p>}
        {draft.rules.map((rule, index) => (
          <div key={index} className="flex flex-wrap items-center gap-2 rounded-lg border border-line p-2.5">
            <span aria-hidden className="w-5 text-right text-[12.5px] text-ink-3 tabular-nums">
              {index + 1}
            </span>
            <input
              aria-label={t("Açıklama")}
              placeholder={t("Açıklama")}
              value={rule.label}
              onChange={(event) => patch(index, { label: event.target.value })}
              className={classNames(CELL_CLASS, "w-44 font-sans")}
              autoComplete="off"
            />
            <select aria-label={t("Protokol")} value={rule.protocol} onChange={(event) => patch(index, { protocol: parseProtocol(event.target.value) })} className={SELECT_CLASS}>
              <option value="tcp">TCP</option>
              <option value="udp">UDP</option>
              <option value="both">{t("İkisi de")}</option>
            </select>
            <input
              aria-label={t("Portlar")}
              placeholder={t("8080 ya da 3000-3010")}
              value={rule.ports}
              onChange={(event) => patch(index, { ports: event.target.value })}
              className={classNames(CELL_CLASS, "w-44")}
              spellCheck={false}
              autoComplete="off"
            />
            <input
              aria-label={t("Kaynak adres")}
              placeholder={t("Herkes")}
              value={rule.source}
              onChange={(event) => patch(index, { source: event.target.value })}
              className={classNames(CELL_CLASS, "w-44")}
              spellCheck={false}
              autoComplete="off"
            />
            <select aria-label={t("İşlem")} value={rule.action} onChange={(event) => patch(index, { action: parseAction(event.target.value) })} className={classNames(SELECT_CLASS, rule.action === "drop" && "text-bad")}>
              <option value="accept">{t("İzin ver")}</option>
              <option value="drop">{t("Engelle")}</option>
            </select>
            <div className="ml-auto flex gap-0.5">
              <IconButton label={t("Yukarı taşı")} disabled={index === 0} onClick={() => move(index, -1)}>
                <ArrowUp className="size-4" />
              </IconButton>
              <IconButton label={t("Aşağı taşı")} disabled={index === draft.rules.length - 1} onClick={() => move(index, 1)}>
                <ArrowDown className="size-4" />
              </IconButton>
              <IconButton label={t("Kuralı kaldır")} className="hover:text-bad" onClick={() => remove(index)}>
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          </div>
        ))}
        <div>
          <Button icon={<Plus className="size-4" />} onClick={add}>
            {t("Engelleme kuralı ekle")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

// ---- Gelişmiş: dinleyen portlar ----

function ListenerStatus({ listener }: { listener: Listener }) {
  if (listener.local_only) return <Status tone="idle">{t("Yalnızca sunucu içi")}</Status>;
  const tone: Tone = listener.verdict === "open" ? "ok" : listener.verdict === "restricted" ? "accent" : "idle";
  const text = listener.verdict === "open" ? t("Herkese açık") : listener.verdict === "restricted" ? t("Tek adrese açık") : t("Kapalı");
  return <Status tone={tone}>{text}</Status>;
}

function ListenersPanel({ listeners }: { listeners: Listener[] }) {
  const sorted = useMemo(() => [...listeners].sort((a, b) => a.port - b.port || a.protocol.localeCompare(b.protocol)), [listeners]);

  return (
    <Panel title={t("Sunucuda çalışan bütün portlar")} flush>
      {sorted.length === 0 ? (
        <p className="px-4 py-4 text-ink-2">{t("Şu anda dinleyen bir port görünmüyor.")}</p>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{t("Port")}</Th>
              <Th>{t("Süreç")}</Th>
              <Th>{t("Dinlediği adres")}</Th>
              <Th>{t("Şu anki durum")}</Th>
            </tr>
          </thead>
          <tbody>
            {sorted.map((listener, index) => (
              <tr key={`${listener.protocol}-${listener.port}-${listener.address}-${index}`}>
                <Td>
                  <div className="flex items-center gap-2">
                    <Mono className="text-[13px]">{listener.port}</Mono>
                    <Badge>{listener.protocol.toUpperCase()}</Badge>
                  </div>
                </Td>
                <Td>{listener.process || "—"}</Td>
                <Td>
                  <Mono>{listener.address}</Mono>
                </Td>
                <Td>
                  <ListenerStatus listener={listener} />
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <p className="border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">
        {t("“Yalnızca sunucu içi” portlara dışarıdan zaten ulaşılamaz. Docker'ın dışarıya açtığı portlar bu kurallardan etkilenmez.")}
      </p>
    </Panel>
  );
}

// ---- Uygulama sonrası onay penceresi ----

function ConfirmDialog({
  secondsLeft,
  receivedAt,
  confirming,
  reverting,
  onConfirm,
  onRevert,
  onExpired,
}: {
  secondsLeft: number;
  /** Bu değerin sunucudan alındığı an (ms); geri sayım buna göre yürür. */
  receivedAt: number;
  confirming: boolean;
  reverting: boolean;
  onConfirm: () => void;
  onRevert: () => void;
  onExpired: () => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  const deadline = useMemo(() => receivedAt + secondsLeft * 1000, [receivedAt, secondsLeft]);
  const left = Math.max(0, Math.ceil((deadline - now) / 1000));

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, []);

  // Süre bitince sunucunun durumu yeniden okunur (kendiliğinden geri dönmüş olmalı).
  useEffect(() => {
    if (left === 0) onExpired();
  }, [left, onExpired]);

  return (
    <Modal
      open
      // Bu pencere yalnızca iki düğmeden biriyle kapanır.
      onClose={() => undefined}
      title={t("Hâlâ erişebiliyor musunuz?")}
      footer={
        <>
          <Button onClick={onRevert} loading={reverting} disabled={confirming}>
            {t("Geri al")}
          </Button>
          <Button variant="primary" onClick={onConfirm} loading={confirming} disabled={reverting}>
            {t("Evet, kuralları koru")}
          </Button>
        </>
      }
    >
      <div className="flex items-center gap-5">
        <div aria-hidden className="flex size-20 shrink-0 items-center justify-center rounded-full border-2 border-warn text-[30px] font-semibold text-warn tabular-nums">
          {left}
        </div>
        <div className="min-w-0">
          <p className="font-medium">{t("Yeni kurallar şu an yürürlükte.")}</p>
          <p className="mt-1 text-ink-2">{t("Bu pencereyi görüyorsanız panele erişiminiz sürüyor. SSH kullanıyorsanız yeni bir bağlantı açıp onu da deneyin.")}</p>
        </div>
      </div>
      <p role="status" className="mt-4 rounded-md bg-sunken px-3 py-2.5 text-[13px] text-ink-2">
        {t("{n} saniye içinde onaylamazsanız sunucu kendiliğinden önceki kurallara döner; kilitli kalmazsınız.", { n: left })}
      </p>
    </Modal>
  );
}

// ---- Sayfa içeriği ----

function FirewallContent({ view, receivedAt, onRefetch }: { view: FirewallView; receivedAt: number; onRefetch: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  // Kullanıcının düzenlediği taslak; null ise sunucudaki ayarlar gösterilir.
  const [edits, setEdits] = useState<Settings | null>(null);
  const [addingPort, setAddingPort] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const base = view.pending?.settings ?? view.settings;
  const draft = edits ?? base;
  const locked = view.pending !== null;
  const dirty = edits !== null && JSON.stringify(normalizeSettings(edits)) !== JSON.stringify(normalizeSettings(base));

  const update: UpdateSettings = (change) => setEdits((current) => change(current ?? base));

  const store = (next: FirewallView) => queryClient.setQueryData(QUERY_KEY, next);

  // Onay/geri alma kullanıcıdan geldiğinde "süre doldu" bildirimi gösterilmez.
  const resolvedByUser = useRef(false);
  const hadPending = useRef(false);
  const hasPending = view.pending !== null;
  useEffect(() => {
    if (hasPending) resolvedByUser.current = false;
    else if (hadPending.current && !resolvedByUser.current) feedback.success(t("Onaylanmadığı için önceki kurallara dönüldü"));
    hadPending.current = hasPending;
  }, [hasPending, feedback]);

  const apply = useMutation({
    mutationFn: (settings: Settings) => api.put<FirewallView>("/firewall", { settings: normalizeSettings(settings) }),
    onSuccess: (next) => {
      store(next);
      setEdits(null);
    },
  });

  const confirm = useMutation({
    mutationFn: () => api.post<FirewallView>("/firewall/confirm"),
    onSuccess: (next) => {
      resolvedByUser.current = true;
      store(next);
      setEdits(null);
      feedback.success(t("Kurallar kalıcı oldu"));
    },
    onError: (error) => feedback.error(error),
  });

  const revert = useMutation({
    mutationFn: () => api.post<FirewallView>("/firewall/revert"),
    onSuccess: (next) => {
      resolvedByUser.current = true;
      store(next);
      setEdits(null);
      feedback.success(t("Önceki kurallara dönüldü"));
    },
    onError: (error) => feedback.error(error),
  });

  function discard() {
    setEdits(null);
    apply.reset();
  }

  return (
    <>
      {view.pending && (
        <ConfirmDialog
          secondsLeft={view.pending.seconds_left}
          receivedAt={receivedAt}
          confirming={confirm.isPending}
          reverting={revert.isPending}
          onConfirm={() => confirm.mutate()}
          onRevert={() => revert.mutate()}
          onExpired={onRefetch}
        />
      )}

      {dirty && !locked && (
        <div className="sticky top-0 z-10 mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg border border-accent/50 bg-surface px-4 py-2.5 shadow-pop">
          <div className="min-w-0">
            <p className="font-medium">{t("Değişiklikleriniz henüz uygulanmadı")}</p>
            <p className="text-[12.5px] text-ink-3">{t("Uyguladıktan sonra erişiminizi denemeniz için {n} saniyeniz olur; onaylamazsanız her şey eski hâline döner.", { n: view.confirm_seconds })}</p>
          </div>
          <div className="flex items-center gap-2">
            <Button onClick={discard} disabled={apply.isPending}>
              {t("Vazgeç")}
            </Button>
            <Button variant="primary" loading={apply.isPending} onClick={() => apply.mutate(draft)}>
              {t("Uygula")}
            </Button>
          </div>
        </div>
      )}

      {apply.isError && (
        <div className="mb-4">
          <ErrorBlock message={errorMessage(apply.error)} />
        </div>
      )}

      <fieldset disabled={locked} className="m-0 flex min-w-0 flex-col gap-4 border-0 p-0">
        <Panel>
          <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-3">
            <div className="flex min-w-0 items-start gap-3">
              <div className="pt-0.5">
                <Switch checked={draft.enabled} label={t("Güvenlik duvarı")} onChange={(next) => update((current) => ({ ...current, enabled: next }))} />
              </div>
              <div className="min-w-0">
                <p className="font-medium">{draft.enabled ? t("Güvenlik duvarı açık") : t("Güvenlik duvarı kapalı")}</p>
                <p className="max-w-[70ch] text-[13px] text-ink-2">
                  {draft.enabled
                    ? t("Yalnızca aşağıda açık görünen kapılardan girilebilir; geri kalan her şey reddedilir.")
                    : t("Sunucuda çalışan her şeye dışarıdan ulaşılabilir. Açtığınızda yalnızca aşağıda açık görünen kapılar kalır.")}
                </p>
              </div>
            </div>
            {view.your_ip !== "" && (
              <p className="text-[13px] text-ink-2">
                {t("Sizin adresiniz")}: <Mono className="text-[13px] text-ink">{view.your_ip}</Mono>
              </p>
            )}
          </div>
        </Panel>

        <DoorsPanel view={view} draft={draft} onUpdate={update} onAddPort={() => setAddingPort(true)} />
        <BlockedPanel draft={draft} onUpdate={update} />

        <div>
          <button
            type="button"
            aria-expanded={advanced}
            onClick={() => setAdvanced((current) => !current)}
            className="flex items-center gap-1.5 rounded-md py-1 text-[13px] font-medium text-ink-2 hover:text-ink"
          >
            {advanced ? <ChevronDown aria-hidden className="size-4" /> : <ChevronRight aria-hidden className="size-4" />}
            {advanced ? t("Gelişmiş ayarları gizle") : t("Gelişmiş ayarları göster")}
          </button>
        </div>
        {advanced && (
          <>
            <RulesPanel draft={draft} onUpdate={update} />
            <ListenersPanel listeners={view.listeners} />
          </>
        )}
      </fieldset>

      <AddPortModal open={addingPort} yourIp={view.your_ip} onClose={() => setAddingPort(false)} onAdd={(rule) => update((current) => ({ ...current, rules: [...current.rules, rule] }))} />
    </>
  );
}

// ---- Sayfa ----

export function FirewallPage() {
  const query = useQuery({
    queryKey: QUERY_KEY,
    queryFn: () => api.get<FirewallView>("/firewall"),
    // Onay beklerken sunucudaki durum sık sık okunur.
    refetchInterval: (current) => (current.state.data?.pending ? POLL_INTERVAL_MS : false),
  });

  const { refetch } = query;
  const refetchView = useMemo(() => () => void refetch(), [refetch]);

  const view = query.data;
  let body;
  if (!view) {
    body = query.isError ? <ErrorBlock message={errorMessage(query.error)} onRetry={() => void query.refetch()} /> : <LoadingBlock />;
  } else if (!view.available) {
    body = (
      <Panel flush>
        <EmptyState
          title={t("nftables bu sunucuda kurulu değil")}
          description={t("Güvenlik duvarını yönetebilmek için sunucuya nftables paketinin kurulu olması gerekir.")}
        />
      </Panel>
    );
  } else {
    body = <FirewallContent view={view} receivedAt={query.dataUpdatedAt} onRefetch={refetchView} />;
  }

  return (
    <>
      <PageHeader title={t("Güvenlik duvarı")} description={t("Sunucunun hangi kapısının kime açık olduğunu buradan belirlersiniz.")} />
      <SecurityTabs />
      {body}
    </>
  );
}
