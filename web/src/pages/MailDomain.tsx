// Posta alan adı çalışma alanı: posta kutuları (yönlendirme, otomatik yanıt ve
// süzgeç kuralları dâhil), takma adlar, DNS kayıtları ve alan adı ayarları.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { LucideIcon } from "lucide-react";
import { ArrowDown, ArrowLeft, ArrowUp, AtSign, Check, Copy, ExternalLink, Globe, Inbox, MailOpen, MailPlus, Pencil, Plus, RefreshCw, Shuffle, SlidersHorizontal, Smartphone, Trash2, Users } from "lucide-react";
import type { FormEvent, ReactNode } from "react";
import { useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Meter,
  Modal,
  Mono,
  Panel,
  Spinner,
  Status,
  Table,
  Tabs,
  Td,
  Textarea,
  Th,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatBytes, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { MailImportModal } from "./MailImport";
import { useMe } from "../lib/session";
import { MailClientSettings } from "./MailClientSettings";
import { MailLists } from "./MailLists";
import type { AutoReply, DnsRecord, FilterAction, FilterCondition, MailAlias, MailDomain, MailFilter, Mailbox } from "./Mail";

interface DomainDetail {
  domain: MailDomain;
  mailboxes: Mailbox[];
  aliases: MailAlias[];
  dns: DnsRecord[];
  hostname: string;
  /** Alan adının DNS'i panelde yönetiliyor mu? */
  dns_managed: boolean;
  /** Web postası kurulu mu? */
  webmail: boolean;
  /** Posta istemcilerinin bağlanacağı ad (`mail.<alan>` ya da sunucu adı). */
  client_host: string;
  /** `webmail.<alan>` adresi sunuluyorsa adı. */
  webmail_host: string | null;
}

/** Web postasının adresi; kutu verilirse giriş sayfasında adres yazılı gelir. */
function webmailUrl(address?: string): string {
  return address ? `/webmail/?_user=${encodeURIComponent(address)}` : "/webmail/";
}

type MailboxChanges = Partial<Pick<Mailbox, "quota_mb" | "enabled" | "forward_to" | "forward_keep" | "auto_reply" | "filters">> & { password?: string; send_limit?: number };

/** Saatlik gönderim sınırını yalnızca yönetici ve bayi değiştirebilir. */
function useCanSetSendLimit(): boolean {
  const me = useMe();
  return me.role === "admin" || me.role === "reseller";
}

/** Sınırın okunur hâli: 0 sınırsızdır. */
function limitLabel(limit: number): string {
  return limit > 0 ? String(limit) : t("sınırsız");
}

const detailKey = (domainId: number | string) => ["mail-domain", String(domainId)] as const;

const byAddress = <T extends { address: string }>(a: T, b: T) => a.address.localeCompare(b.address);

/** Önbellekteki alan adı ayrıntısını günceller. */
function useDetailCache(domainId: number) {
  const queryClient = useQueryClient();
  return (update: (detail: DomainDetail) => DomainDetail) =>
    queryClient.setQueryData<DomainDetail>(detailKey(domainId), (old) => (old ? update(old) : old));
}

// ---- Ortak küçük bileşenler ----

function useCopy(): [boolean, (value: string) => Promise<void>] {
  const [copied, setCopied] = useState(false);
  async function copy(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1800);
    } catch {
      // Pano kapalıysa kullanıcı metni elle seçip kopyalayabilir.
    }
  }
  return [copied, copy];
}

