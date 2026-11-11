// "Yedek yerleri" sekmesi: yedeklerin nerede durduğu (bu sunucu, SFTP, S3).

import { useMutation, useQuery } from "@tanstack/react-query";
import { Cloud, FolderUp, HardDrive, Pencil, Plus, Server, Trash2 } from "lucide-react";
import type { ReactNode } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, Checkbox, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Spinner, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import type { BackupOverview, Target, TargetConfig, TargetKind } from "./BackupShared";
import { CopyBox, MonoSentence, Notice, useOverviewSetter } from "./BackupShared";

// ---- Yardımcılar ----

export function targetKindLabel(kind: TargetKind): string {
  switch (kind) {
    case "local":
      return t("Bu sunucuda bir klasör");
    case "sftp":
      return t("SFTP sunucusu");
    case "s3":
      return t("S3 uyumlu depolama");
    case "ftp":
      return t("FTP sunucusu");
  }
}

/** Yerin adresini tek satırda yazar. */
function targetAddress(target: Target): string {
  const { config } = target;
  switch (target.kind) {
    case "local":
      return config.path;
    case "sftp": {
      const base = `${config.user}@${config.host}:${config.port}`;
      return config.path.startsWith("/") ? `${base}${config.path}` : `${base}/~/${config.path}`;
    }
    case "s3":
      return [config.endpoint.replace(/\/+$/, ""), config.bucket, config.path.replace(/^\/+/, "")].filter((part) => part !== "").join("/");
    case "ftp":
      return `${config.tls ? "ftpes" : "ftp"}://${config.user}@${config.host}:${config.port}/${config.path}`;
  }
}

/** Bağlantı denemesini başlatan ortak işlem; sonuç hedefin `last_check_*` alanlarında görünür. */
function useCheckTarget() {
  const setOverview = useOverviewSetter();
  return useMutation({
    mutationFn: (id: number) => api.post<BackupOverview>(`/backup/targets/${id}/check`),
    onSuccess: setOverview,
  });
}

// ---- Bağlantı sonucu ----

function CheckResult({ target, checking }: { target: Target; checking: boolean }) {
  if (checking) {
    return (
      <p className="flex items-center gap-2 text-ink-2">
        <Spinner /> {t("Bağlantı deneniyor; bu bir dakikaya kadar sürebilir.")}
      </p>
    );
  }
  if (target.last_check_at === null) return <Status tone="idle">{t("Henüz denenmedi")}</Status>;
  if (target.last_check_ok) return <Status tone="ok">{t("Bağlantı sağlam")}</Status>;
  return (
    <div className="flex flex-col gap-1">
      <Status tone="bad">{t("Bağlanılamadı")}</Status>
      {target.last_check_error && <p className="text-[13px] break-words text-bad">{target.last_check_error}</p>}
    </div>
  );
}

// ---- Parola penceresi ----

function PasswordModal({ target, onClose }: { target: Target; onClose: () => void }) {
  const query = useQuery({
    queryKey: ["backup", "password", target.id],
    queryFn: () => api.get<{ password: string }>(`/backup/targets/${target.id}/password`),
    gcTime: 0,
    staleTime: 0,
    retry: false,
  });

  return (
    <Modal open onClose={onClose} wide title={t("{name} yerinin parolası", { name: target.name })} footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      <div className="flex flex-col gap-3">
        {query.isPending && <LoadingBlock />}
        {query.isError && <Notice tone="bad">{errorMessage(query.error)}</Notice>}
        {query.data && <CopyBox value={query.data.password} label={t("Yedek parolası")} />}
        <p className="text-ink-2">
          {t(
            "Yedekler bu parolayla şifrelenir. Sunucu tümüyle kaybedilirse yedekleri açmanın tek yolu budur; parolayı panelin dışında, güvenli bir yerde saklayın.",
          )}
        </p>
      </div>
    </Modal>
  );
}

// ---- Ekleme ve düzenleme penceresi ----

