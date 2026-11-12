import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Download, ExternalLink, KeyRound, Plus, Trash2, Upload, UserCog, Wand2, Wrench } from "lucide-react";
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
import { formatBytes, generatePassword } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";
import { JobLog } from "./Jobs";
import { SlowQueries } from "./SlowQueries";

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

function PasswordField({ value, onChange, label }: { value: string; onChange: (next: string) => void; label: string }) {
  return (
    <div className="flex items-start gap-2">
      <div className="min-w-0 flex-1">
        <Input
          label={label}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          hint={t("En az 10 karakter. Uygulamanızın ayar dosyasına yazacağınız parola.")}
          className="font-mono text-[13.5px]"
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
          className="font-mono text-[13.5px]"
          required
        />
      )}
    </div>
  );
}

function CreateDatabaseModal({
  initialEngine,
  engines,
  prefix,
  onClose,
  onCreated,
}: {
  /** Pencere açıldığında seçili gelen tür (o an bakılan sekme). */
  initialEngine: DbEngine;
  /** Kurulu ve çalışan veritabanı türleri. */
  engines: { id: DbEngine; label: string }[];
  prefix: string;
  onClose: () => void;
  onCreated: (engine: DbEngine) => void;
}) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [engine, setEngine] = useState<DbEngine>(initialEngine);
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
      onCreated(engine);
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
      <form id="create-database" onSubmit={submit} className="flex flex-col gap-5">
        {engines.length > 1 ? (
          <Select label={t("Veritabanı türü")} value={engine} onChange={(event) => setEngine(event.target.value as DbEngine)}>
            {engines.map((item) => (
              <option key={item.id} value={item.id}>
                {item.label}
              </option>
            ))}
          </Select>
        ) : (
          <p className="text-[13.5px] text-ink-2">
            {t("Tür")}: <span className="font-medium text-ink">{engines[0]?.label}</span>
          </p>
        )}
        <Input
          label={t("Veritabanı adı")}
          value={name}
          onChange={(event) => setName(event.target.value.toLowerCase())}
          hint={prefix ? t("“{prefix}” ile başlamalı; küçük harf, rakam ve _ kullanın.", { prefix }) : t("Küçük harf, rakam ve _ kullanın.")}
          className="font-mono text-[13.5px]"
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
              className="font-mono text-[13.5px]"
              autoComplete="off"
              spellCheck={false}
              required
            />
            <PasswordField label={t("Parola")} value={password} onChange={setPassword} />
            {engine === "mariadb" && <HostField value={host} onChange={setHost} />}
            <p className="rounded-md bg-accent-soft px-3 py-2 text-[13.5px] text-accent">
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
        className="flex flex-col gap-5"
      >
        <Input
          label={t("Kullanıcı adı")}
          value={name}
          onChange={(event) => setName(event.target.value.toLowerCase())}
          hint={prefix ? t("“{prefix}” ile başlamalı.", { prefix }) : undefined}
          className="font-mono text-[13.5px]"
          autoComplete="off"
          spellCheck={false}
          autoFocus
          required
        />
        <PasswordField label={t("Parola")} value={password} onChange={setPassword} />
        {engine === "mariadb" && <HostField value={host} onChange={setHost} />}
        <p className="text-[13.5px] text-ink-3">{t("Kullanıcıyı oluşturduktan sonra erişebileceği veritabanlarını “Erişim” düğmesinden seçin.")}</p>
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
        className="flex flex-col gap-5"
      >
        <PasswordField label={t("Yeni parola")} value={password} onChange={setPassword} />
        <p className="text-[13.5px] text-ink-3">{t("Bu kullanıcıyla bağlanan uygulamaların ayar dosyasını da güncellemeyi unutmayın.")}</p>
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
                  {user.host && user.host !== "localhost" && <span className="ml-2 text-[13px] text-ink-3">@{user.host}</span>}
                  <p className="text-[13px] text-ink-3">{current === "none" ? t("Bu veritabanına erişemez") : t(accessHints[current])}</p>
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
        <div className="flex flex-col gap-5">
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
            <p className="text-[13.5px] text-ink-3">
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

/** Veritabanı bakım işlemleri (sunucudaki `action` değerleri). */
type MaintenanceAction = "check" | "repair" | "optimize";