function CopyField({ label, value, hint, multiline }: { label: string; value: string; hint?: string; multiline?: boolean }) {
  const [copied, copy] = useCopy();
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <p className="text-[13.5px] font-medium text-ink-2">{label}</p>
      <div className="flex items-start gap-2">
        <code
          className={`min-w-0 flex-1 rounded-md border border-line bg-sunken/60 px-3 py-2 font-mono text-[13px] select-all ${multiline ? "break-all" : "truncate"}`}
        >
          {value}
        </code>
        <IconButton label={copied ? t("Kopyalandı") : t("Kopyala")} onClick={() => void copy(value)} className="mt-0.5">
          {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
        </IconButton>
      </div>
      {hint && <p className="max-w-[75ch] text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}

// Karışabilecek karakterler (0/O, 1/l/I) çıkarılmıştır.
const passwordAlphabet = "abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** Tarayıcının güvenli rastgele kaynağıyla parola üretir; mod önyargısı için sınır dışı baytlar atılır. */
function generatePassword(length = 16): string {
  const limit = 256 - (256 % passwordAlphabet.length);
  let result = "";
  while (result.length < length) {
    const bytes = crypto.getRandomValues(new Uint8Array(length * 2));
    for (const byte of bytes) {
      if (byte < limit && result.length < length) result += passwordAlphabet.charAt(byte % passwordAlphabet.length);
    }
  }
  return result;
}

function PasswordField({
  label,
  value,
  onChange,
  hint,
  required,
}: {
  label: string;
  value: string;
  onChange: (next: string) => void;
  hint?: string;
  required?: boolean;
}) {
  const [copied, copy] = useCopy();
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-end gap-2">
        <div className="min-w-0 flex-1">
          <Input
            label={label}
            value={value}
            onChange={(event) => onChange(event.target.value)}
            className="font-mono text-[13.5px]"
            autoComplete="new-password"
            autoCapitalize="none"
            spellCheck={false}
            required={required}
          />
        </div>
        <Button icon={<Shuffle className="size-3.5" />} onClick={() => onChange(generatePassword())}>
          {t("Rastgele üret")}
        </Button>
        <IconButton label={copied ? t("Kopyalandı") : t("Parolayı kopyala")} disabled={!value} onClick={() => void copy(value)} className="size-9 border border-line-strong">
          {copied ? <Check className="size-4 text-ok" /> : <Copy className="size-4" />}
        </IconButton>
      </div>
      {hint && <p className="text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}

/** Adres yerel kısmı; alan adı sabit bir son ek olarak gösterilir. */
function LocalPartInput({
  label,
  value,
  onChange,
  domain,
  disabled,
  autoFocus,
}: {
  label: string;
  value: string;
  onChange: (next: string) => void;
  domain: string;
  disabled?: boolean;
  autoFocus?: boolean;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-[13.5px] font-medium text-ink-2">{label}</span>
      <span className="flex">
        <input
          value={value}
          onChange={(event) => onChange(event.target.value)}
          disabled={disabled}
          autoFocus={autoFocus}
          autoCapitalize="none"
          autoComplete="off"
          spellCheck={false}
          required
          className="h-9 min-w-0 flex-1 rounded-l-md border border-r-0 border-line-strong bg-surface px-3 font-mono text-[13.5px] text-ink transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 disabled:bg-sunken disabled:text-ink-3"
        />
        <span className="inline-flex h-9 items-center rounded-r-md border border-line-strong bg-sunken px-3 font-mono text-[13.5px] text-ink-2">@{domain}</span>
      </span>
    </label>
  );
}

/** Her sekmenin kendi kaydet düğmesi olan formu. */
function FormShell({
  onSubmit,
  saving,
  error,
  children,
}: {
  onSubmit: () => void;
  saving: boolean;
  error: unknown;
  children: ReactNode;
}) {
  function submit(event: FormEvent) {
    event.preventDefault();
    onSubmit();
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      {children}
      {error ? <ErrorBlock message={typeof error === "string" ? error : errorMessage(error)} /> : null}
      <div>
        <Button type="submit" variant="primary" loading={saving}>
          {t("Kaydet")}
        </Button>
      </div>
    </form>
  );
}

const linesToList = (text: string) =>
  text
    .split(/[\s,;]+/)
    .map((item) => item.trim())
    .filter(Boolean);

// ---- Posta kutusu: kaydetme kancası ----

function useSaveMailbox(mailbox: Mailbox) {
  const feedback = useFeedback();
  const updateCache = useDetailCache(mailbox.domain_id);
  return useMutation({
    mutationFn: (vars: { changes: MailboxChanges; message: string }) => api.patch<Mailbox>(`/mail/mailboxes/${mailbox.id}`, vars.changes),
    onSuccess: (updated, vars) => {
      updateCache((detail) => ({ ...detail, mailboxes: detail.mailboxes.map((item) => (item.id === updated.id ? updated : item)) }));
      feedback.success(vars.message);
    },
  });
}

// ---- Posta kutusu düzenleme sekmeleri ----

function GeneralTab({ mailbox }: { mailbox: Mailbox }) {
  const save = useSaveMailbox(mailbox);
  const [enabled, setEnabled] = useState(mailbox.enabled);
  const [quota, setQuota] = useState(String(mailbox.quota_mb));
  const [password, setPassword] = useState("");
  const [sendLimit, setSendLimit] = useState(mailbox.send_limit === null ? "" : String(mailbox.send_limit));
  const canSetSendLimit = useCanSetSendLimit();
  const [localError, setLocalError] = useState<string | null>(null);

  function submit() {
    const quotaMb = Number(quota);
    if (!Number.isInteger(quotaMb) || quotaMb < 1) {
      setLocalError(t("Kota, en az 1 olan bir tam sayı (MB) olmalıdır."));
      return;
    }
    setLocalError(null);
    const changes: MailboxChanges = { enabled, quota_mb: quotaMb };
    if (canSetSendLimit) {
      // Boş alan "varsayılana dön" demektir (-1).
      const limit = sendLimit.trim() === "" ? -1 : Number(sendLimit);
      if (!Number.isInteger(limit) || limit < -1 || limit > 1_000_000) {
        setLocalError(t("Gönderim sınırı, 0 ile 1.000.000 arasında bir tam sayı olmalıdır (boş: varsayılan)."));
        return;
      }
      changes.send_limit = limit;
    }
    if (password) changes.password = password;
    save.mutate({ changes, message: t("Kutu ayarları kaydedildi") }, { onSuccess: () => setPassword("") });
  }

  return (
    <FormShell onSubmit={submit} saving={save.isPending} error={localError ?? save.error}>
      <Checkbox
        label={t("Kutu etkin")}
        hint={t("Kapalı kutu oturum açamaz ve posta kabul etmez.")}
        checked={enabled}
        onChange={(event) => setEnabled(event.target.checked)}
      />
      <Input
        label={t("Kota (MB)")}
        type="number"
        min={1}
        value={quota}
        onChange={(event) => setQuota(event.target.value)}
        hint={mailbox.used_bytes === null ? undefined : t("Şu an kullanılan: {used}", { used: formatBytes(mailbox.used_bytes) })}
        className="max-w-48"
        required
      />
      {canSetSendLimit ? (
        <Input
          label={t("Saatlik gönderim sınırı")}
          type="number"
          min={0}
          value={sendLimit}
          onChange={(event) => setSendLimit(event.target.value)}
          placeholder={t("Varsayılan")}
          hint={t("Kutunun saatte en fazla kaç alıcıya gönderebileceği. Boş: sunucu varsayılanı ({limit}); 0: sınırsız. Son bir saatte: {sent}.", {
            limit: limitLabel(sendLimit.trim() === "" ? mailbox.effective_limit : Number(sendLimit)),
            sent: mailbox.sent_last_hour,
          })}
          className="max-w-48"
        />
      ) : (
        <p className="text-[13.5px] text-ink-2">
          {t("Saatlik gönderim sınırı: {limit}. Son bir saatte gönderilen: {sent}. Sınırı yöneticiniz değiştirebilir.", {
            limit: limitLabel(mailbox.effective_limit),
            sent: mailbox.sent_last_hour,
          })}
        </p>
      )}
      <PasswordField label={t("Yeni parola")} value={password} onChange={setPassword} hint={t("Parolayı değiştirmek istemiyorsanız boş bırakın. En az 10 karakter olmalıdır.")} />
    </FormShell>
  );
}

const MAX_FORWARDS = 5;

function ForwardTab({ mailbox }: { mailbox: Mailbox }) {
  const save = useSaveMailbox(mailbox);
  const [addresses, setAddresses] = useState<string[]>(mailbox.forward_to);
  const [keep, setKeep] = useState(mailbox.forward_keep);
  const [localError, setLocalError] = useState<string | null>(null);

  function submit() {
    const cleaned = addresses.map((item) => item.trim()).filter(Boolean);
    if (cleaned.some((item) => !item.includes("@"))) {
      setLocalError(t("Yönlendirme adreslerinin tamamı e-posta adresi olmalıdır."));
      return;
    }
    setLocalError(null);
    save.mutate({ changes: { forward_to: cleaned, forward_keep: keep }, message: t("Yönlendirme ayarları kaydedildi") });
  }

  return (
    <FormShell onSubmit={submit} saving={save.isPending} error={localError ?? save.error}>
      <fieldset className="flex flex-col gap-2">
        <legend className="mb-1 text-[13.5px] font-medium text-ink-2">{t("Yönlendirme adresleri")}</legend>
        <p className="mb-1 text-[13px] text-ink-3">{t("Bu kutuya gelen her ileti aşağıdaki adreslere de gönderilir. En fazla {n} adres eklenebilir.", { n: MAX_FORWARDS })}</p>
        {addresses.length === 0 && <p className="text-ink-2">{t("Yönlendirme yok.")}</p>}
        {addresses.map((address, index) => (
          <div key={index} className="flex items-center gap-2">
            <input
              aria-label={t("Yönlendirme adresi")}
              type="email"
              placeholder="adres@ornek.com"
              value={address}
              onChange={(event) => setAddresses((list) => list.map((item, i) => (i === index ? event.target.value : item)))}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]"
              autoCapitalize="none"
              spellCheck={false}
            />
            <IconButton label={t("Adresi kaldır")} onClick={() => setAddresses((list) => list.filter((_, i) => i !== index))}>
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        ))}
        <div>
          <Button size="sm" icon={<Plus className="size-3.5" />} disabled={addresses.length >= MAX_FORWARDS} onClick={() => setAddresses((list) => [...list, ""])}>
            {t("Adres ekle")}
          </Button>
        </div>
      </fieldset>
      <Checkbox
        label={t("Bir kopyasını bu kutuda sakla")}
        hint={t("Kapalıysa iletiler yalnızca yönlendirme adreslerine gider ve bu kutuya düşmez.")}
        checked={keep}
        disabled={addresses.every((item) => !item.trim())}
        onChange={(event) => setKeep(event.target.checked)}
      />
    </FormShell>
  );
}

function AutoReplyTab({ mailbox }: { mailbox: Mailbox }) {
  const save = useSaveMailbox(mailbox);
  const [reply, setReply] = useState<AutoReply>(mailbox.auto_reply);
  const [localError, setLocalError] = useState<string | null>(null);
  const set = <K extends keyof AutoReply>(key: K, value: AutoReply[K]) => setReply((current) => ({ ...current, [key]: value }));

  function submit() {
    if (reply.enabled && !reply.body.trim()) {
      setLocalError(t("Otomatik yanıt etkinken ileti metni boş olamaz."));
      return;
    }
    if (reply.start_date && reply.end_date && reply.end_date < reply.start_date) {
      setLocalError(t("Bitiş tarihi, başlangıç tarihinden önce olamaz."));
      return;
    }
    setLocalError(null);
    save.mutate({ changes: { auto_reply: reply }, message: t("Otomatik yanıt kaydedildi") });
  }

  return (
    <FormShell onSubmit={submit} saving={save.isPending} error={localError ?? save.error}>
      <Checkbox
        label={t("Otomatik yanıt etkin")}
        hint={t("Her gönderene günde en fazla bir kez yanıt verilir.")}
        checked={reply.enabled}
        onChange={(event) => set("enabled", event.target.checked)}
      />
      <Input label={t("Konu")} value={reply.subject} onChange={(event) => set("subject", event.target.value)} placeholder={t("Ofis dışındayım")} />
      <Textarea
        label={t("İleti metni")}
        value={reply.body}
        onChange={(event) => set("body", event.target.value)}
        placeholder={t("Merhaba, 20 Ocak'a kadar ofis dışındayım. Dönüşümde size yazacağım.")}
        className="min-h-32 font-sans text-[14px]"
      />
      <div className="grid gap-4 sm:grid-cols-2">
        <Input label={t("Başlangıç tarihi")} type="date" value={reply.start_date} onChange={(event) => set("start_date", event.target.value)} />
        <Input
          label={t("Bitiş tarihi")}
          type="date"
          value={reply.end_date}
          min={reply.start_date || undefined}
          onChange={(event) => set("end_date", event.target.value)}
        />
      </div>
      <p className="-mt-2 text-[13px] text-ink-3">{t("Tarihler isteğe bağlıdır; boş bırakırsanız yanıt, kapatana kadar sürer.")}</p>
    </FormShell>
  );
}

// ---- Süzgeç kural oluşturucu ----

const fieldOrder: FilterCondition["field"][] = ["from", "to", "cc", "subject", "header", "size_over"];
const fieldLabels: Record<FilterCondition["field"], string> = {
  from: "Gönderen",
  to: "Alıcı",
  cc: "Bilgi (Cc)",
  subject: "Konu",
  header: "Başlık",
  size_over: "Boyut büyükse",
};
const opOrder: FilterCondition["op"][] = ["contains", "not_contains", "is", "matches"];
const opLabels: Record<FilterCondition["op"], string> = {
  contains: "içerir",
  not_contains: "içermez",
  is: "tam olarak eşit",
  matches: "kalıba uyar (* ve ?)",
};
const actionOrder: FilterAction["kind"][] = ["move", "copy", "mark_read", "flag", "forward", "discard"];
const actionLabels: Record<FilterAction["kind"], string> = {
  move: "Klasöre taşı",
  copy: "Klasöre kopyala",
  mark_read: "Okundu işaretle",
  flag: "Yıldızla",
  forward: "Adrese ilet",
  discard: "Sil",
};

const actionNeedsValue = (kind: FilterAction["kind"]) => kind === "move" || kind === "copy" || kind === "forward";

const rowControl =
  "h-9 rounded-md border border-line-strong bg-surface px-2.5 text-ink placeholder:text-ink-3 transition-colors " +
  "focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 disabled:bg-sunken disabled:text-ink-3";

const newCondition = (): FilterCondition => ({ field: "from", header: "", op: "contains", value: "" });
const newAction = (): FilterAction => ({ kind: "move", value: "" });
const newRule = (number: number): MailFilter => ({
  name: t("Kural {n}", { n: number }),
  enabled: true,
  match_all: true,
  conditions: [newCondition()],
  actions: [newAction()],
  stop: false,
});

/** Kaydetmeden önce kuralları temizler; kullanılmayan alanlar boşaltılır. */
function normalizeFilters(filters: MailFilter[]): MailFilter[] {
  return filters.map((rule) => ({
    ...rule,
    name: rule.name.trim(),
    conditions: rule.conditions.map((condition) => ({
      field: condition.field,
      header: condition.field === "header" ? condition.header.trim() : "",
      op: condition.field === "size_over" ? "contains" : condition.op,
      value: condition.value.trim(),
    })),
    actions: rule.actions.map((action) => ({ kind: action.kind, value: actionNeedsValue(action.kind) ? action.value.trim() : "" })),
  }));
}

function validateFilters(filters: MailFilter[]): string | null {
  for (const [index, rule] of filters.entries()) {
    const n = index + 1;
    if (!rule.name) return t("{n}. kuralın bir adı olmalı.", { n });
    if (rule.conditions.length === 0) return t("{n}. kuralda en az bir koşul olmalı.", { n });
    for (const condition of rule.conditions) {
      if (condition.field === "header" && !condition.header) return t("{n}. kuralda başlık adı boş.", { n });
      if (!condition.value) return t("{n}. kuralda boş bir koşul değeri var.", { n });
      if (condition.field === "size_over" && !(Number(condition.value) > 0)) return t("{n}. kuralda boyut, 0'dan büyük bir sayı olmalı.", { n });
    }
    if (rule.actions.length === 0) return t("{n}. kuralda en az bir eylem olmalı.", { n });
    for (const action of rule.actions) {
      if (actionNeedsValue(action.kind) && !action.value) return t("{n}. kuralda klasör adı ya da adres boş.", { n });
    }
  }
  return null;
}

function RuleEditor({
  rule,
  index,
  total,
  onChange,
  onMove,
  onRemove,
}: {
  rule: MailFilter;
  index: number;
  total: number;
  onChange: (next: MailFilter) => void;
  onMove: (delta: -1 | 1) => void;
  onRemove: () => void;
}) {
  const setCondition = (at: number, patch: Partial<FilterCondition>) =>
    onChange({ ...rule, conditions: rule.conditions.map((item, i) => (i === at ? { ...item, ...patch } : item)) });
  const setAction = (at: number, patch: Partial<FilterAction>) =>
    onChange({ ...rule, actions: rule.actions.map((item, i) => (i === at ? { ...item, ...patch } : item)) });

  return (
    <li className={classNames("rounded-lg border border-line-strong", !rule.enabled && "opacity-70")}>
      <div className="flex flex-wrap items-center gap-2 border-b border-line bg-sunken/50 px-3 py-2">
        <span className="tabular w-5 shrink-0 text-center text-[13.5px] font-medium text-ink-3">{index + 1}</span>
        <input
          aria-label={t("Kural adı")}
          value={rule.name}
          onChange={(event) => onChange({ ...rule, name: event.target.value })}
          className={classNames(rowControl, "min-w-40 flex-1 font-medium")}
        />
        <label className="inline-flex items-center gap-1.5 text-[13.5px] text-ink-2">
          <input type="checkbox" className="size-4 accent-(--accent)" checked={rule.enabled} onChange={(event) => onChange({ ...rule, enabled: event.target.checked })} />
          {t("Etkin")}
        </label>
        <div className="flex">
          <IconButton label={t("Yukarı taşı")} disabled={index === 0} onClick={() => onMove(-1)}>
            <ArrowUp className="size-4" />
          </IconButton>
          <IconButton label={t("Aşağı taşı")} disabled={index === total - 1} onClick={() => onMove(1)}>
            <ArrowDown className="size-4" />
          </IconButton>
          <IconButton label={t("Kuralı sil")} onClick={onRemove}>
            <Trash2 className="size-4" />
          </IconButton>
        </div>
      </div>

      <div className="flex flex-col gap-5 p-3">
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2 text-[13.5px]">
            <span className="font-medium text-ink-2">{t("Koşullar")}</span>
            <select
              aria-label={t("Koşulların eşleşme türü")}
              value={rule.match_all ? "all" : "any"}
              onChange={(event) => onChange({ ...rule, match_all: event.target.value === "all" })}
              className={classNames(rowControl, "h-8 pr-7 text-[13.5px]")}
            >
              <option value="all">{t("Koşulların hepsi sağlanırsa")}</option>
              <option value="any">{t("Koşullardan herhangi biri sağlanırsa")}</option>
            </select>
          </div>
          {rule.conditions.map((condition, at) => (
            <div key={at} className="flex flex-wrap items-center gap-2">
              <select
                aria-label={t("Koşul alanı")}
                value={condition.field}
                onChange={(event) => setCondition(at, { field: event.target.value as FilterCondition["field"] })}
                className={classNames(rowControl, "w-40 pr-7")}
              >
                {fieldOrder.map((field) => (
                  <option key={field} value={field}>
                    {t(fieldLabels[field])}
                  </option>
                ))}
              </select>
              {condition.field === "header" && (
                <input
                  aria-label={t("Başlık adı")}
                  placeholder="List-Id"
                  value={condition.header}
                  onChange={(event) => setCondition(at, { header: event.target.value })}
                  className={classNames(rowControl, "w-32 font-mono text-[13.5px]")}
                  spellCheck={false}
                />
              )}
              {condition.field !== "size_over" && (
                <select
                  aria-label={t("Karşılaştırma")}
                  value={condition.op}
                  onChange={(event) => setCondition(at, { op: event.target.value as FilterCondition["op"] })}
                  className={classNames(rowControl, "w-44 pr-7")}
                >
                  {opOrder.map((op) => (
                    <option key={op} value={op}>
                      {t(opLabels[op])}
                    </option>
                  ))}
                </select>
              )}
              {condition.field === "size_over" ? (
                <>
                  <input
                    aria-label={t("Boyut (MB)")}
                    type="number"
                    min={1}
                    step="any"
                    placeholder="10"
                    value={condition.value}
                    onChange={(event) => setCondition(at, { value: event.target.value })}
                    className={classNames(rowControl, "w-28")}
                  />
                  <span className="text-[13.5px] text-ink-2">MB</span>
                </>
              ) : (
                <input
                  aria-label={t("Değer")}
                  value={condition.value}
                  onChange={(event) => setCondition(at, { value: event.target.value })}
                  className={classNames(rowControl, "min-w-32 flex-1")}
                  spellCheck={false}
                />
              )}
              <IconButton
                label={t("Koşulu kaldır")}
                disabled={rule.conditions.length <= 1}
                onClick={() => onChange({ ...rule, conditions: rule.conditions.filter((_, i) => i !== at) })}
              >
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          ))}
          <div>
            <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => onChange({ ...rule, conditions: [...rule.conditions, newCondition()] })}>
              {t("Koşul ekle")}
            </Button>
          </div>
        </div>

        <div className="flex flex-col gap-2 border-t border-line pt-3">
          <span className="text-[13.5px] font-medium text-ink-2">{t("Eylemler")}</span>
          {rule.actions.map((action, at) => (
            <div key={at} className="flex flex-wrap items-center gap-2">
              <select
                aria-label={t("Eylem türü")}
                value={action.kind}
                onChange={(event) => setAction(at, { kind: event.target.value as FilterAction["kind"] })}
                className={classNames(rowControl, "w-44 pr-7")}
              >
                {actionOrder.map((kind) => (
                  <option key={kind} value={kind}>
                    {t(actionLabels[kind])}
                  </option>
                ))}
              </select>
              {actionNeedsValue(action.kind) && (
                <input
                  aria-label={action.kind === "forward" ? t("Adres") : t("Klasör")}
                  placeholder={action.kind === "forward" ? "adres@ornek.com" : t("Faturalar")}
                  value={action.value}
                  onChange={(event) => setAction(at, { value: event.target.value })}
                  className={classNames(rowControl, "min-w-40 flex-1")}
                  autoCapitalize="none"
                  spellCheck={false}
                />
              )}
              <IconButton
                label={t("Eylemi kaldır")}
                disabled={rule.actions.length <= 1}
                onClick={() => onChange({ ...rule, actions: rule.actions.filter((_, i) => i !== at) })}
              >
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          ))}
          <div>
            <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => onChange({ ...rule, actions: [...rule.actions, newAction()] })}>
              {t("Eylem ekle")}
            </Button>
          </div>
        </div>

        <div className="border-t border-line pt-3">
          <Checkbox label={t("Bu kural uyarsa sonraki kurallara bakma")} checked={rule.stop} onChange={(event) => onChange({ ...rule, stop: event.target.checked })} />
        </div>
      </div>
    </li>
  );
}

function FiltersTab({ mailbox }: { mailbox: Mailbox }) {
  const save = useSaveMailbox(mailbox);
  const [filters, setFilters] = useState<MailFilter[]>(mailbox.filters);
  const [localError, setLocalError] = useState<string | null>(null);

  function move(index: number, delta: -1 | 1) {
    setFilters((list) => {
      const target = index + delta;
      if (target < 0 || target >= list.length) return list;
      const next = [...list];
      const [item] = next.splice(index, 1);
      if (item) next.splice(target, 0, item);
      return next;
    });
  }

  function submit() {
    const cleaned = normalizeFilters(filters);
    const problem = validateFilters(cleaned);
    if (problem) {
      setLocalError(problem);
      return;
    }
    setLocalError(null);
    save.mutate({ changes: { filters: cleaned }, message: t("Süzgeç kuralları kaydedildi") });
  }

  return (
    <FormShell onSubmit={submit} saving={save.isPending} error={localError ?? save.error}>
      <p className="max-w-[75ch] text-ink-2">
        {t("Kurallar, ileti geldiğinde yukarıdan aşağıya sırayla çalışır. İstenmeyen postalar bu kurallardan önce Junk klasörüne taşınır.")}
      </p>
      {filters.length === 0 ? (
        <p className="rounded-md border border-dashed border-line-strong px-4 sm:px-6 py-6 text-center text-ink-2">{t("Henüz kural yok. İletileri klasörlere ayıran ilk kuralı ekleyin.")}</p>
      ) : (
        <ol className="flex flex-col gap-3">
          {filters.map((rule, index) => (
            <RuleEditor
              key={index}
              rule={rule}
              index={index}
              total={filters.length}
              onChange={(next) => setFilters((list) => list.map((item, i) => (i === index ? next : item)))}
              onMove={(delta) => move(index, delta)}
              onRemove={() => setFilters((list) => list.filter((_, i) => i !== index))}
            />
          ))}
        </ol>
      )}
      <div>
        <Button icon={<Plus className="size-4" />} onClick={() => setFilters((list) => [...list, newRule(list.length + 1)])}>
          {t("Kural ekle")}
        </Button>
      </div>
    </FormShell>
  );
}

type EditorTab = "general" | "forward" | "reply" | "filters";

function MailboxEditor({ mailbox, onClose }: { mailbox: Mailbox; onClose: () => void }) {
  const [tab, setTab] = useState<EditorTab>("general");
  const tabs: { id: EditorTab; label: string }[] = [
    { id: "general", label: t("Genel") },
    { id: "forward", label: t("Yönlendirme") },
    { id: "reply", label: t("Otomatik yanıt") },
    { id: "filters", label: mailbox.filters.length > 0 ? t("Filtreler ({n})", { n: mailbox.filters.length }) : t("Filtreler") },
  ];
  // Sekmeler bağlı kalır, yalnızca gizlenir; böylece kaydedilmemiş girişler sekme değişince kaybolmaz.
  return (
    <Modal open onClose={onClose} extraWide title={t("Kutuyu düzenle")} description={mailbox.address} footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      <Tabs tabs={tabs} value={tab} onChange={setTab} label={t("Kutu ayarları")} />
      <div hidden={tab !== "general"}>
        <GeneralTab mailbox={mailbox} />
      </div>
      <div hidden={tab !== "forward"}>
        <ForwardTab mailbox={mailbox} />
      </div>
      <div hidden={tab !== "reply"}>
        <AutoReplyTab mailbox={mailbox} />
      </div>
      <div hidden={tab !== "filters"}>
        <FiltersTab mailbox={mailbox} />
      </div>
    </Modal>
  );
}

// ---- Posta kutuları bölümü ----

function CreateMailboxModal({ domain, onClose }: { domain: MailDomain; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const updateCache = useDetailCache(domain.id);
  const [local, setLocal] = useState("");
  const [password, setPassword] = useState("");
  const [quota, setQuota] = useState("1024");

  const create = useMutation({
    mutationFn: () =>
      api.post<Mailbox>(`/mail/domains/${domain.id}/mailboxes`, {
        local: local.trim().toLowerCase(),
        password,
        quota_mb: Number(quota),
      }),
    onSuccess: (created) => {
      updateCache((detail) => ({
        ...detail,
        domain: { ...detail.domain, mailbox_count: detail.domain.mailbox_count + 1 },
        mailboxes: [...detail.mailboxes, created].sort(byAddress),
      }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(t("{address} oluşturuldu", { address: created.address }));
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("Kutu ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-mailbox" loading={create.isPending}>
            {t("Kutuyu oluştur")}
          </Button>
        </>
      }
    >
      <form id="create-mailbox" onSubmit={submit} className="flex flex-col gap-5">
        <LocalPartInput label={t("E-posta adresi")} value={local} onChange={setLocal} domain={domain.domain} autoFocus />
        <PasswordField label={t("Parola")} value={password} onChange={setPassword} hint={t("En az 10 karakter. Rastgele üretirseniz kutunun sahibine güvenli bir yolla iletin.")} required />
        <Input label={t("Kota (MB)")} type="number" min={1} value={quota} onChange={(event) => setQuota(event.target.value)} hint={t("1024 MB = 1 GB")} className="max-w-48" required />
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function DeleteMailboxModal({ mailbox, onClose, onChoose }: { mailbox: Mailbox; onClose: () => void; onChoose: (deleteMail: boolean) => void }) {
  return (
    <Modal
      open
      onClose={onClose}
      title={t("Posta kutusu silinsin mi?")}
      description={mailbox.address}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button onClick={() => onChoose(false)}>{t("Kutuyu sil")}</Button>
          <Button variant="danger" onClick={() => onChoose(true)}>
            {t("Kutuyu ve iletileri sil")}
          </Button>
        </>
      }
    >
      <ul className="flex flex-col gap-3 text-ink-2">
        <li>
          <span className="font-medium text-ink">{t("Kutuyu sil")}</span> {t("kutuyu kaldırır; oturum açılamaz ve yeni posta kabul edilmez. Eski iletiler sunucuda kalır, dilerseniz elle silebilirsiniz.")}
        </li>
        <li>
          <span className="font-medium text-ink">{t("Kutuyu ve iletileri sil")}</span> {t("kutudaki bütün iletileri de kalıcı olarak siler. Bu işlem geri alınamaz.")}
        </li>
      </ul>
    </Modal>
  );
}

function UsageCell({ mailbox }: { mailbox: Mailbox }) {
  if (mailbox.used_bytes === null) return <span className="text-ink-3">—</span>;
  const quotaBytes = mailbox.quota_mb * 1024 * 1024;
  const percent = quotaBytes > 0 ? (mailbox.used_bytes / quotaBytes) * 100 : 0;
  return (
    <div className="w-44">
      <p className="tabular mb-1 text-[13px] text-ink-2">
        {formatBytes(mailbox.used_bytes)} / {quotaBytes > 0 ? formatBytes(quotaBytes) : t("sınırsız")}
      </p>
      <Meter value={percent} label={t("{address} kota kullanımı", { address: mailbox.address })} />
    </div>
  );
}

function MailboxesSection({ detail }: { detail: DomainDetail }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const updateCache = useDetailCache(detail.domain.id);
  const [creating, setCreating] = useState(false);
  const [editingId, setEditingId] = useState<number | null>(null);
  const [deleting, setDeleting] = useState<Mailbox | null>(null);
  const [importing, setImporting] = useState<Mailbox | null>(null);
  const editing = detail.mailboxes.find((item) => item.id === editingId);

  const remove = useMutation({
    mutationFn: (vars: { mailbox: Mailbox; deleteMail: boolean }) => api.delete(`/mail/mailboxes/${vars.mailbox.id}?delete_mail=${vars.deleteMail}`),
    onSuccess: (_, vars) => {
      updateCache((current) => ({
        ...current,
        domain: { ...current.domain, mailbox_count: Math.max(0, current.domain.mailbox_count - 1) },
        mailboxes: current.mailboxes.filter((item) => item.id !== vars.mailbox.id),
      }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(t("{address} silindi", { address: vars.mailbox.address }));
    },
    onError: (error) => feedback.error(error),
  });

  async function chooseDelete(mailbox: Mailbox, deleteMail: boolean) {
    setDeleting(null);
    const confirmed = await feedback.confirm({
      title: t("{address} silinsin mi?", { address: mailbox.address }),
      description: deleteMail
        ? t("Kutu kaldırılır ve içindeki bütün iletiler kalıcı olarak silinir. Bu işlem geri alınamaz.")
        : t("Kutu kaldırılır. İletileri sunucuda kalır; dilediğinizde elle silebilirsiniz."),
      confirmLabel: deleteMail ? t("Kutuyu ve iletileri sil") : t("Kutuyu sil"),
      danger: true,
      typeToConfirm: deleteMail ? mailbox.address : undefined,
    });
    if (confirmed) remove.mutate({ mailbox, deleteMail });
  }

  // Hiçbir kutuda yönlendirme, otomatik yanıt ya da filtre yoksa sütun gösterilmez.
  const anyFeatures = detail.mailboxes.some((mailbox) => mailbox.forward_to.length > 0 || mailbox.auto_reply.enabled || mailbox.filters.length > 0);

  return (
    <>
      <Panel
        flush
        title={t("Posta kutuları")}
        actions={
          <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
            {t("Kutu ekle")}
          </Button>
        }
      >
        {detail.mailboxes.length === 0 ? (
          <EmptyState
            title={t("Henüz posta kutusu yok")}
            description={t("İlk kutuyu oluşturun; sonra DNS kayıtlarını alan adı sağlayıcınıza girerek postayı yayına alın.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
                {t("Kutu ekle")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Adres")}</Th>
                <Th>{t("Kullanım")}</Th>
                <Th>{t("Durum")}</Th>
                <Th>{t("Son saat gönderim")}</Th>
                {anyFeatures && <Th>{t("Özellikler")}</Th>}
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {detail.mailboxes.map((mailbox) => (
                <tr key={mailbox.id} className="hover:bg-sunken/50">
                  <Td className="max-w-72">
                    <p className="truncate">
                      <Mono className="text-[14px] font-medium">{mailbox.address}</Mono>
                    </p>
                  </Td>
                  <Td>
                    <UsageCell mailbox={mailbox} />
                  </Td>
                  <Td className="whitespace-nowrap">{mailbox.enabled ? <Status tone="ok">{t("Etkin")}</Status> : <Status tone="idle">{t("Kapalı")}</Status>}</Td>
                  <Td className="tabular whitespace-nowrap">
                    {mailbox.effective_limit > 0 && mailbox.sent_last_hour >= mailbox.effective_limit ? (
                      <Status tone="bad">
                        {mailbox.sent_last_hour} / {mailbox.effective_limit}
                      </Status>
                    ) : (
                      <span className="text-ink-2">
                        {mailbox.sent_last_hour} / {limitLabel(mailbox.effective_limit)}
                      </span>
                    )}
                  </Td>
                  {anyFeatures && (
                    <Td>
                      <div className="flex flex-wrap gap-1">
                        {mailbox.forward_to.length > 0 && <Badge>{t("Yönlendirme")}</Badge>}
                        {mailbox.auto_reply.enabled && <Badge>{t("Otomatik yanıt")}</Badge>}
                        {mailbox.filters.length > 0 && <Badge>{t("{n} filtre", { n: mailbox.filters.length })}</Badge>}
                      </div>
                    </Td>
                  )}
                  <Td align="right" className="whitespace-nowrap">
                    <div className="flex items-center justify-end gap-1">
                      {detail.webmail && (
                        <a
                          href={webmailUrl(mailbox.address)}
                          target="_blank"
                          rel="noreferrer"
                          title={t("{address} kutusunu web postasında aç", { address: mailbox.address })}
                          className="mr-1 inline-flex h-8 items-center gap-1.5 rounded-lg border border-line-strong bg-surface px-3 text-[13.5px] font-medium shadow-[0_1px_1px_rgb(21_32_43/0.04)] transition-colors hover:bg-sunken"
                        >
                          <MailOpen aria-hidden className="size-4 text-ink-2" />
                          {t("Web postası")}
                        </a>
                      )}
                      <IconButton label={t("Eski sunucudan ileti aktar")} onClick={() => setImporting(mailbox)}>
                        <MailPlus className="size-4" />
                      </IconButton>
                      <IconButton label={t("Düzenle")} onClick={() => setEditingId(mailbox.id)}>
                        <Pencil className="size-4" />
                      </IconButton>
                      <IconButton label={t("Sil")} disabled={remove.isPending} onClick={() => setDeleting(mailbox)}>
                        <Trash2 className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
      {creating && <CreateMailboxModal domain={detail.domain} onClose={() => setCreating(false)} />}
      {editing && <MailboxEditor key={editing.id} mailbox={editing} onClose={() => setEditingId(null)} />}
      {importing && <MailImportModal mailbox={importing} onClose={() => setImporting(null)} />}
      {deleting && <DeleteMailboxModal mailbox={deleting} onClose={() => setDeleting(null)} onChoose={(deleteMail) => void chooseDelete(deleting, deleteMail)} />}
    </>
  );
}

// ---- Takma adlar bölümü ----

function AliasModal({ domain, alias, onClose }: { domain: MailDomain; alias: MailAlias | null; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const updateCache = useDetailCache(domain.id);
  const [local, setLocal] = useState(alias?.local ?? "");
  const [destinations, setDestinations] = useState(alias?.destinations.join("\n") ?? "");
  const [localError, setLocalError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (list: string[]) =>
      alias
        ? api.patch<MailAlias>(`/mail/aliases/${alias.id}`, { destinations: list })
        : api.post<MailAlias>(`/mail/domains/${domain.id}/aliases`, { local: local.trim().toLowerCase(), destinations: list }),
    onSuccess: (saved) => {
      updateCache((detail) => ({
        ...detail,
        domain: alias ? detail.domain : { ...detail.domain, alias_count: detail.domain.alias_count + 1 },
        aliases: alias ? detail.aliases.map((item) => (item.id === saved.id ? saved : item)) : [...detail.aliases, saved].sort(byAddress),
      }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(alias ? t("Takma ad güncellendi") : t("{address} oluşturuldu", { address: saved.address }));
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    const list = linesToList(destinations);
    if (list.length === 0 || list.some((item) => !item.includes("@"))) {
      setLocalError(t("En az bir hedef yazın; hedeflerin tamamı e-posta adresi olmalıdır."));
      return;
    }
    setLocalError(null);
    save.mutate(list);
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={alias ? t("Takma adı düzenle") : t("Takma ad ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="alias-form" loading={save.isPending}>
            {alias ? t("Kaydet") : t("Takma adı oluştur")}
          </Button>
        </>
      }
    >
      <form id="alias-form" onSubmit={submit} className="flex flex-col gap-5">
        <LocalPartInput label={t("Takma ad")} value={local} onChange={setLocal} domain={domain.domain} disabled={alias !== null} autoFocus={alias === null} />
        <Textarea
          label={t("Hedef adresler")}
          value={destinations}
          onChange={(event) => setDestinations(event.target.value)}
          hint={t("Her satıra bir adres yazın. Yerel kutular ya da dış adresler olabilir; ileti hepsine teslim edilir.")}
          placeholder={"ahmet@ornek.com\nmuhasebe@baska-alan.com"}
          spellCheck={false}
          autoFocus={alias !== null}
        />
        {(localError || save.isError) && <ErrorBlock message={localError ?? errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

function AliasesSection({ detail }: { detail: DomainDetail }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const updateCache = useDetailCache(detail.domain.id);
  const [modal, setModal] = useState<{ alias: MailAlias | null } | null>(null);

  const remove = useMutation({
    mutationFn: (alias: MailAlias) => api.delete(`/mail/aliases/${alias.id}`),
    onSuccess: (_, alias) => {
      updateCache((current) => ({
        ...current,
        domain: { ...current.domain, alias_count: Math.max(0, current.domain.alias_count - 1) },
        aliases: current.aliases.filter((item) => item.id !== alias.id),
      }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(t("{address} silindi", { address: alias.address }));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(alias: MailAlias) {
    const confirmed = await feedback.confirm({
      title: t("{address} silinsin mi?", { address: alias.address }),
      description: t("Bu adrese gelen iletiler artık hedeflere iletilmez. Hedef kutulardaki iletilere dokunulmaz."),
      confirmLabel: t("Takma adı sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(alias);
  }

  return (
    <>
      <Panel
        flush
        title={t("Takma adlar")}
        actions={
          <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={() => setModal({ alias: null })}>
            {t("Takma ad ekle")}
          </Button>
        }
      >
        {detail.aliases.length === 0 ? (
          <EmptyState
            title={t("Henüz takma ad yok")}
            description={t("Takma ad, bir adrese gelen iletileri bir ya da birden çok hedefe iletir; örneğin info@ adresini birkaç kişiye dağıtabilirsiniz.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setModal({ alias: null })}>
                {t("Takma ad ekle")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Takma ad")}</Th>
                <Th>{t("Hedefler")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {detail.aliases.map((alias) => (
                <tr key={alias.id} className="align-top hover:bg-sunken/50">
                  <Td className="max-w-64">
                    <p className="truncate">
                      <Mono className="text-[14px] font-medium">{alias.address}</Mono>
                    </p>
                  </Td>
                  <Td>
                    <ul className="flex flex-col gap-0.5">
                      {alias.destinations.map((destination) => (
                        <li key={destination} className="break-all">
                          <Mono>{destination}</Mono>
                        </li>
                      ))}
                    </ul>
                  </Td>
                  <Td align="right" className="whitespace-nowrap">
                    <div className="flex justify-end gap-1">
                      <IconButton label={t("Düzenle")} onClick={() => setModal({ alias })}>
                        <Pencil className="size-4" />
                      </IconButton>
                      <IconButton label={t("Sil")} disabled={remove.isPending} onClick={() => void confirmRemove(alias)}>
                        <Trash2 className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
      {modal && <AliasModal domain={detail.domain} alias={modal.alias} onClose={() => setModal(null)} />}
    </>
  );
}

// ---- DNS bölümü ----

const dnsStatusView: Record<DnsRecord["status"], { tone: Tone; label: string }> = {
  ok: { tone: "ok", label: "Doğru" },
  missing: { tone: "bad", label: "Bulunamadı" },
  mismatch: { tone: "warn", label: "Farklı bir değer var" },
  unknown: { tone: "idle", label: "Denetlenmedi" },
};

function DnsSection({ detail }: { detail: DomainDetail }) {
  // Bölüm ilk açıldığında bir kez denetlenir; sonuçlar kısa süre önbellekte kalır.
  const dns = useQuery({
    queryKey: ["mail-dns", detail.domain.id],
    queryFn: () => api.get<DnsRecord[]>(`/mail/domains/${detail.domain.id}/dns`),
    staleTime: 5 * 60_000,
    refetchOnWindowFocus: false,
    retry: false,
  });
  const records = dns.data ?? detail.dns;
  const feedback = useFeedback();
  const [applied, setApplied] = useState<string[] | null>(null);
  // Bölge panelde yönetiliyorsa kayıtlar tek tıkla eklenir.
  const apply = useMutation({
    mutationFn: () => api.post<{ report: string[] }>(`/mail/domains/${detail.domain.id}/dns/apply`),
    onSuccess: (result) => {
      setApplied(result.report);
      void dns.refetch();
    },
    onError: (error) => feedback.error(error),
  });

  return (
    <Panel
      title={t("DNS kayıtları")}
      actions={
        <Button size="sm" icon={<RefreshCw className="size-3.5" />} loading={dns.isFetching} onClick={() => void dns.refetch()}>
          {t("Kayıtları denetle")}
        </Button>
      }
    >
      <div className="flex flex-col gap-5">
        {detail.dns_managed ? (
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-md bg-accent-soft px-3 py-2.5">
            <p className="max-w-[60ch] text-[13.5px] text-accent">
              {t("Bu alan adının DNS'i panelde yönetiliyor; eksik kayıtları sizin yerinize ekleyebiliriz. Farklı değerdeki var olan kayıtlara dokunulmaz.")}
            </p>
            <Button size="sm" variant="primary" loading={apply.isPending} onClick={() => apply.mutate()}>
              {t("Kayıtları otomatik ekle")}
            </Button>
          </div>
        ) : (
          <p className="rounded-md bg-sunken/70 px-3 py-2.5 text-[13.5px] text-ink-2">
            {t("Alan adının DNS bölgesini panele eklerseniz (bu sunucuda ya da Cloudflare'de) bu kayıtlar kendiliğinden yazılır.")}{" "}
            <Link to="/dns" className="font-medium text-accent hover:underline">
              {t("DNS sayfasına git")}
            </Link>
          </p>
        )}
        {applied && (
          <ul className="rounded-md border border-line px-3 py-2 font-mono text-[13px] text-ink-2">
            {applied.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        )}
        <p className="max-w-[75ch] text-ink-2">
          {t("Aşağıdaki kayıtların {domain} alan adının DNS sağlayıcısında tanımlı olması gerekir; aksi hâlde posta alınamaz ya da gönderilen iletiler istenmeyen posta sayılır. Denetim canlı DNS sorgularıyla yapılır ve birkaç saniye sürebilir.", {
            domain: detail.domain.domain,
          })}
        </p>
        {dns.isError && <ErrorBlock message={errorMessage(dns.error)} onRetry={() => void dns.refetch()} />}
        {records.length === 0 ? (
          <p className="text-ink-2">{t("Bu alan adı için gösterilecek kayıt yok.")}</p>
        ) : (
          <ul className="flex flex-col divide-y divide-line">
            {records.map((record) => {
              const checking = dns.isFetching && record.status === "unknown";
              const view = dnsStatusView[record.status];
              return (
                <li key={`${record.kind}-${record.name}-${record.value}`} className="flex flex-col gap-3 py-4 first:pt-0 last:pb-0">
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                    <Badge tone="accent">{record.kind}</Badge>
                    <p className="min-w-0 flex-1 font-medium">{record.purpose}</p>
                    {checking ? (
                      <span className="inline-flex items-center gap-1.5 text-ink-3">
                        <Spinner className="size-3.5" /> {t("Denetleniyor")}
                      </span>
                    ) : (
                      <Status tone={view.tone}>{t(view.label)}</Status>
                    )}
                  </div>
                  <div className="grid gap-3 lg:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
                    <CopyField label={t("Ad")} value={record.name} multiline />
                    <CopyField label={t("Değer")} value={record.value} multiline />
                  </div>
                  {record.status !== "ok" && record.status !== "unknown" && record.found && (
                    <p className="text-[13px] text-ink-2">
                      {t("Şu an bulunan değer")}: <Mono className="break-all">{record.found}</Mono>
                    </p>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </Panel>
  );
}

// ---- Ayarlar bölümü ----

/** Alan adının toplam saatlik alıcı sınırı (yalnızca yönetici ve bayi). */
function DomainLimitPanel({ detail }: { detail: DomainDetail }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const updateCache = useDetailCache(detail.domain.id);
  const { domain } = detail;
  const [value, setValue] = useState(domain.send_limit === null ? "" : String(domain.send_limit));
  const [localError, setLocalError] = useState<string | null>(null);
  const save = useMutation({
    mutationFn: (limit: number) => api.patch<MailDomain>(`/mail/domains/${domain.id}`, { send_limit: limit }),
    onSuccess: (updated) => {
      updateCache((current) => ({ ...current, domain: updated }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      feedback.success(t("Alan adı gönderim sınırı kaydedildi"));
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    const limit = value.trim() === "" ? -1 : Number(value);
    if (!Number.isInteger(limit) || limit < -1 || limit > 1_000_000) {
      setLocalError(t("Gönderim sınırı, 0 ile 1.000.000 arasında bir tam sayı olmalıdır (boş: varsayılan)."));
      return;
    }
    setLocalError(null);
    save.mutate(limit);
  }

  return (
    <Panel title={t("Alan adı gönderim sınırı")}>
      <form onSubmit={submit} className="flex max-w-xl flex-col gap-4">
        <Input
          label={t("Saatlik toplam alıcı sınırı")}
          type="number"
          min={0}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          placeholder={t("Varsayılan")}
          hint={t("Alan adındaki bütün kutuların birlikte saatte gönderebileceği alıcı sayısı. Boş: varsayılan ({limit}); 0: sınırsız. Son bir saatte: {sent}.", {
            limit: limitLabel(domain.effective_limit),
            sent: domain.sent_last_hour,
          })}
          className="max-w-48"
        />
        {localError && <ErrorBlock message={localError} />}
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending}>
            {t("Kaydet")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function SettingsSection({ detail }: { detail: DomainDetail }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const updateCache = useDetailCache(detail.domain.id);
  const { domain } = detail;
  const canSetSendLimit = useCanSetSendLimit();
  const [catchAll, setCatchAll] = useState(domain.catch_all);

  const saveCatchAll = useMutation({
    mutationFn: () => api.patch<MailDomain>(`/mail/domains/${domain.id}`, { catch_all: catchAll.trim() }),
    onSuccess: (updated) => {
      updateCache((current) => ({ ...current, domain: updated }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      setCatchAll(updated.catch_all);
      feedback.success(updated.catch_all ? t("Hepsini yakala adresi kaydedildi") : t("Hepsini yakala adresi kaldırıldı"));
    },
  });

  const remove = useMutation({
    mutationFn: (deleteMail: boolean) => api.delete(`/mail/domains/${domain.id}?delete_mail=${deleteMail}`),
    onSuccess: () => {
      feedback.success(t("{domain} için posta kaldırıldı", { domain: domain.domain }));
      void queryClient.invalidateQueries({ queryKey: ["mail"] });
      void navigate("/mail");
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(deleteMail: boolean) {
    const confirmed = await feedback.confirm({
      title: t("{domain} için posta kaldırılsın mı?", { domain: domain.domain }),
      description: deleteMail
        ? t("Alan adının bütün kutuları ve takma adları kaldırılır; kutulardaki iletiler kalıcı olarak silinir. Bu işlem geri alınamaz.")
        : t("Alan adının bütün kutuları ve takma adları kaldırılır. İletileri sunucuda kalır; dilediğinizde elle silebilirsiniz."),
      confirmLabel: deleteMail ? t("Postayı ve iletileri sil") : t("Postayı kaldır"),
      danger: true,
      typeToConfirm: deleteMail ? domain.domain : undefined,
    });
    if (confirmed) remove.mutate(deleteMail);
  }

  function submitCatchAll(event: FormEvent) {
    event.preventDefault();
    saveCatchAll.mutate();
  }

  return (
    <div className="flex flex-col gap-5">
      <Panel title={t("Hepsini yakala adresi")}>
        <form onSubmit={submitCatchAll} className="flex max-w-xl flex-col gap-4">
          <Input
            label={t("Hepsini yakala")}
            value={catchAll}
            onChange={(event) => setCatchAll(event.target.value)}
            placeholder={`info@${domain.domain}`}
            hint={t("Var olmayan adreslere gelen iletiler bu adrese teslim edilir. Boş bırakılırsa bu iletiler reddedilir.")}
            className="font-mono text-[13.5px]"
            autoCapitalize="none"
            spellCheck={false}
          />
          {saveCatchAll.isError && <ErrorBlock message={errorMessage(saveCatchAll.error)} />}
          <div>
            <Button type="submit" variant="primary" loading={saveCatchAll.isPending}>
              {t("Kaydet")}
            </Button>
          </div>
        </form>
      </Panel>

      {canSetSendLimit && <DomainLimitPanel detail={detail} />}

      <Panel title={t("Postayı kaldır")}>
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="max-w-[60ch]">
            <p className="font-medium">{t("Bu alan adı için postayı kaldır")}</p>
            <p className="text-[13.5px] text-ink-2">{t("Kutular ve takma adlar silinir. Kutulardaki iletileri saklamayı ya da birlikte silmeyi seçebilirsiniz.")}</p>
          </div>
          <div className="flex gap-2">
            <Button loading={remove.isPending} onClick={() => void confirmRemove(false)}>
              {t("Postayı kaldır")}
            </Button>
            <Button variant="danger" loading={remove.isPending} onClick={() => void confirmRemove(true)}>
              {t("Postayı ve iletileri sil")}
            </Button>
          </div>
        </div>
      </Panel>
    </div>
  );
}

// ---- Sayfa ----

type SectionId = "kutular" | "takma-adlar" | "listeler" | "dns" | "istemci" | "ayarlar";

interface Section {
  id: SectionId;
  label: string;
  icon: LucideIcon;
  count?: number;
}

export function MailDomainPage() {
  const me = useMe();
  const { id } = useParams();
  const [searchParams] = useSearchParams();
  const detail = useQuery({ queryKey: detailKey(id ?? ""), queryFn: () => api.get<DomainDetail>(`/mail/domains/${id}`), enabled: Boolean(id) });
  // Yan menüdeki sayı için; Listeler bölümüyle aynı sorgu anahtarını paylaşır.
  const lists = useQuery({ queryKey: ["mail-lists", Number(id)], queryFn: () => api.get<unknown[]>(`/mail/domains/${id}/lists`), enabled: Boolean(id) });

  if (detail.isPending) return <LoadingBlock />;
  if (detail.isError) return <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />;

  const data = detail.data;
  const sections: Section[] = [
    { id: "kutular", label: t("Kutular"), icon: Inbox, count: data.domain.mailbox_count },
    { id: "takma-adlar", label: t("Takma adlar"), icon: AtSign, count: data.domain.alias_count },
    { id: "listeler", label: t("Listeler"), icon: Users, count: lists.data?.length },
    { id: "dns", label: t("DNS kayıtları"), icon: Globe },
    { id: "istemci", label: t("İstemci ayarları"), icon: Smartphone },
    { id: "ayarlar", label: t("Ayarlar"), icon: SlidersHorizontal },
  ];
  const requested = searchParams.get("bolum");
  const active: SectionId = sections.find((item) => item.id === requested)?.id ?? "kutular";

  return (
    <>
      <Link to="/mail" className="mb-3 inline-flex items-center gap-1.5 text-[13.5px] font-medium text-ink-2 hover:text-ink">
        <ArrowLeft className="size-3.5" /> {t("Posta")}
      </Link>
      <header className="mb-5 flex flex-wrap items-center gap-x-4 gap-y-2">
        <h1 className="font-mono text-[22px] leading-tight font-medium tracking-[-0.01em] break-all">{data.domain.domain}</h1>
        <Badge>{t("{n} kutu", { n: data.domain.mailbox_count })}</Badge>
        <Badge>{t("{n} takma ad", { n: data.domain.alias_count })}</Badge>
        {data.domain.catch_all && <Badge tone="accent">{t("Hepsini yakala")}</Badge>}
        <span className="text-[13.5px] text-ink-3">{t("Oluşturulma")}: {formatDateTime(data.domain.created_at)}</span>
        {data.webmail ? (
          <a
            href={data.webmail_host ? `https://${data.webmail_host}/webmail/` : webmailUrl()}
            target="_blank"
            rel="noreferrer"
            className="ml-auto inline-flex h-9.5 items-center gap-2 rounded-lg border border-line-strong bg-surface px-4 font-medium shadow-[0_1px_1px_rgb(21_32_43/0.04)] transition-colors hover:bg-sunken max-sm:w-full max-sm:justify-center"
          >
            <MailOpen aria-hidden className="size-4 text-ink-2" />
            {t("Web postasını aç")}
            <ExternalLink aria-hidden className="size-3.5 text-ink-3" />
          </a>
        ) : (
          me.role === "admin" && (
            <Link to="/components" className="ml-auto text-[13.5px] font-medium text-accent hover:underline max-sm:ml-0">
              {t("Web postası kurulu değil; Bileşenler sayfasından kurun")}
            </Link>
          )
        )}
      </header>

      <div className="flex flex-col gap-5 lg:flex-row lg:items-start">
        <nav aria-label={t("Posta bölümleri")} className="no-scrollbar -mx-1 flex gap-1 overflow-x-auto px-1 lg:sticky lg:top-18 lg:mx-0 lg:w-52 lg:shrink-0 lg:flex-col lg:gap-px lg:overflow-visible lg:px-0">
          {sections.map((item) => (
            <Link
              key={item.id}
              to={item.id === "kutular" ? `/mail/${data.domain.id}` : `/mail/${data.domain.id}?bolum=${item.id}`}
              aria-current={active === item.id ? "page" : undefined}
              className={classNames(
                "flex h-9 shrink-0 items-center gap-2.5 rounded-md px-2.5 font-medium whitespace-nowrap transition-colors",
                active === item.id ? "bg-accent-soft text-accent" : "text-ink-2 hover:bg-sunken hover:text-ink",
              )}
            >
              <item.icon className="size-4 shrink-0" />
              {item.label}
              {item.count !== undefined && <span className="tabular ml-auto pl-2 text-[13px] font-normal text-ink-3">{item.count}</span>}
            </Link>
          ))}
        </nav>
        <div className="min-w-0 flex-1">
          {active === "kutular" && <MailboxesSection detail={data} />}
          {active === "takma-adlar" && <AliasesSection detail={data} />}
          {active === "listeler" && <MailLists domain={data.domain} />}
          {active === "dns" && <DnsSection detail={data} />}
          {active === "istemci" && <MailClientSettings domain={data.domain} />}
          {active === "ayarlar" && <SettingsSection key={data.domain.catch_all} detail={data} />}
        </div>
      </div>
    </>
  );
}