interface TargetForm {
  name: string;
  path: string;
  host: string;
  port: string;
  user: string;
  endpoint: string;
  bucket: string;
  region: string;
  accessKey: string;
  secretKey: string;
  password: string;
  tls: boolean;
  tlsSkipVerify: boolean;
}

const KIND_CARDS: { id: TargetKind; description: string; icon: ReactNode }[] = [
  { id: "local", description: "En kolayı; ancak sunucu kaybedilirse yedekler de gider.", icon: <HardDrive aria-hidden className="size-4" /> },
  { id: "sftp", description: "Yedekleri SSH ile başka bir sunucuya gönderir.", icon: <Server aria-hidden className="size-4" /> },
  { id: "s3", description: "Amazon S3, Backblaze, MinIO gibi bulut depolamalar.", icon: <Cloud aria-hidden className="size-4" /> },
  { id: "ftp", description: "Depolama kutusu ya da paylaşımlı hosting gibi yalnızca FTP veren yerler.", icon: <FolderUp aria-hidden className="size-4" /> },
];

const DEFAULT_NAMES: Record<TargetKind, string> = { local: "Bu sunucu", sftp: "Uzak sunucu", s3: "Bulut depolama", ftp: "FTP sunucusu" };

function formFromTarget(target: Target | null): TargetForm {
  const config = target?.config;
  return {
    name: target?.name ?? "",
    path: config?.path ?? "",
    host: config?.host ?? "",
    port: String(config && config.port > 0 ? config.port : target?.kind === "ftp" ? 21 : 22),
    user: config?.user ?? "",
    endpoint: config?.endpoint ?? "",
    bucket: config?.bucket ?? "",
    region: config?.region ?? "",
    accessKey: config?.access_key ?? "",
    secretKey: "",
    password: "",
    tls: config?.tls ?? true,
    tlsSkipVerify: config?.tls_skip_verify ?? false,
  };
}

/** Seçilen türe göre yapılandırmayı kurar; kullanılmayan alanlar boş gider. */
function buildConfig(kind: TargetKind, form: TargetForm): TargetConfig {
  return {
    path: form.path.trim(),
    host: kind === "sftp" || kind === "ftp" ? form.host.trim() : "",
    port: kind === "sftp" || kind === "ftp" ? Number(form.port) : 0,
    user: kind === "sftp" || kind === "ftp" ? form.user.trim() : "",
    endpoint: kind === "s3" ? form.endpoint.trim() : "",
    bucket: kind === "s3" ? form.bucket.trim() : "",
    region: kind === "s3" ? form.region.trim() : "",
    access_key: kind === "s3" ? form.accessKey.trim() : "",
    tls: kind === "ftp" && form.tls,
    tls_skip_verify: kind === "ftp" && form.tls && form.tlsSkipVerify,
  };
}

function validateForm(kind: TargetKind | null, form: TargetForm, editing: Target | null): string | null {
  if (kind === null) return t("Önce yedek yerinin türünü seçin.");
  if (form.name.trim() === "") return t("Yere bir ad verin.");
  if (kind === "local" && form.path.trim() === "") return t("Klasörü yazın.");
  if (kind === "sftp") {
    const port = Number(form.port);
    if (form.host.trim() === "" || form.user.trim() === "" || form.path.trim() === "") return t("Sunucu, kullanıcı ve klasör gerekli.");
    if (!Number.isInteger(port) || port < 1 || port > 65535) return t("Port 1 ile 65535 arasında olmalı.");
  }
  if (kind === "ftp") {
    const port = Number(form.port);
    if (form.host.trim() === "" || form.user.trim() === "" || form.path.trim() === "") return t("Sunucu, kullanıcı ve klasör gerekli.");
    if (!Number.isInteger(port) || port < 1 || port > 65535) return t("Port 1 ile 65535 arasında olmalı.");
    if (form.secretKey === "" && !editing?.has_secret_key) return t("FTP parolasını yazın.");
  }
  if (kind === "s3") {
    if (form.endpoint.trim() === "" || form.bucket.trim() === "" || form.accessKey.trim() === "") return t("Uç nokta, kova ve erişim anahtarı gerekli.");
    if (form.secretKey === "" && !editing?.has_secret_key) return t("Gizli anahtarı yazın.");
  }
  return null;
}

