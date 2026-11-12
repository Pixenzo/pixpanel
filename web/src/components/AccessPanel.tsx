// Hesabın SFTP ve kabuk (SSH) erişimi: erişim düzeyi, bağlantı bilgisi, açık
// anahtarlar ve SFTP parolası. Hesabım sayfasında hesabın kendisi, Kullanıcılar
// sayfasında hesabı yöneten kullanır.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, KeyRound, Plus, SquareTerminal, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link } from "react-router";
import type { AccountAccess } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames, formatRelative, generatePassword } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Button, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Textarea } from "./ui";

const MODES: { id: AccountAccess["mode"]; label: string; hint: string }[] = [
  { id: "none", label: "Kapalı", hint: "Hesap sunucuya SSH ya da SFTP ile bağlanamaz." },
  { id: "sftp", label: "Yalnızca SFTP", hint: "Dosya aktarımı; hesap kendi klasörüne hapsedilir, komut çalıştıramaz." },
  { id: "shell", label: "Kabuk (SSH)", hint: "Komut satırı erişimi. Yalnızca güvendiğiniz hesaplara verin; yalnızca anahtarla girilir." },
];

function AddKeyModal({ accountId, onClose }: { accountId: number; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [publicKey, setPublicKey] = useState("");
  const add = useMutation({
    mutationFn: () => api.post<AccountAccess>(`/users/${accountId}/access/keys`, { name, public_key: publicKey }),
    onSuccess: (access) => {
      queryClient.setQueryData(["access", accountId], access);
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    add.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={t("Açık anahtar ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="add-ssh-key" loading={add.isPending}>
            {t("Anahtarı ekle")}
          </Button>
        </>
      }
    >
      <form id="add-ssh-key" onSubmit={submit} className="flex flex-col gap-5">
        <Textarea
          label={t("Açık anahtar")}
          value={publicKey}
          onChange={(event) => setPublicKey(event.target.value)}
          placeholder="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAA… kullanici@bilgisayar"
          hint={t("Bilgisayarınızdaki ~/.ssh/id_ed25519.pub dosyasının içeriği. Özel anahtarı (.pub uzantısı olmayan dosyayı) asla yapıştırmayın.")}
          spellCheck={false}
          autoFocus
          required
        />
        <Input label={t("Ad")} value={name} onChange={(event) => setName(event.target.value)} placeholder={t("Dizüstü bilgisayarım")} hint={t("Anahtarı sonradan tanımanız için; boş bırakırsanız anahtarın kendi notu kullanılır.")} maxLength={60} />
        {add.isError && <ErrorBlock message={errorMessage(add.error)} />}
      </form>
    </Modal>
  );
}

function PasswordForm({ accountId }: { accountId: number }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [password, setPassword] = useState("");
  const save = useMutation({
    mutationFn: () => api.put<AccountAccess>(`/users/${accountId}/access/password`, { password }),
    onSuccess: (access) => {
      queryClient.setQueryData(["access", accountId], access);
      feedback.success(t("SFTP parolası belirlendi"));
      setPassword("");
    },
    onError: (error) => feedback.error(error),
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <form onSubmit={submit} className="flex flex-wrap items-end gap-2">
      <div className="min-w-0 flex-1 basis-56">
        <Input
          label={t("Yeni SFTP parolası")}
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          className="font-mono"
          autoComplete="off"
          minLength={12}
          hint={t("En az 12 karakter. Parola yerine açık anahtar kullanmanız daha güvenlidir.")}
          required
        />
      </div>
      <div className="mb-[26px] flex gap-2">
        <Button onClick={() => setPassword(generatePassword(24))}>{t("Üret")}</Button>
        <Button type="submit" variant="primary" loading={save.isPending}>
          {t("Parolayı belirle")}
        </Button>
      </div>
    </form>
  );
}

export function AccessPanel({ accountId }: { accountId: number }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [adding, setAdding] = useState(false);
  const queryKey = ["access", accountId];
  const access = useQuery({ queryKey, queryFn: () => api.get<AccountAccess>(`/users/${accountId}/access`) });

  const setMode = useMutation({
    mutationFn: (mode: AccountAccess["mode"]) => api.put<AccountAccess>(`/users/${accountId}/access`, { mode }),
    onSuccess: (next) => queryClient.setQueryData(queryKey, next),
    onError: (error) => feedback.error(error),
  });
  const removeKey = useMutation({
    mutationFn: (keyId: number) => api.delete<AccountAccess>(`/users/${accountId}/access/keys/${keyId}`),
    onSuccess: (next) => queryClient.setQueryData(queryKey, next),
    onError: (error) => feedback.error(error),
  });

  if (access.isPending) return <LoadingBlock />;
  // Alt hesapların (ve erişilemeyen hesapların) SSH erişimi olmaz; panel hiç gösterilmez.
  if (access.isError) return null;
  const data = access.data;
  const host = window.location.hostname;
  const command = data.mode === "shell" ? `ssh -p ${data.port} ${data.username}@${host}` : `sftp -P ${data.port} ${data.username}@${host}`;

  return (
    <Panel title={t("SFTP ve SSH erişimi")}>
      {!data.available ? (
        <p className="text-ink-2">{t("Bu sunucuda SSH sunucusu kurulu değil.")}</p>
      ) : (
        <div className="flex flex-col gap-5">
          {data.can_change_mode ? (
            <fieldset disabled={setMode.isPending}>
              <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Erişim düzeyi")}</legend>
              <div className="grid gap-2 sm:grid-cols-3">
                {MODES.filter((mode) => mode.id !== "shell" || data.can_grant_shell || data.mode === "shell").map((mode) => (
                  <label
                    key={mode.id}
                    className={classNames(
                      "flex cursor-pointer flex-col gap-1 rounded-lg border p-3 transition-colors",
                      data.mode === mode.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                    )}
                  >
                    <span className="flex items-center gap-2 font-medium">
                      <input type="radio" name={`access-mode-${accountId}`} className="accent-(--accent)" checked={data.mode === mode.id} onChange={() => setMode.mutate(mode.id)} />
                      {t(mode.label)}
                    </span>
                    <span className="text-[13px] text-ink-2">{t(mode.hint)}</span>
                  </label>
                ))}
              </div>
            </fieldset>
          ) : (
            <p className="text-ink-2">
              {data.mode === "none"
                ? t("Bu hesap için SFTP ve SSH erişimi kapalı. Açılmasını sunucu yöneticinizden isteyebilirsiniz; açık anahtarınızı şimdiden ekleyebilirsiniz.")
                : data.mode === "sftp"
                  ? t("Bu hesap SFTP ile dosya aktarabilir.")
                  : t("Bu hesap SSH ile komut satırına bağlanabilir.")}
            </p>
          )}

          {data.mode !== "none" && (
            <div>
              <p className="mb-1.5 text-[13.5px] font-medium text-ink-2">{t("Bağlantı")}</p>
              <div className="flex items-center gap-1 rounded-md border border-line bg-sunken/60 py-1 pr-1 pl-3">
                <Mono className="min-w-0 flex-1 break-all">{command}</Mono>
                <IconButton label={t("Komutu kopyala")} onClick={() => void navigator.clipboard.writeText(command).then(() => feedback.success(t("Kopyalandı")))}>
                  <Copy className="size-4" />
                </IconButton>
              </div>
              {data.mode === "shell" && (data.can_grant_shell || !data.can_change_mode) && (
                <div className="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1">
                  <Link to={`/terminal?kapsam=${encodeURIComponent(`user:${accountId}`)}`} className="inline-flex items-center gap-1.5 text-[13.5px] font-medium text-accent hover:underline">
                    <SquareTerminal aria-hidden className="size-4" />
                    {t("Web terminalini aç")}
                  </Link>
                  <span className="text-[13px] text-ink-3">{t("Kabuk hesabınızın kaynak diliminde ve kullanıcısıyla açılır; en çok 3 oturum.")}</span>
                </div>
              )}
              {data.mode === "sftp" && (
                <p className="mt-1.5 text-[13px] text-ink-3">
                  {t("FileZilla gibi programlarda: protokol SFTP, sunucu {host}, port {port}, kullanıcı {user}.", { host, port: data.port, user: data.username })}
                </p>
              )}
            </div>
          )}

          <div>
            <div className="mb-2 flex items-center justify-between gap-3">
              <p className="text-[13.5px] font-medium text-ink-2">{t("Açık anahtarlar")}</p>
              <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setAdding(true)}>
                {t("Anahtar ekle")}
              </Button>
            </div>
            {data.keys.length === 0 ? (
              <p className="text-ink-2">{t("Henüz anahtar eklenmemiş. Anahtarla giriş paroladan daha güvenlidir.")}</p>
            ) : (
              <ul className="divide-y divide-line rounded-lg border border-line">
                {data.keys.map((key) => (
                  <li key={key.id} className="flex items-center gap-3 px-3 py-2">
                    <KeyRound aria-hidden className="size-4 shrink-0 text-ink-3" />
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-medium">{key.name}</p>
                      <p className="truncate font-mono text-[12.5px] text-ink-3">{key.fingerprint}</p>
                    </div>
                    <span className="shrink-0 text-[13px] text-ink-3 max-sm:hidden">{formatRelative(key.created_at)}</span>
                    <IconButton label={t("{name} anahtarını sil", { name: key.name })} disabled={removeKey.isPending} onClick={() => removeKey.mutate(key.id)}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  </li>
                ))}
              </ul>
            )}
          </div>

          {data.mode === "sftp" && (
            <div>
              <p className="mb-2 text-[13.5px] text-ink-2">
                {data.password_set ? t("Bu hesabın bir SFTP parolası var. Yenisini belirlerseniz eskisi geçersiz olur.") : t("Parolayla da bağlanmak isterseniz bir SFTP parolası belirleyin.")}
              </p>
              <PasswordForm accountId={accountId} />
            </div>
          )}
        </div>
      )}
      {adding && <AddKeyModal accountId={accountId} onClose={() => setAdding(false)} />}
    </Panel>
  );
}
