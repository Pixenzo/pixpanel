import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, LogIn, Pause, Play, Plus, ShieldOff, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { AccessPanel } from "../components/AccessPanel";
import { useFeedback } from "../components/feedback";
import { ResellerBrandSummary } from "../components/ResellerBranding";
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Modal,
  Mono,
  PageHeader,
  Panel,
  Select,
  Status,
  Table,
  Tabs,
  Td,
  Th,
} from "../components/ui";
import type { Role, UserInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { PackageSelect, PackagesPanel, UsagePanel } from "./Packages";

/** Liste uçunun döndürdüğü, iki adımlı doğrulama durumunu da içeren hesap kaydı. */
type ManagedUser = UserInfo & { two_factor: boolean };

export const roleLabels: Record<Role, string> = {
  admin: "Yönetici",
  reseller: "Bayi",
  user: "Müşteri",
  member: "Alt hesap",
};

/** Alt hesaplara verilebilecek yetki alanları. */
export const permissionAreas: [string, string][] = [
  ["sites", "Siteler"],
  ["files", "Dosyalar"],
  ["databases", "Veritabanları"],
  ["mail", "E-posta"],
  ["dns", "DNS"],
  ["ftp", "FTP"],
  ["ssl", "SSL sertifikaları"],
  ["cron", "Zamanlanmış görevler"],
  ["backups", "Yedekler"],
  ["logs", "Günlükler"],
  ["docker", "Docker"],
];

/** Oturumdaki kullanıcının rolüne göre açabileceği hesap türleri. */
function creatableRoles(role: Role): Role[] {
  if (role === "admin") return ["user", "reseller", "admin"];
  if (role === "reseller") return ["user"];
  if (role === "user") return ["member"];
  return [];
}

function PermissionPicker({ value, onChange }: { value: string[]; onChange: (next: string[]) => void }) {
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Erişebileceği alanlar")}</legend>
      <div className="grid grid-cols-2 gap-x-4 gap-y-2">
        {permissionAreas.map(([key, label]) => (
          <Checkbox
            key={key}
            label={t(label)}
            checked={value.includes(key)}
            onChange={(event) => onChange(event.target.checked ? [...value, key] : value.filter((item) => item !== key))}
          />
        ))}
      </div>
    </fieldset>
  );
}

