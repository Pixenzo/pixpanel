import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { AccessPanel } from "../components/AccessPanel";
import { NotificationSettings } from "../components/NotificationSettings";
import { WebhookSettings } from "../components/WebhookSettings";
import { TwoFactorPanel } from "../components/TwoFactorPanel";
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  Modal,
  Mono,
  PageHeader,
  Panel,
  Select,
  Table,
  Td,
  Th,
} from "../components/ui";
import type { SessionInfo, TokenInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { UsagePanel } from "./Packages";

function LanguagePanel() {
  const me = useMe();
  const queryClient = useQueryClient();
  const [language, setLanguage] = useState(me.language === "en" ? "en" : "tr");

  const save = useMutation({
    mutationFn: () => api.patch(`/users/${me.id}`, { language }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["me"] });
      // `t` çağıran bileşenlerin hepsi yeni dille çizilsin diye sayfa yeniden yüklenir.
      window.location.reload();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Panel title={t("Dil")}>
      <form onSubmit={submit} className="flex max-w-sm flex-col gap-4">
        <Select label={t("Arayüz dili")} value={language} onChange={(e) => setLanguage(e.target.value)}>
          <option value="tr">Türkçe</option>
          <option value="en">English</option>
        </Select>
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending} disabled={language === me.language}>
            {t("Dili kaydet")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function PasswordPanel() {
  const feedback = useFeedback();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const mismatch = repeat.length > 0 && next !== repeat;

  const change = useMutation({
    mutationFn: () => api.post("/auth/password", { current_password: current, new_password: next }),
    onSuccess: () => {
      feedback.success(t("Parola değiştirildi; diğer oturumlar kapatıldı"));
      setCurrent("");
      setNext("");
      setRepeat("");
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    if (!mismatch) change.mutate();
  }

  return (
    <Panel title={t("Parola")}>
      <form onSubmit={submit} className="flex max-w-sm flex-col gap-4">
        <Input label={t("Mevcut parola")} type="password" value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="current-password" required />
        <Input
          label={t("Yeni parola")}
          type="password"
          value={next}
          onChange={(e) => setNext(e.target.value)}
          hint={t("En az 10 karakter")}
          autoComplete="new-password"
          minLength={10}
          required
        />
        <Input
          label={t("Yeni parola (yeniden)")}
          type="password"
          value={repeat}
          onChange={(e) => setRepeat(e.target.value)}
          error={mismatch ? t("Parolalar birbirini tutmuyor") : undefined}
          autoComplete="new-password"
          required
        />
        {change.isError && <ErrorBlock message={errorMessage(change.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={change.isPending}>
            {t("Parolayı değiştir")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function SessionsPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const sessions = useQuery({ queryKey: ["sessions"], queryFn: () => api.get<SessionInfo[]>("/auth/sessions") });
  const revoke = useMutation({
    mutationFn: (id: number) => api.delete(`/auth/sessions/${id}`),
    onSuccess: () => feedback.success(t("Oturum kapatıldı")),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["sessions"] }),
  });

  return (
    <Panel flush title={t("Açık oturumlar")}>
      <Table>
        <thead>
          <tr>
            <Th>{t("IP adresi")}</Th>
            <Th className="w-full">{t("Tarayıcı")}</Th>
            <Th>{t("Açılış")}</Th>
            <Th />
          </tr>
        </thead>
        <tbody>
          {(sessions.data ?? []).map((session) => (
            <tr key={session.id}>
              <Td>
                <Mono>{session.ip}</Mono>
              </Td>
              <Td className="max-w-0">
                <span className="block truncate text-ink-2" title={session.user_agent}>
                  {session.user_agent || t("Bilinmiyor")}
                </span>
              </Td>
              <Td className="whitespace-nowrap text-ink-2">{formatRelative(session.created_at)}</Td>
              <Td align="right" className="py-0.5 whitespace-nowrap">
                {session.current ? (
                  <Badge tone="accent">{t("Bu oturum")}</Badge>
                ) : (
                  <Button size="sm" onClick={() => revoke.mutate(session.id)}>
                    {t("Oturumu kapat")}
                  </Button>
                )}
              </Td>
            </tr>
          ))}
        </tbody>
      </Table>
    </Panel>
  );
}

function CreateTokenModal({ open, onClose }: { open: boolean; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [readOnly, setReadOnly] = useState(false);
  const [allowedIps, setAllowedIps] = useState("");
  const [expiry, setExpiry] = useState("0");
  const [created, setCreated] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: () =>
      api.post<{ token: string }>("/auth/tokens", {
        name,
        read_only: readOnly,
        allowed_ips: allowedIps,
        expires_in_days: Number(expiry) || null,
      }),
    onSuccess: (result) => {
      setCreated(result.token);
      void queryClient.invalidateQueries({ queryKey: ["tokens"] });
    },
  });

  function close() {
    setName("");
    setReadOnly(false);
    setAllowedIps("");
    setExpiry("0");
    setCreated(null);
    create.reset();
    onClose();
  }

  async function copy() {
    if (!created) return;
    await navigator.clipboard.writeText(created);
    feedback.success(t("Anahtar panoya kopyalandı"));
  }

  if (created) {
    return (
      <Modal open={open} onClose={close} title={t("Anahtarınız hazır")} footer={<Button variant="primary" onClick={close}>{t("Anahtarı kaydettim")}</Button>}>
        <div className="flex flex-col gap-3">
          <p className="text-ink-2">{t("Bu anahtar yalnızca şimdi gösterilir. Güvenli bir yere kaydedin; kaybederseniz yenisini oluşturmanız gerekir.")}</p>
          <div className="flex items-center gap-2 rounded-md border border-line bg-sunken p-2.5">
            <code className="min-w-0 flex-1 font-mono text-[13px] break-all">{created}</code>
            <IconButton label={t("Kopyala")} onClick={() => void copy()}>
              <Copy className="size-4" />
            </IconButton>
          </div>
          <p className="text-[13px] text-ink-3">
            {t("Kullanım:")} <code className="font-mono">Authorization: Bearer {created.slice(0, 8)}…</code>
          </p>
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      open={open}
      onClose={close}
      title={t("API anahtarı oluştur")}
      footer={
        <>
          <Button onClick={close}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-token" loading={create.isPending}>
            {t("Anahtarı oluştur")}
          </Button>
        </>
      }
    >
      <form
        id="create-token"
        onSubmit={(event) => {
          event.preventDefault();
          create.mutate();
        }}
        className="flex flex-col gap-5"
      >
        <Input label={t("Ad")} value={name} onChange={(e) => setName(e.target.value)} hint={t("Anahtarı nerede kullandığınızı hatırlatacak bir ad")} maxLength={64} required />
        <Select label={t("Geçerlilik")} value={expiry} onChange={(e) => setExpiry(e.target.value)}>
          <option value="0">{t("Süresiz")}</option>
          <option value="30">{t("30 gün")}</option>
          <option value="90">{t("90 gün")}</option>
          <option value="365">{t("1 yıl")}</option>
        </Select>
        <Input
          label={t("İzinli IP adresleri")}
          value={allowedIps}
          onChange={(e) => setAllowedIps(e.target.value)}
          hint={t("Virgülle ayırın. Boş bırakırsanız anahtar her adresten kullanılabilir.")}
          placeholder="203.0.113.10, 2001:db8::1"
          spellCheck={false}
        />
        <Checkbox
          label={t("Salt okunur")}
          hint={t("Bu anahtarla yalnızca bilgi okunabilir, değişiklik yapılamaz.")}
          checked={readOnly}
          onChange={(e) => setReadOnly(e.target.checked)}
        />
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function TokensPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const tokens = useQuery({ queryKey: ["tokens"], queryFn: () => api.get<TokenInfo[]>("/auth/tokens") });
  const remove = useMutation({
    mutationFn: (token: TokenInfo) => api.delete(`/auth/tokens/${token.id}`),
    onSuccess: (_, token) => feedback.success(t("{name} anahtarı silindi", { name: token.name })),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["tokens"] }),
  });

  async function confirmRemove(token: TokenInfo) {
    const confirmed = await feedback.confirm({
      title: t("{name} anahtarı silinsin mi?", { name: token.name }),
      description: t("Bu anahtarı kullanan betikler ve uygulamalar panele erişemez hâle gelir."),
      confirmLabel: t("Anahtarı sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(token);
  }

  return (
    <Panel
      flush
      title={t("API anahtarları")}
      actions={
        <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
          {t("API anahtarı oluştur")}
        </Button>
      }
    >
      {tokens.data?.length === 0 ? (
        <EmptyState
          title={t("Henüz API anahtarı yok")}
          description={t("Paneli betiklerinizden ya da başka sistemlerden yönetmek için bir anahtar oluşturun.")}
        />
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>{t("Ad")}</Th>
              <Th>{t("Anahtar")}</Th>
              <Th>{t("Kısıtlar")}</Th>
              <Th>{t("Son kullanım")}</Th>
              <Th>{t("Bitiş")}</Th>
              <Th />
            </tr>
          </thead>
          <tbody>
            {(tokens.data ?? []).map((token) => (
              <tr key={token.id}>
                <Td className="font-medium">{token.name}</Td>
                <Td>
                  <Mono className="text-ink-2">{token.prefix}…</Mono>
                </Td>
                <Td>
                  <div className="flex flex-wrap gap-1">
                    {token.read_only && <Badge>{t("Salt okunur")}</Badge>}
                    {token.allowed_ips && <Badge>{token.allowed_ips}</Badge>}
                    {!token.read_only && !token.allowed_ips && <span className="text-ink-3">{t("Yok")}</span>}
                  </div>
                </Td>
                <Td className="whitespace-nowrap text-ink-2">{formatRelative(token.last_used_at)}</Td>
                <Td className="whitespace-nowrap text-ink-2">{token.expires_at ? formatDateTime(token.expires_at) : t("Süresiz")}</Td>
                <Td align="right" className="py-0.5">
                  <IconButton label={t("Sil")} onClick={() => void confirmRemove(token)}>
                    <Trash2 className="size-4" />
                  </IconButton>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <CreateTokenModal open={creating} onClose={() => setCreating(false)} />
    </Panel>
  );
}

export function AccountPage() {
  const me = useMe();
  const role = me.role;
  return (
    <>
      <PageHeader title={t("Hesabım")} description={t("Parolanız, iki adımlı doğrulamanız, bildirim tercihleriniz, açık oturumlarınız ve API anahtarlarınız.")} />
      <div className="flex flex-col gap-5">
        {/* Yöneticinin sınırı yoktur; öteki hesaplar paketlerini ve ne kadarını kullandıklarını görür. */}
        {role !== "admin" && <UsagePanel path="/packages/usage" />}
        <PasswordPanel />
        <AccessPanel accountId={me.id} />
        <TwoFactorPanel />
        <NotificationSettings />
        {role !== "member" && <WebhookSettings />}
        <LanguagePanel />
        <SessionsPanel />
        <TokensPanel />
      </div>
    </>
  );
}
