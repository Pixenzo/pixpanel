import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Pause, Play, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
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
  Td,
  Th,
} from "../components/ui";
import type { Role, UserInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

export const roleLabels: Record<Role, string> = {
  admin: "Yönetici",
  reseller: "Bayi",
  user: "Müşteri",
  member: "Alt hesap",
};

/** Alt hesaplara verilebilecek yetki alanları. */
const permissionAreas: [string, string][] = [
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
      <legend className="mb-2 text-[13px] font-medium text-ink-2">{t("Erişebileceği alanlar")}</legend>
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
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [username, setUsername] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>(roles[0]);
  const [permissions, setPermissions] = useState<string[]>(["sites", "files"]);

  const create = useMutation({
    mutationFn: () =>
      api.post("/users", { username, email, password, role, permissions: role === "member" ? permissions : [] }),
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
      <form id="create-user" onSubmit={submit} className="flex flex-col gap-4">
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

  const save = useMutation({
    mutationFn: () =>
      api.patch(`/users/${user.id}`, {
        email,
        password: password || undefined,
        permissions: user.role === "member" ? permissions : undefined,
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
        className="flex flex-col gap-4"
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
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
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

  const users = useQuery({ queryKey: ["users"], queryFn: () => api.get<UserInfo[]>("/users") });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["users"] });

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
      {users.isError && <ErrorBlock message={errorMessage(users.error)} onRetry={() => void users.refetch()} />}
      {users.isPending && <LoadingBlock />}
      {users.data && (
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
                        {self && <span className="ml-2 text-[12.5px] text-ink-3">{t("(siz)")}</span>}
                        {user.email && <p className="text-[12.5px] text-ink-3">{user.email}</p>}
                      </Td>
                      <Td>
                        <Badge tone={user.role === "admin" ? "accent" : "idle"}>{t(roleLabels[user.role])}</Badge>
                        {user.parent_username && me.role === "admin" && (
                          <span className="ml-2 text-[12.5px] text-ink-3">{t("{name} altında", { name: user.parent_username })}</span>
                        )}
                      </Td>
                      <Td>
                        {user.status === "active" ? <Status tone="ok">{t("Etkin")}</Status> : <Status tone="warn">{t("Askıda")}</Status>}
                      </Td>
                      <Td className="whitespace-nowrap text-ink-2">{formatRelative(user.last_login_at)}</Td>
                      <Td align="right" className="py-0.5">
                        <div className="flex justify-end gap-0.5">
                          <IconButton label={t("Düzenle ve parola değiştir")} onClick={() => setEditing(user)}>
                            <KeyRound className="size-4" />
                          </IconButton>
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