function CreateUserModal({ open, onClose, roles }: { open: boolean; onClose: () => void; roles: Role[] }) {
  const me = useMe();
  const canManagePackages = me.role === "admin" || me.role === "reseller";
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>(roles[0]);
  const [permissions, setPermissions] = useState<string[]>(["sites", "files"]);
  const [packageId, setPackageId] = useState(0);
  // Paket yalnızca müşteri ve bayi hesaplarına atanır; alt hesap üst hesabının paketine tabidir.
  const packaged = role === "user" || role === "reseller";

  const create = useMutation({
    mutationFn: () =>
      api.post("/users", {
        username,
        email,
        password,
        role,
        permissions: role === "member" ? permissions : [],
        package_id: packaged && packageId ? packageId : undefined,
      }),
    onSuccess: () => {
      feedback.success(t("{name} hesabı oluşturuldu", { name: username }));
      void queryClient.invalidateQueries({ queryKey: ["users"] });
      setUsername("");
      setEmail("");
      setPassword("");
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={role === "member" ? t("Alt hesap oluştur") : t("Hesap oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-user" loading={create.isPending}>
            {t("Hesabı oluştur")}
          </Button>
        </>
      }
    >
      <form id="create-user" onSubmit={submit} className="flex flex-col gap-5">
        {roles.length > 1 && (
          <Select label={t("Hesap türü")} value={role} onChange={(event) => setRole(event.target.value as Role)}>
            {roles.map((item) => (
              <option key={item} value={item}>
                {t(roleLabels[item])}
              </option>
            ))}
          </Select>
        )}
        <Input
          label={t("Kullanıcı adı")}
          value={username}
          onChange={(event) => setUsername(event.target.value.toLowerCase())}
          hint={t("3-32 karakter; küçük harf, rakam, - ve _")}
          autoComplete="off"
          spellCheck={false}
          required
        />
        <Input label={t("E-posta")} type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
        <Input
          label={t("Parola")}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          hint={t("En az 10 karakter")}
          autoComplete="new-password"
          minLength={10}
          required
        />
        {role === "member" && <PermissionPicker value={permissions} onChange={setPermissions} />}
        {packaged && canManagePackages && <PackageSelect value={packageId} onChange={setPackageId} />}
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function EditUserModal({ user, onClose }: { user: UserInfo; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [email, setEmail] = useState(user.email);
  const [password, setPassword] = useState("");
  const [permissions, setPermissions] = useState(user.permissions);
  const [packageId, setPackageId] = useState(user.package_id ?? 0);
  const me = useMe();
  // Hesap kendi paketini değiştiremez; paketi onu yöneten yönetici ya da bayi atar.
  const packaged = (user.role === "user" || user.role === "reseller") && (me.role === "admin" || me.role === "reseller") && user.id !== me.id;

  const save = useMutation({
    mutationFn: () =>
      api.patch(`/users/${user.id}`, {
        email,
        password: password || undefined,
        permissions: user.role === "member" ? permissions : undefined,
        package_id: packaged && packageId !== (user.package_id ?? 0) ? packageId : undefined,
      }),
    onSuccess: () => {
      feedback.success(t("{name} güncellendi", { name: user.username }));
      void queryClient.invalidateQueries({ queryKey: ["users"] });
      onClose();
    },
  });

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={t("{name} hesabını düzenle", { name: user.username })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="edit-user" loading={save.isPending}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </>
      }
    >
      <form
        id="edit-user"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate();
        }}
        className="flex flex-col gap-5"
      >
        <Input label={t("E-posta")} type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
        <Input
          label={t("Yeni parola")}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          hint={t("Değiştirmek istemiyorsanız boş bırakın. Değişirse açık oturumları kapanır.")}
          autoComplete="new-password"
          minLength={10}
        />
        {user.role === "member" && <PermissionPicker value={permissions} onChange={setPermissions} />}
        {packaged && <PackageSelect value={packageId} onChange={setPackageId} />}
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
      {packaged && (
        <div className="mt-5">
          <UsagePanel path={`/users/${user.id}/usage`} title={t("Kullanım")} />
        </div>
      )}
      {user.role === "reseller" && me.role === "admin" && (
        <div className="mt-5">
          <ResellerBrandSummary resellerId={user.id} />
        </div>
      )}
      {user.role !== "member" && (
        <div className="mt-5">
          <AccessPanel accountId={user.id} />
        </div>
      )}
    </Modal>
  );
}

export function UsersPage() {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<UserInfo | null>(null);
  const roles = creatableRoles(me.role);

  const users = useQuery({ queryKey: ["users"], queryFn: () => api.get<ManagedUser[]>("/users") });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["users"] });

  const resetTwoFactor = useMutation({
    mutationFn: (user: ManagedUser) => api.post(`/users/${user.id}/2fa/reset`),
    onSuccess: (_, user) => feedback.success(t("{name} hesabının iki adımlı doğrulaması sıfırlandı", { name: user.username })),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });

  async function confirmResetTwoFactor(user: ManagedUser) {
    const confirmed = await feedback.confirm({
      title: t("{name} için iki adımlı doğrulama sıfırlansın mı?", { name: user.username }),
      description: t(
        "Hesap yalnızca parolasıyla girebilir hâle gelir ve bütün oturumları kapatılır. Kullanıcı telefonunu kaybettiyse kullanın.",
      ),
      confirmLabel: t("Doğrulamayı sıfırla"),
      danger: true,
    });
    if (confirmed) resetTwoFactor.mutate(user);
  }

  const setStatus = useMutation({
    mutationFn: ({ user, status }: { user: UserInfo; status: string }) => api.patch(`/users/${user.id}`, { status }),
    onSuccess: (_, { user, status }) =>
      feedback.success(
        status === "suspended" ? t("{name} askıya alındı", { name: user.username }) : t("{name} yeniden etkinleştirildi", { name: user.username }),
      ),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });

  const remove = useMutation({
    mutationFn: (user: UserInfo) => api.delete(`/users/${user.id}`),
    onSuccess: (_, user) => feedback.success(t("{name} silindi", { name: user.username })),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });

  const impersonate = useMutation({
    mutationFn: (user: UserInfo) => api.post(`/users/${user.id}/impersonate`),
    // Oturum değişti: sayfa baştan yüklenir ki bütün veriler yeni hesabın gözünden gelsin.
    onSuccess: () => window.location.assign("/"),
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(user: UserInfo) {
    const confirmed = await feedback.confirm({
      title: t("{name} silinsin mi?", { name: user.username }),
      description: t("Hesap ve ona bağlı alt hesaplar kalıcı olarak silinir. Bu işlem geri alınamaz."),
      confirmLabel: t("Hesabı sil"),
      danger: true,
      typeToConfirm: user.username,
    });
    if (confirmed) remove.mutate(user);
  }

  const isMemberView = me.role === "user";
  const canManagePackages = me.role === "admin" || me.role === "reseller";
  const [tab, setTab] = useState<"accounts" | "packages">("accounts");
  const createLabel = isMemberView ? t("Alt hesap oluştur") : t("Hesap oluştur");

  return (
    <>
      <PageHeader
        title={isMemberView ? t("Alt hesaplar") : t("Kullanıcılar")}
        description={
          isMemberView
            ? t("Ekibinizdeki kişilere yalnızca seçtiğiniz alanlarda çalışabilecekleri ayrı girişler verin.")
            : t("Panele giriş yapabilen hesaplar.")
        }
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
            {createLabel}
          </Button>
        }
      />
      {canManagePackages && (
        <Tabs
          label={t("Kullanıcılar")}
          value={tab}
          onChange={setTab}
          tabs={[
            { id: "accounts", label: t("Hesaplar") },
            { id: "packages", label: t("Paketler") },
          ]}
        />
      )}
      {tab === "packages" && <PackagesPanel />}
      {tab === "accounts" && users.isError && <ErrorBlock message={errorMessage(users.error)} onRetry={() => void users.refetch()} />}
      {tab === "accounts" && users.isPending && <LoadingBlock />}
      {tab === "accounts" && users.data && (
        <Panel flush>
          {users.data.length === 0 ? (
            <EmptyState
              title={isMemberView ? t("Henüz alt hesap yok") : t("Henüz hesap yok")}
              description={t("İlk hesabı oluşturduğunuzda burada listelenir.")}
              action={
                <Button variant="primary" onClick={() => setCreating(true)}>
                  {createLabel}
                </Button>
              }
            />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Kullanıcı")}</Th>
                  <Th>{t("Tür")}</Th>
                  {canManagePackages && <Th>{t("Paket")}</Th>}
                  <Th>{t("Durum")}</Th>
                  <Th>{t("Son giriş")}</Th>
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {users.data.map((user) => {
                  const self = user.id === me.id;
                  return (
                    <tr key={user.id} className="hover:bg-sunken/50">
                      <Td>
                        <Mono className="font-medium">{user.username}</Mono>
                        {self && <span className="ml-2 text-[13px] text-ink-3">{t("(siz)")}</span>}
                        {user.two_factor && (
                          <span className="ml-2">
                            <Badge tone="ok">{t("2 adımlı")}</Badge>
                          </span>
                        )}
                        {user.email && <p className="text-[13px] text-ink-3">{user.email}</p>}
                      </Td>
                      <Td>
                        <Badge tone={user.role === "admin" ? "accent" : "idle"}>{t(roleLabels[user.role])}</Badge>
                        {user.parent_username && me.role === "admin" && (
                          <span className="ml-2 text-[13px] text-ink-3">{t("{name} altında", { name: user.parent_username })}</span>
                        )}
                      </Td>
                      {canManagePackages && <Td className={user.package_name ? undefined : "text-ink-3"}>{user.package_name ?? (user.role === "admin" || user.role === "member" ? "" : t("Sınırsız"))}</Td>}
                      <Td>
                        {user.status === "active" ? <Status tone="ok">{t("Etkin")}</Status> : <Status tone="warn">{t("Askıda")}</Status>}
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2">{formatRelative(user.last_login_at)}</Td>
                      <Td align="right" className="py-0.5">
                        <div className="flex justify-end gap-0.5">
                          {!self && user.role !== "admin" && user.status === "active" && me.impersonated_by === null && (
                            <IconButton label={t("{name} hesabına gir", { name: user.username })} onClick={() => impersonate.mutate(user)} disabled={impersonate.isPending}>
                              <LogIn className="size-4" />
                            </IconButton>
                          )}
                          <IconButton label={t("Düzenle ve parola değiştir")} onClick={() => setEditing(user)}>
                            <KeyRound className="size-4" />
                          </IconButton>
                          {user.two_factor && (
                            <IconButton label={t("İki adımlı doğrulamayı sıfırla")} onClick={() => void confirmResetTwoFactor(user)}>
                              <ShieldOff className="size-4" />
                            </IconButton>
                          )}
                          {user.status === "active" ? (
                            <IconButton label={t("Askıya al")} disabled={self} onClick={() => setStatus.mutate({ user, status: "suspended" })}>
                              <Pause className="size-4" />
                            </IconButton>
                          ) : (
                            <IconButton label={t("Yeniden etkinleştir")} onClick={() => setStatus.mutate({ user, status: "active" })}>
                              <Play className="size-4" />
                            </IconButton>
                          )}
                          <IconButton label={t("Sil")} disabled={self} onClick={() => void confirmRemove(user)}>
                            <Trash2 className="size-4" />
                          </IconButton>
                        </div>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
      {roles.length > 0 && <CreateUserModal open={creating} onClose={() => setCreating(false)} roles={roles} />}
      {editing && <EditUserModal user={editing} onClose={() => setEditing(null)} />}
    </>
  );
}
