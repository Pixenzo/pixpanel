// Site çalışma alanının "FTP hesapları" bölümü.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, KeyRound, Plus, Trash2, Wand2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Table, Td, Th } from "../components/ui";
import type { Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

interface FtpAccount {
  id: number;
  name: string;
  subdir: string;
  created_at: number;
}

interface FtpView {
  installed: boolean;
  running: boolean;
  accounts: FtpAccount[];
  passive_ports: [number, number];
}

/** Karışabilen karakterler (0/O, 1/l) dışarıda bırakılır. */
function randomPassword(): string {
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789";
  const bytes = crypto.getRandomValues(new Uint8Array(18));
  return Array.from(bytes, (byte) => alphabet[byte % alphabet.length]).join("");
}

function PasswordInput({ value, onChange, label, hint }: { value: string; onChange: (next: string) => void; label: string; hint?: string }) {
  return (
    <div className="flex items-end gap-2">
      <div className="min-w-0 flex-1">
        <Input label={label} value={value} onChange={(event) => onChange(event.target.value)} className="font-mono text-[13px]" autoComplete="off" spellCheck={false} required hint={hint} />
      </div>
      <div className={hint ? "mb-[1.6rem] flex gap-1" : "flex gap-1"}>
        <IconButton label={t("Rastgele üret")} onClick={() => onChange(randomPassword())}>
          <Wand2 className="size-4" />
        </IconButton>
        <IconButton label={t("Kopyala")} onClick={() => void navigator.clipboard.writeText(value).catch(() => {})}>
          <Copy className="size-4" />
        </IconButton>
      </div>
    </div>
  );
}

function AccountModal({ site, account, onClose, onSaved }: { site: Site; account: FtpAccount | null; onClose: () => void; onSaved: (view: FtpView) => void }) {
  const feedback = useFeedback();
  const [name, setName] = useState("");
  const [password, setPassword] = useState(randomPassword);
  const [subdir, setSubdir] = useState(account?.subdir ?? "");
  const save = useMutation({
    mutationFn: () =>
      account
        ? api.patch<FtpView>(`/sites/${site.id}/ftp/${account.id}`, { password, subdir })
        : api.post<FtpView>(`/sites/${site.id}/ftp`, { name, password, subdir }),
    onSuccess: (view) => {
      feedback.success(account ? t("{name} güncellendi", { name: account.name }) : t("{name} hesabı oluşturuldu", { name }));
      onSaved(view);
      onClose();
    },
  });
  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }
  return (
    <Modal
      open
      onClose={onClose}
      title={account ? t("{name} hesabını düzenle", { name: account.name }) : t("FTP hesabı oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="ftp-account" loading={save.isPending}>
            {account ? t("Kaydet") : t("Hesabı oluştur")}
          </Button>
        </>
      }
    >
      <form id="ftp-account" onSubmit={submit} className="flex flex-col gap-4">
        {!account && (
          <Input
            label={t("Kullanıcı adı")}
            value={name}
            onChange={(event) => setName(event.target.value.toLowerCase())}
            placeholder={`${site.owner}_ftp`}
            hint={t("Sunucu genelinde benzersiz olmalı; küçük harf, rakam, _ - . kullanın.")}
            className="font-mono text-[13px]"
            autoComplete="off"
            spellCheck={false}
            autoFocus
            required
          />
        )}
        <PasswordInput
          label={account ? t("Yeni parola") : t("Parola")}
          value={password}
          onChange={setPassword}
          hint={t("Parolayı şimdi kopyalayın; sonradan gösterilmez, yalnızca değiştirilebilir.")}
        />
        <Input
          label={t("Erişebileceği klasör")}
          value={subdir}
          onChange={(event) => setSubdir(event.target.value)}
          placeholder={t("boş: sitenin bütün klasörü")}
          hint={t("Hesap bu klasörün dışına çıkamaz. Örnek: {dir}. Klasör önceden var olmalı.", { dir: site.kind === "node" ? "public" : `${site.public_dir}/yuklemeler` })}
          className="font-mono text-[13px]"
          spellCheck={false}
        />
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

export function FtpSection({ site }: { site: Site }) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [dialog, setDialog] = useState<{ account: FtpAccount | null } | null>(null);
  const queryKey = ["site-ftp", site.id];
  const ftp = useQuery({ queryKey, queryFn: () => api.get<FtpView>(`/sites/${site.id}/ftp`) });
  const apply = (view: FtpView) => queryClient.setQueryData(queryKey, view);
  const remove = useMutation({
    mutationFn: (account: FtpAccount) => api.delete<FtpView>(`/sites/${site.id}/ftp/${account.id}`),
    onSuccess: (view, account) => {
      apply(view);
      feedback.success(t("{name} silindi", { name: account.name }));
    },
    onError: (error) => feedback.error(error),
  });

  if (ftp.isPending) return <LoadingBlock />;
  if (ftp.isError) return <ErrorBlock message={errorMessage(ftp.error)} onRetry={() => void ftp.refetch()} />;
  const data = ftp.data;

  if (!data.installed) {
    return (
      <Panel flush>
        <EmptyState
          title={t("FTP sunucusu kurulu değil")}
          description={
            me.role === "admin"
              ? t("FTP hesabı açabilmek için önce FTP sunucusunu kurun. Dosyalar ayrıca panelin dosya yöneticisiyle de yüklenebilir.")
              : t("FTP hesabı açabilmek için sunucu yöneticinizin FTP sunucusunu kurması gerekiyor.")
          }
          action={
            me.role === "admin" ? (
              <Link to="/components" className="font-medium text-accent hover:underline">
                {t("Bileşenler sayfasına git")}
              </Link>
            ) : undefined
          }
        />
      </Panel>
    );
  }

  async function confirmRemove(account: FtpAccount) {
    const confirmed = await feedback.confirm({
      title: t("{name} hesabı silinsin mi?", { name: account.name }),
      description: t("Hesap artık bağlanamaz. Yüklediği dosyalar silinmez."),
      confirmLabel: t("Hesabı sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(account);
  }

  const root = `sites/${site.domain}`;
  return (
    <div className="flex flex-col gap-4">
      {!data.running && (
        <div role="alert" className="rounded-lg border border-warn/40 bg-warn-soft px-4 py-3 text-warn">
          {t("FTP servisi çalışmıyor; hesaplar bağlanamaz. Servisler sayfasından başlatın.")}
        </div>
      )}
      <Panel
        flush
        title={t("FTP hesapları")}
        actions={
          <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={() => setDialog({ account: null })}>
            {t("Hesap oluştur")}
          </Button>
        }
      >
        {data.accounts.length === 0 ? (
          <EmptyState
            title={t("Bu sitenin FTP hesabı yok")}
            description={t("Her hesap bu sitenin klasörüne (ya da seçtiğiniz alt klasöre) hapsedilir; başka sitelere ve sunucunun geri kalanına erişemez.")}
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Kullanıcı adı")}</Th>
                <Th>{t("Klasör")}</Th>
                <Th>{t("Oluşturulma")}</Th>
                <Th align="right">{t("İşlemler")}</Th>
              </tr>
            </thead>
            <tbody>
              {data.accounts.map((account) => (
                <tr key={account.id} className="hover:bg-sunken/50">
                  <Td>
                    <Mono className="font-medium">{account.name}</Mono>
                  </Td>
                  <Td>
                    <Mono className="text-ink-2">{account.subdir ? `${root}/${account.subdir}` : root}</Mono>
                  </Td>
                  <Td className="whitespace-nowrap text-ink-2">{formatDateTime(account.created_at)}</Td>
                  <Td align="right" className="py-0.5">
                    <div className="flex justify-end gap-0.5">
                      <IconButton label={t("Parolayı ya da klasörü değiştir")} onClick={() => setDialog({ account })}>
                        <KeyRound className="size-4" />
                      </IconButton>
                      <IconButton label={t("Sil")} onClick={() => void confirmRemove(account)}>
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
      <Panel title={t("Bağlantı bilgileri")}>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <dt className="text-ink-2">{t("Sunucu")}</dt>
          <dd>
            <Mono>{window.location.hostname}</Mono> · {t("port")} <Mono>21</Mono>
          </dd>
          <dt className="text-ink-2">{t("Şifreleme")}</dt>
          <dd>{t("Açık FTP üzerinden TLS (FTPS) önerilir; şifresiz bağlantı da kabul edilir.")}</dd>
          <dt className="text-ink-2">{t("Aktarım kipi")}</dt>
          <dd>
            {t("Pasif. Sunucunun önünde güvenlik duvarı varsa {from}–{to} portlarına izin verin.", { from: data.passive_ports[0], to: data.passive_ports[1] })}
          </dd>
        </dl>
      </Panel>
      {dialog && <AccountModal key={dialog.account?.id ?? "new"} site={site} account={dialog.account} onClose={() => setDialog(null)} onSaved={apply} />}
    </div>
  );
}