function CopyModal({ database, onClose }: { database: DatabaseInfo; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [name, setName] = useState(`${database.name}_kopya`.slice(0, 48));
  const [jobId, setJobId] = useState<number | null>(null);

  const copy = useMutation({
    mutationFn: () => api.post<{ job_id: number; id: number }>(`/databases/${database.id}/copy`, { name }),
    onSuccess: (result) => {
      setJobId(result.job_id);
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
    },
  });

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{name} veritabanını kopyala", { name: database.name })}
      wide={jobId !== null}
      footer={
        jobId === null ? (
          <>
            <Button onClick={onClose}>{t("Vazgeç")}</Button>
            <Button variant="primary" type="submit" form="copy-database" loading={copy.isPending}>
              {t("Kopyala")}
            </Button>
          </>
        ) : undefined
      }
    >
      {jobId === null ? (
        <form
          id="copy-database"
          onSubmit={(event) => {
            event.preventDefault();
            copy.mutate();
          }}
          className="flex flex-col gap-5"
        >
          <p className="text-ink-2">
            {t("Aynı hesapta yeni bir veritabanı açılır ve içeriği bu veritabanından kopyalanır. Kopya bağımsızdır; kullanıcı erişimleri kopyalanmaz, isterseniz sonradan verirsiniz.")}
          </p>
          <Input
            label={t("Yeni veritabanı adı")}
            value={name}
            onChange={(event) => setName(event.target.value.toLowerCase())}
            hint={t("Küçük harf, rakam ve _ kullanın; hesap önekiyle başlamalı.")}
            className="font-mono text-[13.5px]"
            autoComplete="off"
            spellCheck={false}
            autoFocus
            required
          />
          {copy.isError && <ErrorBlock message={errorMessage(copy.error)} />}
        </form>
      ) : (
        <JobLog jobId={jobId} onDone={() => void queryClient.invalidateQueries({ queryKey: ["databases"] })} />
      )}
    </Modal>
  );
}