function SftpKeyHint({ publicKey }: { publicKey: string }) {
  return (
    <div className="flex flex-col gap-2">
      <p>
        <MonoSentence template={t("Şu anahtarı uzak sunucudaki {file} dosyasına ekleyin.")} values={{ file: "~/.ssh/authorized_keys" }} />
      </p>
      <CopyBox value={publicKey} label={t("Açık anahtar")} />
    </div>
  );
}

export function TargetModal({ editing, targets, onClose }: { editing: Target | null; targets: Target[]; onClose: () => void }) {
  const feedback = useFeedback();
  const setOverview = useOverviewSetter();
  const check = useCheckTarget();

  const [kind, setKind] = useState<TargetKind | null>(editing?.kind ?? null);
  const [form, setForm] = useState<TargetForm>(() => formFromTarget(editing));
  const [createdId, setCreatedId] = useState<number | null>(null);

  const set = <K extends keyof TargetForm>(key: K, value: TargetForm[K]) => setForm((current) => ({ ...current, [key]: value }));

  function pickKind(next: TargetKind) {
    setKind(next);
    setForm((current) => ({ ...current, port: next === "ftp" ? "21" : next === "sftp" ? "22" : current.port }));
    // Ad boşsa türe uygun bir öneri konur.
    setForm((current) => (current.name.trim() === "" ? { ...current, name: t(DEFAULT_NAMES[next]) } : current));
  }

  const save = useMutation({
    mutationFn: () => {
      if (kind === null) throw new Error(t("Önce yedek yerinin türünü seçin."));
      const config = buildConfig(kind, form);
      if (editing) {
        return api.put<BackupOverview>(`/backup/targets/${editing.id}`, {
          name: form.name.trim(),
          config,
          ...(form.secretKey !== "" ? { secret_key: form.secretKey } : {}),
        });
      }
      return api.post<BackupOverview>("/backup/targets", {
        name: form.name.trim(),
        kind,
        config,
        ...(kind === "s3" || kind === "ftp" ? { secret_key: form.secretKey } : {}),
        ...(form.password !== "" ? { password: form.password } : {}),
      });
    },
    onSuccess: (next) => {
      setOverview(next);
      if (editing) {
        feedback.success(t("Yedek yeri kaydedildi"));
        onClose();
        return;
      }
      const known = new Set(targets.map((item) => item.id));
      const created = next.targets.find((item) => !known.has(item.id));
      if (!created) {
        onClose();
        return;
      }
      setCreatedId(created.id);
      // Klasör ve bulut depolamada bağlantı hemen denenir; SFTP'de önce anahtar eklenmelidir.
      if (created.kind !== "sftp") check.mutate(created.id);
    },
  });

  const problem = validateForm(kind, form, editing);

  // Eklendikten sonraki ikinci adım.
  const created = createdId === null ? null : (targets.find((item) => item.id === createdId) ?? null);
  if (createdId !== null) {
    return (
      <Modal open onClose={onClose} wide title={t("Yedek yeri eklendi")} footer={<Button variant="primary" onClick={onClose}>{t("Bitti")}</Button>}>
        <div className="flex flex-col gap-4">
          {created?.kind === "sftp" && <SftpKeyHint publicKey={created.public_key} />}
          {created?.kind === "sftp" && (
            <div>
              <Button loading={check.isPending} onClick={() => check.mutate(created.id)}>
                {t("Bağlantıyı dene")}
              </Button>
            </div>
          )}
          {created && <CheckResult target={created} checking={check.isPending} />}
          {check.isError && <Notice tone="bad">{errorMessage(check.error)}</Notice>}
          {created && created.last_check_at !== null && !created.last_check_ok && (
            <p className="text-ink-2">{t("Bilgileri yedek yerinin yanındaki düzenle düğmesinden düzeltip yeniden deneyebilirsiniz.")}</p>
          )}
        </div>
      </Modal>
    );
  }

  const publicKey = editing ? (targets.find((item) => item.id === editing.id)?.public_key ?? editing.public_key) : "";

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={editing ? t("Yedek yerini düzenle") : t("Yedek yeri ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" loading={save.isPending} disabled={problem !== null} onClick={() => save.mutate()}>
            {editing ? t("Kaydet") : t("Ekle")}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          if (!problem) save.mutate();
        }}
      >
        {editing ? (
          <p className="text-ink-2">{targetKindLabel(editing.kind)}</p>
        ) : (
          <div role="radiogroup" aria-label={t("Yedek yerinin türü")} className="grid gap-2 sm:grid-cols-3">
            {KIND_CARDS.map((card) => (
              <label
                key={card.id}
                className={classNames(
                  "flex cursor-pointer flex-col gap-1 rounded-lg border p-3 transition-colors",
                  kind === card.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                )}
              >
                <span className="flex items-center gap-2 font-medium">
                  <input type="radio" name="target-kind" className="accent-(--accent)" checked={kind === card.id} onChange={() => pickKind(card.id)} />
                  {card.icon}
                  {targetKindLabel(card.id)}
                </span>
                <span className="text-[12.5px] text-ink-2">{t(card.description)}</span>
              </label>
            ))}
          </div>
        )}

        {kind !== null && (
          <>
            <Input label={t("Ad")} value={form.name} onChange={(event) => set("name", event.target.value)} maxLength={80} />

            {kind === "local" && (
              <Input
                label={t("Klasör")}
                placeholder="/var/backups/pixpanel"
                hint={t("Sitelerin durduğu /home altında olamaz.")}
                value={form.path}
                onChange={(event) => set("path", event.target.value)}
                spellCheck={false}
                autoComplete="off"
              />
            )}

            {kind === "sftp" && (
              <div className="grid gap-3 sm:grid-cols-[1fr_6rem]">
                <Input label={t("Sunucu")} placeholder="yedek.example.com" value={form.host} onChange={(event) => set("host", event.target.value)} spellCheck={false} autoComplete="off" />
                <Input label={t("Port")} type="number" inputMode="numeric" min={1} max={65535} value={form.port} onChange={(event) => set("port", event.target.value)} />
                <div className="sm:col-span-2">
                  <Input label={t("Kullanıcı")} value={form.user} onChange={(event) => set("user", event.target.value)} spellCheck={false} autoComplete="off" />
                </div>
                <div className="sm:col-span-2">
                  <Input
                    label={t("Klasör")}
                    placeholder="/srv/yedekler"
                    hint={t("“/” ile başlarsa tam yoldur; başlamazsa kullanıcının ev klasörüne göredir.")}
                    value={form.path}
                    onChange={(event) => set("path", event.target.value)}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </div>
              </div>
            )}

            {kind === "s3" && (
              <div className="grid gap-3 sm:grid-cols-2">
                <div className="sm:col-span-2">
                  <Input label={t("Uç nokta")} placeholder="https://s3.eu-central-1.amazonaws.com" value={form.endpoint} onChange={(event) => set("endpoint", event.target.value)} spellCheck={false} autoComplete="off" />
                </div>
                <Input label={t("Kova")} value={form.bucket} onChange={(event) => set("bucket", event.target.value)} spellCheck={false} autoComplete="off" />
                <Input label={t("Bölge (isteğe bağlı)")} placeholder="eu-central-1" value={form.region} onChange={(event) => set("region", event.target.value)} spellCheck={false} autoComplete="off" />
                <div className="sm:col-span-2">
                  <Input label={t("Klasör (isteğe bağlı)")} placeholder="pixpanel" value={form.path} onChange={(event) => set("path", event.target.value)} spellCheck={false} autoComplete="off" />
                </div>
                <Input label={t("Erişim anahtarı")} value={form.accessKey} onChange={(event) => set("accessKey", event.target.value)} spellCheck={false} autoComplete="off" />
                <Input
                  label={t("Gizli anahtar")}
                  type="password"
                  placeholder={editing?.has_secret_key ? t("değiştirmek için yazın") : undefined}
                  value={form.secretKey}
                  onChange={(event) => set("secretKey", event.target.value)}
                  autoComplete="new-password"
                />
              </div>
            )}

            {kind === "ftp" && (
              <div className="grid gap-3 sm:grid-cols-[1fr_6rem]">
                <Input label={t("Sunucu")} placeholder="ftp.example.com" value={form.host} onChange={(event) => set("host", event.target.value)} spellCheck={false} autoComplete="off" />
                <Input label={t("Port")} type="number" inputMode="numeric" min={1} max={65535} value={form.port} onChange={(event) => set("port", event.target.value)} />
                <Input label={t("Kullanıcı")} value={form.user} onChange={(event) => set("user", event.target.value)} spellCheck={false} autoComplete="off" />
                <div className="sm:col-span-2">
                  <Input
                    label={t("Parola")}
                    type="password"
                    placeholder={editing?.has_secret_key ? t("değiştirmek için yazın") : undefined}
                    value={form.secretKey}
                    onChange={(event) => set("secretKey", event.target.value)}
                    autoComplete="new-password"
                  />
                </div>
                <div className="sm:col-span-2">
                  <Input
                    label={t("Klasör")}
                    placeholder="yedekler/pixpanel"
                    hint={t("FTP hesabının açıldığı klasöre göredir; yoksa oluşturulur.")}
                    value={form.path}
                    onChange={(event) => set("path", event.target.value)}
                    spellCheck={false}
                    autoComplete="off"
                  />
                </div>
                <div className="flex flex-col gap-2 sm:col-span-2">
                  <Checkbox
                    label={t("Bağlantıyı şifrele (TLS)")}
                    hint={t("Kapatırsanız FTP parolası ağda açık gider. Yedeklerin içeriği her durumda şifrelidir.")}
                    checked={form.tls}
                    onChange={(event) => set("tls", event.target.checked)}
                  />
                  {form.tls && (
                    <Checkbox
                      label={t("Sunucunun sertifikasını doğrulama")}
                      hint={t("Yalnızca sunucu kendinden imzalı sertifika kullanıyorsa işaretleyin.")}
                      checked={form.tlsSkipVerify}
                      onChange={(event) => set("tlsSkipVerify", event.target.checked)}
                    />
                  )}
                </div>
              </div>
            )}

            {editing?.kind === "sftp" && publicKey !== "" && <SftpKeyHint publicKey={publicKey} />}

            {!editing && (
              <details className="rounded-md border border-line px-3 py-2">
                <summary className="cursor-pointer font-medium text-ink-2">{t("Var olan bir yedek deposunu bağla")}</summary>
                <div className="pt-3">
                  <Input
                    label={t("Var olan yedeklerin parolası")}
                    type="password"
                    hint={t("Daha önce bu panelle alınmış yedeklerin durduğu yeri yeniden bağlarken o yedeklerin parolasını yazın. Boş bırakırsanız yeni bir parola üretilir.")}
                    value={form.password}
                    onChange={(event) => set("password", event.target.value)}
                    autoComplete="new-password"
                  />
                </div>
              </details>
            )}
          </>
        )}

        {save.isError && <Notice tone="bad">{errorMessage(save.error)}</Notice>}
        {/* Enter ile gönderimi etkinleştiren gizli düğme. */}
        <button type="submit" hidden />
      </form>
    </Modal>
  );
}

