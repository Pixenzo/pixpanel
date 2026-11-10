// Güvenlik duvarı sayfası: sunucuya hangi portlardan ve hangi adreslerden erişilebileceğini belirler.
// Değişiklikler hemen uygulanır; süre dolmadan onaylanmazsa sunucu önceki kurallara kendiliğinden döner.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDown, ArrowUp, Plus, Trash2, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, EmptyState, ErrorBlock, IconButton, LoadingBlock, Mono, PageHeader, Panel, Status, Table, Td, Textarea, Th } from "../components/ui";
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

function presetPorts(preset: Preset): string {
  const parts: string[] = [];
  if (preset.tcp.length > 0) parts.push(`tcp ${preset.tcp.join(", ")}`);
  if (preset.udp.length > 0) parts.push(`udp ${preset.udp.join(", ")}`);
  return parts.join(" · ");
}

function stateOf(settings: Settings, key: string): PresetState {
  return settings.presets.find((state) => state.key === key) ?? { key, enabled: false, source: "" };
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

function LinkButton({ children, onClick }: { children: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="text-[12.5px] whitespace-nowrap text-accent hover:underline disabled:cursor-not-allowed disabled:opacity-55">
      {children}
    </button>
  );
}

// ---- Onay bekleyen kurallar ----

function PendingBanner({
  secondsLeft,
  receivedAt,
  busy,
  onConfirm,
  onRevert,
  onExpired,
}: {
  secondsLeft: number;
  /** Bu değerin sunucudan alındığı an (ms); geri sayım buna göre yürür. */
  receivedAt: number;
  busy: boolean;
  onConfirm: () => void;
  onRevert: () => void;
  onExpired: () => void;
}) {
  const [now, setNow] = useState(() => Date.now());
  const deadline = useMemo(() => receivedAt + secondsLeft * 1000, [receivedAt, secondsLeft]);
  const left = Math.max(0, Math.ceil((deadline - now) / 1000));

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  // Süre bitince sunucunun durumu yeniden okunur (kendiliğinden geri dönmüş olmalı).
  useEffect(() => {
    if (left === 0) onExpired();
  }, [left, onExpired]);

  return (
    <section aria-label={t("Yeni kurallar uygulandı; onay bekleniyor")} className="mb-4 rounded-lg border border-warn/50 bg-warn-soft px-4 py-3.5">
      <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1">
          <h2 className="flex items-center gap-2 font-semibold text-warn">
            <TriangleAlert aria-hidden className="size-4 shrink-0" />
            {t("Yeni kurallar uygulandı; onay bekleniyor")}
          </h2>
          <p className="mt-1.5 font-medium text-ink">
            <span className="tabular-nums">{t("{n} saniye içinde onaylamazsanız önceki kurallara dönülecek", { n: left })}</span>
          </p>
          <p className="mt-1 max-w-[75ch] text-ink-2">
            {t("Bu sayfayı hâlâ görebiliyorsanız panele erişiminiz kesilmemiş demektir. SSH gibi başka bağlantılarınızı da denedikten sonra onaylayın.")}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button onClick={onRevert} disabled={busy}>
            {t("Geri al")}
          </Button>
          <Button variant="primary" onClick={onConfirm} loading={busy}>
            {t("Kuralları onayla")}
          </Button>
        </div>
      </div>
    </section>
  );
}

// ---- Ana anahtar ----

function MasterPanel({ view, draft, onUpdate }: { view: FirewallView; draft: Settings; onUpdate: UpdateSettings }) {
  const firstTime = draft.enabled && !view.settings.enabled;
  return (
    <Panel title={t("Genel durum")} actions={view.settings.enabled ? <Status tone="ok">{t("Açık")}</Status> : <Status tone="idle">{t("Kapalı")}</Status>}>
      <div className="flex flex-col gap-3">
        <dl className="flex flex-wrap gap-x-8 gap-y-2">
          <div>
            <dt className="text-[12.5px] text-ink-2">{t("Sizin adresiniz")}</dt>
            <dd className="mt-0.5 font-medium">
              <Mono className="text-[13px]">{view.your_ip || "—"}</Mono>
            </dd>
          </div>
        </dl>
        <div className="flex items-center gap-2.5">
          <Switch checked={draft.enabled} label={t("Güvenlik duvarını etkinleştir")} onChange={(next) => onUpdate((current) => ({ ...current, enabled: next }))} />
          <span className="text-ink">{t("Güvenlik duvarını etkinleştir")}</span>
        </div>
        {firstTime && (
          <p className="max-w-[75ch] text-[13px] text-ink-2">{t("Açtığınızda aşağıdaki işaretli servisler dışındaki bütün portlar dışarıya kapanır.")}</p>
        )}
      </div>
    </Panel>
  );
}

// ---- Servisler ----

function PresetsPanel({ view, draft, onUpdate }: { view: FirewallView; draft: Settings; onUpdate: UpdateSettings }) {
  const feedback = useFeedback();

  const patch = (key: string, changes: Partial<PresetState>) =>
    onUpdate((current) => {
      const exists = current.presets.some((state) => state.key === key);
      const presets = exists
        ? current.presets.map((state) => (state.key === key ? { ...state, ...changes } : state))
        : [...current.presets, { ...stateOf(current, key), ...changes }];
      return { ...current, presets };
    });

  async function toggle(preset: Preset, enabled: boolean) {
    if (!enabled && preset.critical) {
      const confirmed = await feedback.confirm({
        title: t("“{label}” kapatılsın mı?", { label: preset.label }),
        description: disableWarning(preset),
        confirmLabel: t("Kapat"),
        danger: true,
      });
      if (!confirmed) return;
    }
    patch(preset.key, { enabled });
  }

  return (
    <Panel title={t("Servisler")} flush>
      {view.presets.length === 0 ? (
        <p className="px-4 py-4 text-ink-2">{t("Bu sunucuda kurallarını yönetebileceğiniz bir servis yok.")}</p>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>
                <span className="sr-only">{t("Açık")}</span>
              </Th>
              <Th>{t("Servis")}</Th>
              <Th>{t("Portlar")}</Th>
              <Th>{t("Kimler erişebilir")}</Th>
            </tr>
          </thead>
          <tbody>
            {view.presets.map((preset) => {
              const state = stateOf(draft, preset.key);
              return (
                <tr key={preset.key}>
                  <Td className="w-14">
                    <Switch checked={state.enabled} label={preset.label} onChange={(next) => void toggle(preset, next)} />
                  </Td>
                  <Td>
                    <div className="font-medium">{preset.label}</div>
                    <div className="text-[13px] text-ink-2">{preset.description}</div>
                  </Td>
                  <Td>
                    <Mono>{presetPorts(preset) || "—"}</Mono>
                  </Td>
                  <Td>
                    <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                      <input
                        aria-label={t("Kimler erişebilir: {label}", { label: preset.label })}
                        placeholder={t("Herkes")}
                        value={state.source}
                        onChange={(event) => patch(preset.key, { source: event.target.value })}
                        className={classNames(CELL_CLASS, "w-52")}
                        spellCheck={false}
                        autoComplete="off"
                      />
                      {preset.critical && view.your_ip !== "" && <LinkButton onClick={() => patch(preset.key, { source: view.your_ip })}>{t("Benim adresimle sınırla")}</LinkButton>}
                    </div>
                  </Td>
                </tr>
              );
            })}
          </tbody>
        </Table>
      )}
      <p className="border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">
        {t("Tek adres ya da ağ yazarak erişimi daraltabilirsiniz; örneğin 203.0.113.0/24. Boş bırakırsanız her adres erişebilir.")}
      </p>
    </Panel>
  );
}

// ---- Özel kurallar ----

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
  const add = () => onUpdate((current) => ({ ...current, rules: [...current.rules, { ...EMPTY_RULE }] }));

  return (
    <Panel title={t("Özel kurallar")}>
      <p className="mb-4 max-w-[75ch] text-ink-2">{t("Kurallar sırayla değerlendirilir, ilk uyan geçerlidir; özel kurallar servislerden önce gelir.")}</p>
      <div className="flex flex-col gap-2">
        {draft.rules.length === 0 && <p className="rounded-md bg-sunken/70 px-3 py-2.5 text-[13px] text-ink-2">{t("Henüz özel kural yok. “Kural ekle” ile ilk kuralınızı tanımlayın.")}</p>}
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
            {t("Kural ekle")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

// ---- Engelli adresler ----

function BlockedPanel({ draft, onUpdate }: { draft: Settings; onUpdate: UpdateSettings }) {
  return (
    <Panel title={t("Engelli adresler")}>
      <div className="max-w-xl">
        <Textarea
          label={t("Adresler ve ağlar")}
          hint={t("Her satıra bir adres ya da ağ yazın, örneğin 203.0.113.5 ya da 198.51.100.0/24. Bu adreslerden gelen bütün bağlantılar reddedilir.")}
          value={draft.blocked.join("\n")}
          onChange={(event) => onUpdate((current) => ({ ...current, blocked: event.target.value.split("\n") }))}
          spellCheck={false}
          autoComplete="off"
          rows={5}
        />
      </div>
    </Panel>
  );
}

// ---- Dinleyen portlar ----

function ListenerStatus({ listener }: { listener: Listener }) {
  if (listener.local_only) return <Status tone="idle">{t("Yalnızca sunucu içi")}</Status>;
  const tone: Tone = listener.verdict === "open" ? "ok" : listener.verdict === "restricted" ? "accent" : "idle";
  const text = listener.verdict === "open" ? t("Dışarıya açık") : listener.verdict === "restricted" ? t("Belirli adreslere açık") : t("Kapalı");
  return <Status tone={tone}>{text}</Status>;
}

function ListenersPanel({ listeners, onUpdate }: { listeners: Listener[]; onUpdate: UpdateSettings }) {
  const sorted = useMemo(() => [...listeners].sort((a, b) => a.port - b.port || a.protocol.localeCompare(b.protocol)), [listeners]);

  const allow = (listener: Listener) =>
    onUpdate((current) => ({
      ...current,
      rules: [...current.rules, { label: listener.process, protocol: listener.protocol, ports: String(listener.port), source: "", action: "accept" }],
    }));

  return (
    <Panel title={t("Dinleyen portlar")} flush>
      {sorted.length === 0 ? (
        <p className="px-4 py-4 text-ink-2">{t("Şu anda dinleyen bir port görünmüyor.")}</p>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{t("Port")}</Th>
              <Th>{t("Süreç")}</Th>
              <Th>{t("Dinlediği adres")}</Th>
              <Th>{t("Durum")}</Th>
              <Th align="right">
                <span className="sr-only">{t("İşlemler")}</span>
              </Th>
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
                <Td align="right">
                  {!listener.local_only && listener.verdict === "closed" && (
                    <Button size="sm" onClick={() => allow(listener)}>
                      {t("İzin ver")}
                    </Button>
                  )}
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <p className="border-t border-line px-4 py-2.5 text-[12.5px] text-ink-3">{t("Docker'ın dışarıya açtığı portlar bu kurallardan etkilenmez.")}</p>
    </Panel>
  );
}

// ---- Sayfa içeriği ----

function FirewallContent({ view, receivedAt, onRefetch }: { view: FirewallView; receivedAt: number; onRefetch: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();

  // Kullanıcının düzenlediği taslak; null ise sunucudaki ayarlar gösterilir.
  const [edits, setEdits] = useState<Settings | null>(null);
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
        <PendingBanner
          secondsLeft={view.pending.seconds_left}
          receivedAt={receivedAt}
          busy={confirm.isPending || revert.isPending}
          onConfirm={() => confirm.mutate()}
          onRevert={() => revert.mutate()}
          onExpired={onRefetch}
        />
      )}

      {dirty && !locked && (
        <div className="sticky top-0 z-10 mb-4 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 rounded-lg border border-line-strong bg-surface px-4 py-2.5 shadow-pop">
          <div className="min-w-0">
            <p className="font-medium">{t("Kaydedilmemiş değişiklikler")}</p>
            <p className="text-[12.5px] text-ink-3">{t("Uyguladıktan sonra {n} saniye içinde onaylamanız gerekir.", { n: view.confirm_seconds })}</p>
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
        <MasterPanel view={view} draft={draft} onUpdate={update} />
        <PresetsPanel view={view} draft={draft} onUpdate={update} />
        <RulesPanel draft={draft} onUpdate={update} />
        <BlockedPanel draft={draft} onUpdate={update} />
        <ListenersPanel listeners={view.listeners} onUpdate={update} />
      </fieldset>
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
      <PageHeader
        title={t("Güvenlik duvarı")}
        description={t("Sunucuya hangi portlardan ve hangi adreslerden erişilebileceğini belirler; izin verilmeyen her bağlantı reddedilir.")}
      />
      {body}
    </>
  );
}
