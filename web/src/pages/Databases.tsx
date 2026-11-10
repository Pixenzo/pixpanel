import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, KeyRound, Plus, Trash2, Upload, UserCog, Wand2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link } from "react-router";
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
  Table,
  Tabs,
  Td,
  Th,
} from "../components/ui";
import type { DatabaseInfo, DatabaseOverview, DbAccess, DbEngine, DbUserInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { JobLog } from "./Jobs";

const accessLabels: Record<DbAccess, string> = {
  full: "Tam yetki",
  readwrite: "Okuma ve yazma",
  readonly: "Yalnızca okuma",
};

const accessHints: Record<DbAccess, string> = {
  full: "Tablo oluşturma ve silme dahil her şey",
  readwrite: "Veri okur ve değiştirir; tabloların yapısına dokunamaz",
  readonly: "Yalnızca veri okur",
};

/** Karışabilen karakterler (0/O, 1/l) olmadan rastgele parola üretir. */
function generatePassword(length = 20): string {
  const alphabet = "abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const values = crypto.getRandomValues(new Uint32Array(length));
  return Array.from(values, (value) => alphabet[value % alphabet.length]).join("");
}

function PasswordField({ value, onChange, label }: { value: string; onChange: (next: string) => void; label: string }) {
  return (
    <div className="flex items-start gap-2">
      <div className="min-w-0 flex-1">
        <Input
          label={label}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          hint={t("En az 10 karakter. Uygulamanızın ayar dosyasına yazacağınız parola.")}
          className="font-mono text-[13px]"
          autoComplete="off"
          spellCheck={false}
          minLength={10}
          required
        />
      </div>
      <Button className="mt-[25px]" icon={<Wand2 className="size-4" />} onClick={() => onChange(generatePassword())}>
        {t("Üret")}
      </Button>
    </div>
  );
}

function HostField({ value, onChange }: { value: string; onChange: (next: string) => void }) {
  const preset = value === "localhost" || value === "%" ? value : "custom";
  return (
    <div className="flex flex-col gap-3">
      <Select
        label={t("Nereden bağlanabilir")}
        value={preset}
        onChange={(event) => onChange(event.target.value === "custom" ? "" : event.target.value)}
      >
        <option value="localhost">{t("Yalnızca bu sunucudan")}</option>
        <option value="%">{t("Her yerden")}</option>
        <option value="custom">{t("Belirli bir IP adresinden")}</option>
      </Select>
      {preset === "custom" && (
        <Input
          label={t("IP adresi")}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          placeholder="203.0.113.10"
          hint={t("Dışarıdan bağlantı için güvenlik duvarında veritabanı portunun da açık olması gerekir.")}
          className="font-mono text-[13px]"
          required
        />
      )}
    </div>
  );
}

function CreateDatabaseModal({ engine, prefix, onClose }: { engine: DbEngine; prefix: string; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [name, setName] = useState(prefix);
  const [withUser, setWithUser] = useState(true);
  const [userName, setUserName] = useState("");
  const [userTouched, setUserTouched] = useState(false);
  const [password, setPassword] = useState(generatePassword);
  const [host, setHost] = useState("localhost");

  // Kullanıcı adı elle değiştirilmedikçe veritabanı adını izler.
  const effectiveUser = userTouched ? userName : name;

  const create = useMutation({
    mutationFn: () =>
      api.post("/databases", {
        engine,
        name,
        user: withUser ? { name: effectiveUser, password, host: engine === "mariadb" ? host : undefined } : undefined,
      }),
    onSuccess: () => {
      feedback.success(t("{name} veritabanı oluşturuldu", { name }));
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
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
      title={t("Veritabanı oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-database" loading={create.isPending}>
            {t("Veritabanını oluştur")}
          </Button>
        </>
      }
    >
      <form id="create-database" onSubmit={submit} className="flex flex-col gap-4">
        <Input
          label={t("Veritabanı adı")}
          value={name}
          onChange={(event) => setName(event.target.value.toLowerCase())}
          hint={prefix ? t("“{prefix}” ile başlamalı; küçük harf, rakam ve _ kullanın.", { prefix }) : t("Küçük harf, rakam ve _ kullanın.")}
          className="font-mono text-[13px]"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          required
        />
        <Checkbox
          label={t("Bu veritabanı için bir kullanıcı da oluştur")}
          hint={t("Uygulamanız veritabanına bu kullanıcıyla bağlanır.")}
          checked={withUser}
          onChange={(event) => setWithUser(event.target.checked)}
        />
        {withUser && (
          <>
            <Input
              label={t("Kullanıcı adı")}
              value={effectiveUser}
              onChange={(event) => {
                setUserTouched(true);
                setUserName(event.target.value.toLowerCase());
              }}
              className="font-mono text-[13px]"
              autoComplete="off"
              spellCheck={false}
              required
            />
            <PasswordField label={t("Parola")} value={password} onChange={setPassword} />
            {engine === "mariadb" && <HostField value={host} onChange={setHost} />}
            <p className="rounded-md bg-accent-soft px-3 py-2 text-[13px] text-accent">
              {t("Parolayı şimdi kopyalayın; oluşturduktan sonra yeniden gösterilmez, yalnızca değiştirilebilir.")}
            </p>
          </>
        )}
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function CreateUserModal({ engine, prefix, onClose }: { engine: DbEngine; prefix: string; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [name, setName] = useState(prefix);
  const [password, setPassword] = useState(generatePassword);
  const [host, setHost] = useState("localhost");

  const create = useMutation({
    mutationFn: () => api.post("/databases/users", { engine, name, password, host: engine === "mariadb" ? host : undefined }),
    onSuccess: () => {
      feedback.success(t("{name} kullanıcısı oluşturuldu", { name }));
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
      onClose();
    },
  });

  return (
    <Modal
      open
      onClose={onClose}
      title={t("Veritabanı kullanıcısı oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-db-user" loading={create.isPending}>
            {t("Kullanıcıyı oluştur")}
          </Button>
        </>
      }
    >
      <form
        id="create-db-user"
        onSubmit={(event) => {
          event.preventDefault();
          create.mutate();
        }}
        className="flex flex-col gap-4"
      >
        <Input
          label={t("Kullanıcı adı")}
          value={name}
          onChange={(event) => setName(event.target.value.toLowerCase())}
          hint={prefix ? t("“{prefix}” ile başlamalı.", { prefix }) : undefined}
          className="font-mono text-[13px]"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          required
        />
        <PasswordField label={t("Parola")} value={password} onChange={setPassword} />
        {engine === "mariadb" && <HostField value={host} onChange={setHost} />}
        <p className="text-[13px] text-ink-3">{t("Kullanıcıyı oluşturduktan sonra erişebileceği veritabanlarını “Erişim” düğmesinden seçin.")}</p>
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function PasswordModal({ user, onClose }: { user: DbUserInfo; onClose: () => void }) {
  const feedback = useFeedback();
  const [password, setPassword] = useState(generatePassword);
  const change = useMutation({
    mutationFn: () => api.patch(`/databases/users/${user.id}`, { password }),
    onSuccess: () => {
      feedback.success(t("{name} parolası değiştirildi", { name: user.name }));
      onClose();
    },
  });
  return (
    <Modal
      open
      onClose={onClose}
      title={t("{name} parolasını değiştir", { name: user.name })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="db-user-password" loading={change.isPending}>
            {t("Parolayı değiştir")}
          </Button>
        </>
      }
    >
      <form
        id="db-user-password"
        onSubmit={(event) => {
          event.preventDefault();
          change.mutate();
        }}
        className="flex flex-col gap-4"
      >
        <PasswordField label={t("Yeni parola")} value={password} onChange={setPassword} />
        <p className="text-[13px] text-ink-3">{t("Bu kullanıcıyla bağlanan uygulamaların ayar dosyasını da güncellemeyi unutmayın.")}</p>
        {change.isError && <ErrorBlock message={errorMessage(change.error)} />}
      </form>
    </Modal>
  );
}

function AccessModal({ database, users, onClose }: { database: DatabaseInfo; users: DbUserInfo[]; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const candidates = users.filter((user) => user.engine === database.engine && user.owner === database.owner);

  const change = useMutation({
    mutationFn: ({ user, access }: { user: DbUserInfo; access: DbAccess | "none" }) =>
      access === "none"
        ? api.delete(`/databases/${database.id}/grants/${user.id}`)
        : api.put(`/databases/${database.id}/grants/${user.id}`, { access }),
    onSuccess: (_, { user, access }) =>
      feedback.success(
        access === "none"
          ? t("{user} artık {db} veritabanına erişemez", { user: user.name, db: database.name })
          : t("{user} erişimi güncellendi", { user: user.name }),
      ),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["databases"] }),
  });

  return (
    <Modal open onClose={onClose} title={t("{name} erişimi", { name: database.name })} wide>
      {candidates.length === 0 ? (
        <p className="text-ink-2">{t("Bu veritabanına erişim verebileceğiniz bir kullanıcı yok. Önce bir veritabanı kullanıcısı oluşturun.")}</p>
      ) : (
        <ul className="flex flex-col divide-y divide-line">
          {candidates.map((user) => {
            const current = database.grants.find((grant) => grant.db_user_id === user.id)?.access ?? "none";
            return (
              <li key={user.id} className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
                <div>
                  <Mono className="font-medium">{user.name}</Mono>
                  {user.host && user.host !== "localhost" && <span className="ml-2 text-[12.5px] text-ink-3">@{user.host}</span>}
                  <p className="text-[12.5px] text-ink-3">{current === "none" ? t("Bu veritabanına erişemez") : t(accessHints[current])}</p>
                </div>
                <select
                  aria-label={t("{user} için erişim düzeyi", { user: user.name })}
                  value={current}
                  disabled={change.isPending}
                  onChange={(event) => change.mutate({ user, access: event.target.value as DbAccess | "none" })}
                  className="h-9 rounded-md border border-line-strong bg-surface px-3"
                >
                  <option value="none">{t("Erişim yok")}</option>
                  <option value="readonly">{t(accessLabels.readonly)}</option>
                  <option value="readwrite">{t(accessLabels.readwrite)}</option>
                  <option value="full">{t(accessLabels.full)}</option>
                </select>
              </li>
            );
          })}
        </ul>
      )}
    </Modal>
  );
}

function ImportModal({ database, onClose }: { database: DatabaseInfo; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [file, setFile] = useState<File | null>(null);
  const [jobId, setJobId] = useState<number | null>(null);

  const upload = useMutation({
    mutationFn: () => {
      const form = new FormData();
      form.append("file", file!);
      return api.upload<{ job_id: number }>(`/databases/${database.id}/import`, form);
    },
    onSuccess: (result) => setJobId(result.job_id),
  });

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{name} veritabanına içe aktar", { name: database.name })}
      wide={jobId !== null}
      footer={
        jobId === null ? (
          <>
            <Button onClick={onClose}>{t("Vazgeç")}</Button>
            <Button variant="primary" disabled={!file} loading={upload.isPending} onClick={() => upload.mutate()}>
              {t("İçe aktar")}
            </Button>
          </>
        ) : undefined
      }
    >
      {jobId === null ? (
        <div className="flex flex-col gap-4">
          <p className="text-ink-2">
            {t("Bir SQL döküm dosyası seçin (.sql ya da .sql.gz). Dosyadaki tablolar veritabanına eklenir; aynı adlı tablolar varsa dosyanın içeriğine göre üzerine yazılabilir.")}
          </p>
          <input
            type="file"
            aria-label={t("Döküm dosyası seç")}
            accept=".sql,.gz"
            onChange={(event) => setFile(event.target.files?.[0] ?? null)}
            className="text-ink-2 file:mr-3 file:h-9 file:rounded-md file:border file:border-line-strong file:bg-surface file:px-3.5 file:font-medium file:text-ink"
          />
          {file && (
            <p className="text-[13px] text-ink-3">
              {file.name}, {formatBytes(file.size)}
            </p>
          )}
          {upload.isError && <ErrorBlock message={errorMessage(upload.error)} />}
        </div>
      ) : (
        <JobLog jobId={jobId} onDone={() => void queryClient.invalidateQueries({ queryKey: ["databases"] })} />
      )}
    </Modal>
  );
}

type Dialog =
  | { kind: "create-database" }
  | { kind: "create-user" }
  | { kind: "password"; user: DbUserInfo }
  | { kind: "access"; database: DatabaseInfo }
  | { kind: "import"; database: DatabaseInfo };

export function DatabasesPage() {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [engineChoice, setEngineChoice] = useState<DbEngine | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);

  const overview = useQuery({ queryKey: ["databases"], queryFn: () => api.get<DatabaseOverview>("/databases") });
  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["databases"] });

  const dropDatabase = useMutation({
    mutationFn: (database: DatabaseInfo) => api.delete(`/databases/${database.id}`),
    onSuccess: (_, database) => feedback.success(t("{name} silindi", { name: database.name })),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });
  const dropUser = useMutation({
    mutationFn: (user: DbUserInfo) => api.delete(`/databases/users/${user.id}`),
    onSuccess: (_, user) => feedback.success(t("{name} silindi", { name: user.name })),
    onError: (error) => feedback.error(error),
    onSettled: invalidate,
  });

  if (overview.isPending) return <LoadingBlock />;
  if (overview.isError) return <ErrorBlock message={errorMessage(overview.error)} onRetry={() => void overview.refetch()} />;

  const data = overview.data;
  const available = data.engines.filter((engine) => engine.installed);
  const engine = available.find((item) => item.id === engineChoice) ?? available[0];
  const showOwner = me.role === "admin";

  if (!engine) {
    return (
      <>
        <PageHeader title={t("Veritabanları")} />
        <Panel flush>
          <EmptyState
            title={t("Kurulu bir veritabanı sunucusu yok")}
            description={
              me.role === "admin"
                ? t("Veritabanı oluşturabilmek için önce MariaDB ya da PostgreSQL kurun.")
                : t("Sunucu yöneticinizden MariaDB ya da PostgreSQL kurmasını isteyin.")
            }
            action={
              me.role === "admin" ? (
                <Link to="/components" className="font-medium text-accent underline">
                  {t("Bileşenler sayfasına git")}
                </Link>
              ) : undefined
            }
          />
        </Panel>
      </>
    );
  }

  const databases = data.databases.filter((database) => database.engine === engine.id);
  const users = data.users.filter((user) => user.engine === engine.id);

  async function confirmDropDatabase(database: DatabaseInfo) {
    const confirmed = await feedback.confirm({
      title: t("{name} veritabanı silinsin mi?", { name: database.name }),
      description: t("Bütün tablolar ve veriler kalıcı olarak silinir. Bu işlem geri alınamaz; önce dışa aktarıp yedeğini almak isteyebilirsiniz."),
      confirmLabel: t("Veritabanını sil"),
      danger: true,
      typeToConfirm: database.name,
    });
    if (confirmed) dropDatabase.mutate(database);
  }

  async function confirmDropUser(user: DbUserInfo) {
    const confirmed = await feedback.confirm({
      title: t("{name} kullanıcısı silinsin mi?", { name: user.name }),
      description: t("Bu kullanıcıyla bağlanan uygulamalar veritabanına erişemez hâle gelir. Veritabanlarının kendisi silinmez."),
      confirmLabel: t("Kullanıcıyı sil"),
      danger: true,
    });
    if (confirmed) dropUser.mutate(user);
  }

  return (
    <>
      <PageHeader
        title={t("Veritabanları")}
        description={t("Veritabanlarınız, onlara bağlanan kullanıcılar ve erişim düzeyleri.")}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} disabled={!engine.running} onClick={() => setDialog({ kind: "create-database" })}>
            {t("Veritabanı oluştur")}
          </Button>
        }
      />
      {available.length > 1 && (
        <Tabs
          label={t("Veritabanı türü")}
          tabs={available.map((item) => ({ id: item.id, label: item.label }))}
          value={engine.id}
          onChange={setEngineChoice}
        />
      )}
      {!engine.running && (
        <div role="alert" className="mb-4 rounded-lg border border-warn/40 bg-warn-soft px-4 py-3 text-warn">
          {t("{name} servisi çalışmıyor. Servisler sayfasından başlatın.", { name: engine.label })}
        </div>
      )}

      <div className="flex flex-col gap-4">
        <Panel flush title={t("Veritabanları")}>
          {databases.length === 0 ? (
            <EmptyState
              title={t("Henüz {name} veritabanı yok", { name: engine.label })}
              description={t("Veritabanını ve ona bağlanacak kullanıcıyı tek adımda oluşturabilirsiniz.")}
            />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Ad")}</Th>
                  <Th align="right">{t("Boyut")}</Th>
                  <Th className="w-full">{t("Erişebilen kullanıcılar")}</Th>
                  {showOwner && <Th>{t("Sahibi")}</Th>}
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {databases.map((database) => (
                  <tr key={database.id} className="hover:bg-sunken/50">
                    <Td>
                      <Mono className="font-medium">{database.name}</Mono>
                    </Td>
                    <Td align="right" className="whitespace-nowrap text-ink-2">
                      {database.size === null ? "—" : formatBytes(database.size)}
                    </Td>
                    <Td>
                      {database.grants.length === 0 ? (
                        <span className="text-ink-3">{t("Kimse erişemiyor")}</span>
                      ) : (
                        <div className="flex flex-wrap gap-x-4 gap-y-1">
                          {database.grants.map((grant) => (
                            <span key={grant.db_user_id} className="inline-flex items-center gap-1.5">
                              <Mono>{grant.name}</Mono>
                              <Badge tone={grant.access === "full" ? "accent" : "idle"}>{t(accessLabels[grant.access])}</Badge>
                            </span>
                          ))}
                        </div>
                      )}
                    </Td>
                    {showOwner && (
                      <Td>
                        <Mono>{database.owner}</Mono>
                      </Td>
                    )}
                    <Td align="right" className="py-0.5">
                      <div className="flex justify-end gap-0.5">
                        <IconButton label={t("Erişimi yönet")} onClick={() => setDialog({ kind: "access", database })}>
                          <UserCog className="size-4" />
                        </IconButton>
                        <a
                          href={`/api/v1/databases/${database.id}/export`}
                          download
                          aria-label={t("Dışa aktar")}
                          title={t("Dışa aktar")}
                          className="inline-flex size-8 items-center justify-center rounded-md text-ink-2 hover:bg-sunken hover:text-ink"
                        >
                          <Download className="size-4" />
                        </a>
                        <IconButton label={t("İçe aktar")} onClick={() => setDialog({ kind: "import", database })}>
                          <Upload className="size-4" />
                        </IconButton>
                        <IconButton label={t("Sil")} onClick={() => void confirmDropDatabase(database)}>
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

        <Panel
          flush
          title={t("Kullanıcılar")}
          actions={
            <Button size="sm" icon={<Plus className="size-3.5" />} disabled={!engine.running} onClick={() => setDialog({ kind: "create-user" })}>
              {t("Kullanıcı oluştur")}
            </Button>
          }
        >
          {users.length === 0 ? (
            <EmptyState title={t("Henüz veritabanı kullanıcısı yok")} />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Kullanıcı adı")}</Th>
                  {engine.id === "mariadb" && <Th>{t("Bağlanabildiği yer")}</Th>}
                  <Th className="w-full">{t("Veritabanları")}</Th>
                  {showOwner && <Th>{t("Sahibi")}</Th>}
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {users.map((user) => {
                  const granted = databases.filter((database) => database.grants.some((grant) => grant.db_user_id === user.id));
                  return (
                    <tr key={user.id} className="hover:bg-sunken/50">
                      <Td>
                        <Mono className="font-medium">{user.name}</Mono>
                      </Td>
                      {engine.id === "mariadb" && (
                        <Td className="whitespace-nowrap">
                          {user.host === "localhost" ? t("Bu sunucu") : user.host === "%" ? <Badge tone="warn">{t("Her yer")}</Badge> : <Mono>{user.host}</Mono>}
                        </Td>
                      )}
                      <Td>
                        {granted.length === 0 ? (
                          <span className="text-ink-3">{t("Yok")}</span>
                        ) : (
                          <Mono>{granted.map((database) => database.name).join(", ")}</Mono>
                        )}
                      </Td>
                      {showOwner && (
                        <Td>
                          <Mono>{user.owner}</Mono>
                        </Td>
                      )}
                      <Td align="right" className="py-0.5">
                        <div className="flex justify-end gap-0.5">
                          <IconButton label={t("Parolayı değiştir")} onClick={() => setDialog({ kind: "password", user })}>
                            <KeyRound className="size-4" />
                          </IconButton>
                          <IconButton label={t("Sil")} onClick={() => void confirmDropUser(user)}>
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
      </div>

      {dialog?.kind === "create-database" && <CreateDatabaseModal engine={engine.id} prefix={data.name_prefix} onClose={() => setDialog(null)} />}
      {dialog?.kind === "create-user" && <CreateUserModal engine={engine.id} prefix={data.name_prefix} onClose={() => setDialog(null)} />}
      {dialog?.kind === "password" && <PasswordModal user={dialog.user} onClose={() => setDialog(null)} />}
      {dialog?.kind === "access" && (
        <AccessModal
          // Yetki değiştikçe güncel veriyle yeniden çizilsin diye liste her seferinde sorgudan okunur.
          database={data.databases.find((database) => database.id === dialog.database.id) ?? dialog.database}
          users={data.users}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog?.kind === "import" && <ImportModal database={dialog.database} onClose={() => setDialog(null)} />}
    </>
  );
}