// ---- Sekme ----

function TargetCard({
  target,
  checking,
  onCheck,
  onShowPassword,
  onEdit,
  onDelete,
  deleting,
}: {
  target: Target;
  checking: boolean;
  onCheck: () => void;
  onShowPassword: () => void;
  onEdit: () => void;
  onDelete: () => void;
  deleting: boolean;
}) {
  return (
    <li className="flex flex-col gap-3 border-b border-line px-4 py-4 last:border-b-0">
      <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-semibold">{target.name}</span>
            <Badge>{targetKindLabel(target.kind)}</Badge>
          </div>
          <p className="mt-0.5 break-all">
            <Mono>{targetAddress(target)}</Mono>
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-1">
          <Button size="sm" loading={checking} onClick={onCheck}>
            {t("Bağlantıyı dene")}
          </Button>
          <Button size="sm" onClick={onShowPassword}>
            {t("Parolayı göster")}
          </Button>
          <IconButton label={t("Yedek yerini düzenle")} onClick={onEdit}>
            <Pencil className="size-4" />
          </IconButton>
          <IconButton
            label={target.plan_count > 0 ? t("Önce bu yeri kullanan planları silin") : t("Yedek yerini sil")}
            disabled={target.plan_count > 0 || deleting}
            onClick={onDelete}
          >
            {deleting ? <Spinner /> : <Trash2 className="size-4" />}
          </IconButton>
        </div>
      </div>

      <div className="flex flex-wrap items-start gap-x-6 gap-y-1">
        {checking ? (
          <CheckResult target={target} checking />
        ) : (
          <div className="flex flex-col gap-0.5">
            <CheckResult target={target} checking={false} />
            {target.last_check_at !== null && target.last_check_ok && <span className="text-[12.5px] text-ink-3">{formatRelative(target.last_check_at)}</span>}
          </div>
        )}
        <span className="text-ink-2">{target.plan_count > 0 ? t("{n} planda kullanılıyor", { n: target.plan_count }) : t("Hiçbir planda kullanılmıyor")}</span>
      </div>

      {target.kind === "local" && (
        <Notice tone="warn" className="text-[13px]">
          {t("Bu yedekler sitelerle aynı sunucuda duruyor; sunucu ya da diski kaybedilirse yedekler de gider. Asıl yedek için SFTP ya da S3 kullanın.")}
        </Notice>
      )}
    </li>
  );
}