function MaintenanceModal({ database, onClose }: { database: DatabaseInfo; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [jobId, setJobId] = useState<number | null>(null);
  const [running, setRunning] = useState(false);

  const start = useMutation({
    mutationFn: (action: MaintenanceAction) => api.post<{ job_id: number }>(`/databases/${database.id}/maintenance`, { action }),
    onSuccess: (result) => {
      setJobId(result.job_id);
      setRunning(true);
    },
  });

  const actions: { action: MaintenanceAction; label: string; hint: string }[] =
    database.engine === "mariadb"
      ? [
          { action: "check", label: t("Denetle"), hint: t("Tabloların bozuk olup olmadığına bakar; veriyi değiştirmez.") },
          { action: "repair", label: t("Onar"), hint: t("Bozuk tabloları onarmayı dener (InnoDB tabloları bu işlemi desteklemez).") },
          { action: "optimize", label: t("İyileştir"), hint: t("Tabloları yeniden düzenler ve istatistikleri yeniler; sürerken tablolar kilitlenebilir.") },
        ]
      : [{ action: "optimize", label: t("İyileştir"), hint: t("VACUUM ANALYZE çalıştırır: ölü satırları temizler, istatistikleri yeniler.") }];

  return (
    <Modal open onClose={onClose} title={t("{name} bakımı", { name: database.name })} wide>
      <ul className="flex flex-col divide-y divide-line">
        {actions.map((item) => (
          <li key={item.action} className="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-3">
            <p className="min-w-0 flex-1 text-[13.5px] text-ink-2">{item.hint}</p>
            <Button disabled={running} loading={start.isPending && start.variables === item.action} onClick={() => start.mutate(item.action)}>
              {item.label}
            </Button>
          </li>
        ))}
      </ul>
      {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      {jobId !== null && (
        <div className="mt-2">
          <JobLog
            jobId={jobId}
            onDone={() => {
              setRunning(false);
              void queryClient.invalidateQueries({ queryKey: ["databases"] });
            }}
          />
        </div>
      )}
    </Modal>
  );
}

type Dialog =
  | { kind: "create-database" }
  | { kind: "create-user" }
  | { kind: "password"; user: DbUserInfo }
  | { kind: "access"; database: DatabaseInfo }
  | { kind: "import"; database: DatabaseInfo }
  | { kind: "copy"; database: DatabaseInfo }
  | { kind: "maintenance"; database: DatabaseInfo };

/** Sayfadaki sekmeler: veritabanı türleri ve (yöneticiye) yavaş sorgu günlüğü. */
type View = DbEngine | "slow";

export function DatabasesPage() {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [engineChoice, setEngineChoice] = useState<DbEngine | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const [slowView, setSlowView] = useState(false);

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
  // MariaDB phpMyAdmin ile, PostgreSQL Adminer ile açılır.
  const toolName = engine?.id === "postgresql" ? "Adminer" : "phpMyAdmin";
  const toolInstalled = engine?.id === "postgresql" ? data.tools.adminer : data.tools.phpmyadmin;

  /** Veritabanını web aracında yeni sekmede açar; panel geçici bir hesapla oturumu kendisi başlatır. */
  async function openTool(database: DatabaseInfo) {
    // Sekme tıklama anında açılır; adres sonradan verilir. Aksi hâlde tarayıcı açılır pencereyi engeller.
    const tab = window.open("about:blank", "_blank");
    try {
      const result = await api.post<{ url: string }>(`/databases/${database.id}/open`);
      if (tab) tab.location.href = result.url;
      else window.location.href = result.url;
    } catch (error) {
      tab?.close();
      feedback.error(error);
    }
  }
  const showOwner = me.role === "admin";
  // Yavaş sorgu günlüğü bütün hesapların sorgularını içerir; yalnızca yöneticiye ve yalnızca MariaDB için.
  const showSlow = me.role === "admin" && available.some((item) => item.id === "mariadb");
  const tabs: { id: View; label: string }[] = available.map((item) => ({ id: item.id, label: item.label }));
  if (showSlow) tabs.push({ id: "slow", label: t("Yavaş sorgular") });

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
      {tabs.length > 1 && (
        <Tabs
          label={t("Veritabanı türü")}
          tabs={tabs}
          value={slowView && showSlow ? "slow" : engine.id}
          onChange={(next) => {
            if (next === "slow") {
              setSlowView(true);
            } else {
              setSlowView(false);
              setEngineChoice(next);
            }
          }}
        />
      )}
      {slowView && showSlow ? (
        <SlowQueries />
      ) : (
        <>
          {!engine.running && (
            <div role="alert" className="mb-4 rounded-lg border border-warn/40 bg-warn-soft px-4 sm:px-6 py-3 text-warn">
              {t("{name} servisi çalışmıyor. Servisler sayfasından başlatın.", { name: engine.label })}
            </div>
          )}
          {engine.running && !toolInstalled && me.role === "admin" && (
            <p className="mb-4 rounded-lg border border-line bg-surface px-4 sm:px-6 py-2.5 text-[13.5px] text-ink-2">
              {t("Veritabanlarını tarayıcıdan yönetmek için {tool} kurabilirsiniz; kurulduğunda her veritabanının satırında açma düğmesi görünür.", { tool: toolName })}{" "}
              <Link to="/components" className="font-medium text-accent hover:underline">
                {t("Bileşenler sayfasına git")}
              </Link>
            </p>
          )}

          <div className="flex flex-col gap-5">
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
                      <Th align="right">{t("Tablo")}</Th>
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
                        <Td align="right" className="tabular whitespace-nowrap text-ink-2">
                          {database.tables === null ? "—" : database.tables}
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
                            {toolInstalled && (
                              <Button size="sm" className="mr-1 self-center" icon={<ExternalLink className="size-3.5" />} onClick={() => void openTool(database)}>
                                {toolName}
                              </Button>
                            )}
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
                            <IconButton label={t("Kopyala")} disabled={!engine.running} onClick={() => setDialog({ kind: "copy", database })}>
                              <Copy className="size-4" />
                            </IconButton>
                            <IconButton label={t("Denetle / Onar / İyileştir")} disabled={!engine.running} onClick={() => setDialog({ kind: "maintenance", database })}>
                              <Wrench className="size-4" />
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
        </>
      )}

      {dialog?.kind === "create-database" && (
        <CreateDatabaseModal
          initialEngine={engine.id}
          engines={available.filter((item) => item.running)}
          prefix={data.name_prefix}
          onClose={() => setDialog(null)}
          onCreated={setEngineChoice}
        />
      )}
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
      {dialog?.kind === "copy" && <CopyModal database={dialog.database} onClose={() => setDialog(null)} />}
      {dialog?.kind === "maintenance" && <MaintenanceModal database={dialog.database} onClose={() => setDialog(null)} />}
    </>
  );
}