export function TargetsTab({ overview, onAdd }: { overview: BackupOverview; onAdd: () => void }) {
  const feedback = useFeedback();
  const setOverview = useOverviewSetter();
  const check = useCheckTarget();

  const [editing, setEditing] = useState<Target | null>(null);
  const [passwordFor, setPasswordFor] = useState<Target | null>(null);

  const remove = useMutation({
    mutationFn: (target: Target) => api.delete<BackupOverview>(`/backup/targets/${target.id}`),
    onSuccess: (next) => {
      setOverview(next);
      feedback.success(t("Yedek yeri silindi"));
    },
    onError: (error) => feedback.error(error),
  });

  async function askDelete(target: Target) {
    const confirmed = await feedback.confirm({
      title: t("Yedek yeri silinsin mi?"),
      description: t("“{name}” yeri panelden kaldırılır; orada duran yedekler silinmez. Onlara sonradan ulaşmak için parolayı saklayın.", { name: target.name }),
      confirmLabel: t("Sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(target);
  }

  function runCheck(target: Target) {
    check.mutate(target.id, {
      onSuccess: (next) => {
        if (next.targets.find((item) => item.id === target.id)?.last_check_ok) feedback.success(t("Bağlantı sağlam"));
      },
      onError: (error) => feedback.error(error),
    });
  }

  return (
    <>
      <div className="flex flex-col gap-3">
        <div className="flex justify-end">
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAdd}>
            {t("Yedek yeri ekle")}
          </Button>
        </div>
        <Panel flush>
          <ul>
            {overview.targets.map((target) => (
              <TargetCard
                key={target.id}
                target={target}
                checking={check.isPending && check.variables === target.id}
                onCheck={() => runCheck(target)}
                onShowPassword={() => setPasswordFor(target)}
                onEdit={() => setEditing(target)}
                onDelete={() => void askDelete(target)}
                deleting={remove.isPending && remove.variables.id === target.id}
              />
            ))}
          </ul>
        </Panel>
      </div>

      {editing && <TargetModal editing={editing} targets={overview.targets} onClose={() => setEditing(null)} />}
      {passwordFor && <PasswordModal target={passwordFor} onClose={() => setPasswordFor(null)} />}
    </>
  );
}
