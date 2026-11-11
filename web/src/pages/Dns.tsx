// DNS sayfası: bölgeler listesi, Cloudflare bağlantısı ve (yönetici için) DNS sunucusu ayarları.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link, useNavigate, useSearchParams } from "react-router";
import { DnsCluster } from "../components/DnsCluster";
import { useFeedback } from "../components/feedback";
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
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
  Textarea,
  Th,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames, formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

// ---- Türler (DnsZone sayfası da kullanır) ----

export type ZoneProvider = "local" | "cloudflare";

export interface Zone {
  id: number;
  user_id: number;
  owner: string;
  domain: string;
  provider: ZoneProvider;
  created_at: number;
}

export type RecordType = "A" | "AAAA" | "CNAME" | "MX" | "TXT" | "NS" | "SRV" | "CAA";

export interface DnsRecord {
  id: string;
  type: RecordType;
  /** Tam ad, örneğin "www.ornek.com". */
  name: string;
  content: string;
  /** Saniye; 1 = otomatik (Cloudflare). */
  ttl: number;
  priority: number | null;
  proxied: boolean | null;
}

interface DnsOverview {
  local_installed: boolean;
  local_running: boolean;
  cloudflare_connected: boolean;
  server_ip: string;
  zones: Zone[];
  available_domains: string[];
}

interface CloudflareZone {
  id: string;
  name: string;
  linked: boolean;
}

interface CloudflareState {
  connected: boolean;
  zones: CloudflareZone[];
}

interface DnsSettings {
  server_ip: string;
  nameservers: string[];
  cloudflare_real_ip: boolean;
}

interface CreatedZone {
  zone: Zone;
  report: string[];
}

export const providerLabels: Record<ZoneProvider, string> = {
  local: "Bu sunucu",
  cloudflare: "Cloudflare",
};

// ---- Bölge ekleme ----

interface AddZoneInitial {
  provider: ZoneProvider;
  domain: string;
}

const providerChoices: { id: ZoneProvider; title: string; description: string }[] = [
  {
    id: "local",
    title: "Bu sunucuda barındır",
    description: "Bölgeyi bu sunucudaki DNS sunucusu yanıtlar. Alan adının ad sunucularını kayıt şirketinizde bu sunucuya yönlendirirsiniz.",
  },
  {
    id: "cloudflare",
    title: "Cloudflare'deki bölgeyi bağla",
    description: "Bölge Cloudflare'de kalır; panel kayıtları Cloudflare hesabınız üzerinden okur ve yazar.",
  },
];

function AddZoneModal({
  onClose,
  overview,
  isAdmin,
  initial,
}: {
  onClose: () => void;
  overview: DnsOverview;
  isAdmin: boolean;
  initial?: AddZoneInitial;
}) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const defaultProvider: ZoneProvider = overview.local_installed ? "local" : overview.cloudflare_connected ? "cloudflare" : "local";
  const [provider, setProvider] = useState<ZoneProvider>(initial?.provider ?? defaultProvider);
  const [localDomain, setLocalDomain] = useState(
    initial?.provider === "local" ? initial.domain : isAdmin ? "" : (overview.available_domains[0] ?? ""),
  );
  const [cloudflareDomain, setCloudflareDomain] = useState(initial?.provider === "cloudflare" ? initial.domain : "");
  const [created, setCreated] = useState<CreatedZone | null>(null);

  const cloudflare = useQuery({
    queryKey: ["dns-cloudflare"],
    queryFn: () => api.get<CloudflareState>("/dns/cloudflare"),
    enabled: provider === "cloudflare" && overview.cloudflare_connected,
  });
  const unlinked = (cloudflare.data?.zones ?? []).filter((zone) => !zone.linked);
  // Seçim boşsa ya da listede yoksa ilk bağlanabilir bölge kullanılır.
  const selectedCloudflare = unlinked.some((zone) => zone.name === cloudflareDomain) ? cloudflareDomain : (unlinked[0]?.name ?? "");

  const domain = provider === "local" ? localDomain.trim().toLowerCase() : selectedCloudflare;

  const create = useMutation({
    mutationFn: () => api.post<CreatedZone>("/dns/zones", { domain, provider }),
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      void queryClient.invalidateQueries({ queryKey: ["dns-cloudflare"] });
      setCreated(result);
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  const unavailable = (id: ZoneProvider) => (id === "local" ? !overview.local_installed : !overview.cloudflare_connected);
  const nothingToAddLocal = provider === "local" && !isAdmin && overview.available_domains.length === 0;

  if (created) {
    return (
      <Modal
        open
        onClose={onClose}
        title={t("Bölge eklendi")}
        description={created.zone.domain}
        footer={
          <>
            <Button onClick={onClose}>{t("Kapat")}</Button>
            <Button
              variant="primary"
              onClick={() => {
                onClose();
                void navigate(`/dns/${created.zone.id}`);
              }}
            >
              {t("Bölgeye git")}
            </Button>
          </>
        }
      >
        {created.report.length === 0 ? (
          <p className="text-ink-2">{t("Bölge oluşturuldu. Kayıtlarını bölge sayfasından yönetebilirsiniz.")}</p>
        ) : (
          <div className="flex flex-col gap-2">
            <p className="font-medium">{t("Otomatik eklenen kayıtlar")}</p>
            <ul className="flex list-disc flex-col gap-1 pl-5 text-ink-2">
              {created.report.map((line, index) => (
                <li key={index}>{line}</li>
              ))}
            </ul>
          </div>
        )}
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={t("Bölge ekle")}
      description={t("Alan adının DNS kayıtlarını bu panelden yönetmek için bir bölge oluşturun.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="add-dns-zone" loading={create.isPending} disabled={!domain || nothingToAddLocal || unavailable(provider)}>
            {t("Bölgeyi ekle")}
          </Button>
        </>
      }
    >
      <form id="add-dns-zone" onSubmit={submit} className="flex flex-col gap-5">
        <fieldset>
          <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Bölgenin barındırılacağı yer")}</legend>
          <div className="grid gap-2 sm:grid-cols-2">
            {providerChoices.map((choice) => {
              const disabled = unavailable(choice.id);
              return (
                <label
                  key={choice.id}
                  className={classNames(
                    "flex flex-col gap-1 rounded-lg border p-3 transition-colors",
                    disabled ? "cursor-not-allowed opacity-60" : "cursor-pointer",
                    provider === choice.id ? "border-accent bg-accent-soft" : "border-line-strong",
                    !disabled && provider !== choice.id && "hover:bg-sunken",
                  )}
                >
                  <span className="flex items-center gap-2 font-medium">
                    <input
                      type="radio"
                      name="provider"
                      className="accent-(--accent)"
                      checked={provider === choice.id}
                      disabled={disabled}
                      onChange={() => setProvider(choice.id)}
                    />
                    {t(choice.title)}
                  </span>
                  <span className="text-[13px] text-ink-2">{t(choice.description)}</span>
                  {disabled && (
                    <span className="text-[13px] text-warn">
                      {choice.id === "local"
                        ? t("DNS sunucusu bu makinede kurulu değil.")
                        : t("Önce Cloudflare sekmesinden API anahtarınızı bağlayın.")}
                    </span>
                  )}
                </label>
              );
            })}
          </div>
        </fieldset>

        {provider === "local" &&
          overview.local_installed &&
          (nothingToAddLocal ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Bölge açabileceğiniz bir alan adı yok. Alan adları hesabınızdaki sitelerden ve posta alan adlarından gelir;")}{" "}
              <Link to="/sites" className="font-medium underline">
                {t("önce Siteler sayfasından bir site ekleyin")}
              </Link>
              .
            </p>
          ) : isAdmin ? (
            <>
              <Input
                label={t("Alan adı")}
                value={localDomain}
                onChange={(event) => setLocalDomain(event.target.value)}
                list="dns-available-domains"
                placeholder="ornek.com"
                className="font-mono text-[13.5px]"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus
                required
              />
              <datalist id="dns-available-domains">
                {overview.available_domains.map((item) => (
                  <option key={item} value={item} />
                ))}
              </datalist>
            </>
          ) : (
            <Select
              label={t("Alan adı")}
              value={localDomain}
              onChange={(event) => setLocalDomain(event.target.value)}
              hint={t("Hesabınızdaki, henüz bölgesi olmayan alan adları listelenir.")}
              className="font-mono text-[13.5px]"
            >
              {overview.available_domains.map((item) => (
                <option key={item} value={item}>
                  {item}
                </option>
              ))}
            </Select>
          ))}

        {provider === "cloudflare" && overview.cloudflare_connected && (
          <>
            {cloudflare.isPending && <LoadingBlock />}
            {cloudflare.isError && <ErrorBlock message={errorMessage(cloudflare.error)} onRetry={() => void cloudflare.refetch()} />}
            {cloudflare.data &&
              (unlinked.length === 0 ? (
                <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
                  {t("Bağlanabilecek bir Cloudflare bölgesi yok. Anahtarınızın eriştiği bütün bölgeler zaten panele bağlı.")}
                </p>
              ) : (
                <Select
                  label={t("Cloudflare bölgesi")}
                  value={selectedCloudflare}
                  onChange={(event) => setCloudflareDomain(event.target.value)}
                  hint={t("API anahtarınızın erişebildiği ve henüz panele bağlanmamış bölgeler listelenir.")}
                  className="font-mono text-[13.5px]"
                >
                  {unlinked.map((zone) => (
                    <option key={zone.id} value={zone.name}>
                      {zone.name}
                    </option>
                  ))}
                </Select>
              ))}
          </>
        )}

        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

// ---- Bölgeler sekmesi ----

function StatusLine({ overview, isAdmin }: { overview: DnsOverview; isAdmin: boolean }) {
  return (
    <div className="mb-4 flex flex-wrap items-center gap-x-6 gap-y-1.5 rounded-lg border border-line bg-surface px-4 sm:px-6 py-2.5 text-[13.5px]">
      <p className="flex flex-wrap items-center gap-2">
        <span className="text-ink-2">{t("DNS sunucusu")}</span>
        {!overview.local_installed ? (
          <>
            <Status tone="idle">{t("kurulu değil")}</Status>
            {isAdmin && (
              <Link to="/components" className="font-medium text-accent hover:underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            )}
          </>
        ) : (
          <Status tone={overview.local_running ? "ok" : "bad"}>{overview.local_running ? t("çalışıyor") : t("durdu")}</Status>
        )}
      </p>
      <p className="flex items-center gap-2">
        <span className="text-ink-2">Cloudflare</span>
        <Status tone={overview.cloudflare_connected ? "ok" : "idle"}>{overview.cloudflare_connected ? t("bağlı") : t("bağlı değil")}</Status>
      </p>
      <p className="ml-auto text-ink-2">
        {t("Sunucu adresi")}: <Mono>{overview.server_ip || "—"}</Mono>
      </p>
    </div>
  );
}

function ZonesTab({ overview, isAdmin, showOwner, onAdd }: { overview: DnsOverview; isAdmin: boolean; showOwner: boolean; onAdd: () => void }) {
  return (
    <>
      <StatusLine overview={overview} isAdmin={isAdmin} />
      <Panel flush>
        {overview.zones.length === 0 ? (
          <EmptyState
            title={t("Henüz DNS bölgesi yok")}
            description={t(
              "İki seçeneğiniz var: bölgeyi bu sunucuda barındırıp alan adının ad sunucularını buraya yönlendirebilir ya da Cloudflare hesabınızı bağlayıp oradaki bölgeleri panelden yönetebilirsiniz.",
            )}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAdd}>
                {t("Bölge ekle")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Alan adı")}</Th>
                <Th>{t("Sağlayıcı")}</Th>
                {showOwner && <Th>{t("Sahibi")}</Th>}
                <Th>{t("Oluşturulma")}</Th>
              </tr>
            </thead>
            <tbody>
              {overview.zones.map((zone) => (
                <tr key={zone.id} className="hover:bg-sunken/50">
                  <Td>
                    <Link to={`/dns/${zone.id}`} className="font-mono text-[14px] font-medium hover:text-accent hover:underline">
                      {zone.domain}
                    </Link>
                  </Td>
                  <Td>
                    <Badge tone={zone.provider === "cloudflare" ? "accent" : "idle"}>{t(providerLabels[zone.provider])}</Badge>
                  </Td>
                  {showOwner && (
                    <Td>
                      <Mono>{zone.owner}</Mono>
                    </Td>
                  )}
                  <Td className="whitespace-nowrap text-ink-2">{formatDateTime(zone.created_at)}</Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
    </>
  );
}

// ---- Cloudflare sekmesi ----

function TokenForm({ onDone, onCancel }: { onDone: () => void; onCancel?: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [token, setToken] = useState("");

  const connect = useMutation({
    mutationFn: () => api.put<unknown>("/dns/cloudflare", { token: token.trim() }),
    onSuccess: () => {
      setToken("");
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      void queryClient.invalidateQueries({ queryKey: ["dns-cloudflare"] });
      feedback.success(t("Cloudflare bağlantısı kaydedildi"));
      onDone();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    connect.mutate();
  }

  return (
    <form onSubmit={submit} className="flex max-w-xl flex-col gap-3">
      <Input
        label={t("API anahtarı (token)")}
        type="password"
        value={token}
        onChange={(event) => setToken(event.target.value)}
        autoComplete="off"
        spellCheck={false}
        className="font-mono text-[13.5px]"
        hint={t("Anahtar Cloudflare ile doğrulanır ve yalnızca bu sunucuda saklanır.")}
        required
      />
      {connect.isError && <ErrorBlock message={errorMessage(connect.error)} />}
      <div className="flex gap-2">
        <Button type="submit" variant="primary" loading={connect.isPending} disabled={!token.trim()}>
          {t("Bağlan")}
        </Button>
        {onCancel && <Button onClick={onCancel}>{t("Vazgeç")}</Button>}
      </div>
    </form>
  );
}

function CloudflareTab({ overview, onLink }: { overview: DnsOverview; onLink: (domain: string) => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [changing, setChanging] = useState(false);
  const connected = overview.cloudflare_connected;
  const cloudflare = useQuery({
    queryKey: ["dns-cloudflare"],
    queryFn: () => api.get<CloudflareState>("/dns/cloudflare"),
    enabled: connected,
  });

  const disconnect = useMutation({
    mutationFn: () => api.delete("/dns/cloudflare"),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      void queryClient.invalidateQueries({ queryKey: ["dns-cloudflare"] });
      feedback.success(t("Cloudflare bağlantısı kesildi"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmDisconnect() {
    const confirmed = await feedback.confirm({
      title: t("Cloudflare bağlantısı kesilsin mi?"),
      description: t("Kayıtlı API anahtarı bu sunucudan silinir. Cloudflare'deki bölgelere ve kayıtlara dokunulmaz. Panele bağlı Cloudflare bölgeleri varsa önce onların bağlantısını kaldırmanız gerekir."),
      confirmLabel: t("Bağlantıyı kes"),
      danger: true,
    });
    if (confirmed) disconnect.mutate();
  }

  return (
    <div className="flex flex-col gap-5">
      <Panel title={t("Cloudflare bağlantısı")}>
        <div className="flex flex-col gap-5">
          <div className="max-w-[75ch] text-ink-2">
            <p>
              {t("Bağlandığınızda panel, Cloudflare hesabınızdaki DNS kayıtlarını okur ve değiştirir; bölgelerinizi buradan yönetirsiniz, kayıtlar Cloudflare'de kalır.")}{" "}
              {t("API anahtarında Zone → DNS → Edit ve Zone → Zone → Read izinleri gerekir; SSL kipi ve geliştirme kipi düğmeleri için Zone Settings → Edit, önbellek temizleme için Cache Purge izni de ekleyin.")}
            </p>
          </div>

          {!connected ? (
            <TokenForm onDone={() => setChanging(false)} />
          ) : (
            <>
              <div className="flex flex-wrap items-center gap-3">
                <Status tone="ok">{t("Bağlı")}</Status>
                {!changing && (
                  <div className="ml-auto flex gap-2">
                    <Button size="sm" onClick={() => setChanging(true)}>
                      {t("Anahtarı değiştir")}
                    </Button>
                    <Button size="sm" variant="danger" loading={disconnect.isPending} onClick={() => void confirmDisconnect()}>
                      {t("Bağlantıyı kes")}
                    </Button>
                  </div>
                )}
              </div>
              {changing && <TokenForm onDone={() => setChanging(false)} onCancel={() => setChanging(false)} />}
            </>
          )}
        </div>
      </Panel>

      {connected && (
        <Panel flush title={t("Erişilebilen Cloudflare bölgeleri")}>
          {cloudflare.isPending && <LoadingBlock />}
          {cloudflare.isError && (
            <div className="p-4">
              <ErrorBlock message={errorMessage(cloudflare.error)} onRetry={() => void cloudflare.refetch()} />
            </div>
          )}
          {cloudflare.data && cloudflare.data.zones.length === 0 && (
            <EmptyState title={t("Bu anahtarın eriştiği bir bölge yok")} description={t("Anahtarın Zone → Zone → Read iznine ve doğru hesaba/bölgelere yetkisi olduğunu denetleyin.")} />
          )}
          {cloudflare.data && cloudflare.data.zones.length > 0 && (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Alan adı")}</Th>
                  <Th>{t("Durum")}</Th>
                  <Th align="right">
                    <span className="sr-only">{t("İşlemler")}</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {cloudflare.data.zones.map((zone) => (
                  <tr key={zone.id} className="hover:bg-sunken/50">
                    <Td>
                      <Mono className="text-[14px] font-medium">{zone.name}</Mono>
                    </Td>
                    <Td>{zone.linked ? <Badge tone="ok">{t("Bağlı")}</Badge> : <span className="text-ink-3">{t("Bağlı değil")}</span>}</Td>
                    <Td align="right">
                      {!zone.linked && (
                        <Button size="sm" onClick={() => onLink(zone.name)}>
                          {t("Panele bağla")}
                        </Button>
                      )}
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
    </div>
  );
}

// ---- Ayarlar sekmesi (yönetici) ----

const linesToList = (text: string) =>
  text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

function SettingsForm({ settings, localInstalled }: { settings: DnsSettings; localInstalled: boolean }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [serverIp, setServerIp] = useState(settings.server_ip);
  const [nameservers, setNameservers] = useState(settings.nameservers.join("\n"));
  const [realIp, setRealIp] = useState(settings.cloudflare_real_ip);

  const save = useMutation({
    mutationFn: (body: DnsSettings) => api.put<DnsSettings>("/dns/settings", body),
    onSuccess: (updated) => {
      queryClient.setQueryData(["dns-settings"], updated);
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      feedback.success(t("DNS ayarları kaydedildi"));
    },
  });

  const repair = useMutation({
    mutationFn: () => api.post<unknown>("/dns/setup"),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["dns"] });
      feedback.success(t("DNS sunucusu yapılandırması onarıldı"));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRepair() {
    const confirmed = await feedback.confirm({
      title: t("DNS sunucusu yapılandırması onarılsın mı?"),
      description: t("PowerDNS yapılandırması panelin beklediği hâle getirilir ve servis yeniden başlatılır. Bölgeleriniz ve kayıtlarınız silinmez."),
      confirmLabel: t("Yapılandırmayı onar"),
    });
    if (confirmed) repair.mutate();
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate({ server_ip: serverIp.trim(), nameservers: linesToList(nameservers), cloudflare_real_ip: realIp });
  }

  return (
    <div className="flex max-w-3xl flex-col gap-4">
      <form onSubmit={submit} className="flex flex-col gap-5">
        <Panel title={t("DNS ayarları")}>
          <div className="flex flex-col gap-5">
            <Input
              label={t("Sunucu adresi (IPv4)")}
              value={serverIp}
              onChange={(event) => setServerIp(event.target.value)}
              placeholder="203.0.113.10"
              hint={t("Sitelerin A kayıtlarında kullanılır. Sunucu bir NAT arkasındaysa dış adresi yazın.")}
              className="max-w-xs font-mono text-[13.5px]"
              spellCheck={false}
              required
            />
            <Textarea
              label={t("Ad sunucuları")}
              value={nameservers}
              onChange={(event) => setNameservers(event.target.value)}
              placeholder={"ns1.ornek.com\nns2.ornek.com"}
              hint={t("Her satıra bir ad. Boş bırakılırsa her bölge kendi ns1/ns2 adlarını kullanır; ortak ad sunucularınız varsa en az iki tane yazın.")}
              spellCheck={false}
              className="max-w-md"
            />
            <Checkbox
              label={t("Cloudflare arkasındaki sitelerde ziyaretçinin gerçek adresini kullan")}
              hint={t("nginx yalnızca Cloudflare'in adres aralıklarından gelen bağlantıların bildirdiği adrese güvenir; kaydettiğinizde aralıklar güncellenir.")}
              checked={realIp}
              onChange={(event) => setRealIp(event.target.checked)}
            />
          </div>
        </Panel>
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </div>
      </form>

      {localInstalled && (
        <Panel title={t("DNS sunucusu")}>
          <div className="flex flex-wrap items-center justify-between gap-3">
            <p className="max-w-[60ch] text-[13.5px] text-ink-2">
              {t("DNS sunucusu çalışmıyorsa ya da yapılandırması bozulduysa panelin yapılandırmasını yeniden uygulayın.")}
            </p>
            <Button loading={repair.isPending} onClick={() => void confirmRepair()}>
              {t("DNS sunucusu yapılandırmasını onar")}
            </Button>
          </div>
        </Panel>
      )}
    </div>
  );
}

function SettingsTab({ localInstalled }: { localInstalled: boolean }) {
  const settings = useQuery({ queryKey: ["dns-settings"], queryFn: () => api.get<DnsSettings>("/dns/settings") });
  if (settings.isPending) return <LoadingBlock />;
  if (settings.isError) return <ErrorBlock message={errorMessage(settings.error)} onRetry={() => void settings.refetch()} />;
  // `key`, kaydetme sonrası gelen değerlerle formun sıfırlanmasını sağlar.
  return <SettingsForm key={JSON.stringify(settings.data)} settings={settings.data} localInstalled={localInstalled} />;
}

// ---- Sayfa ----

type DnsTab = "zones" | "cloudflare" | "cluster" | "settings";

/** Sekmenin adres çubuğundaki karşılığı (`?sekme=`). */
const tabParams: Record<DnsTab, string> = {
  zones: "bolgeler",
  cloudflare: "cloudflare",
  cluster: "kume",
  settings: "ayarlar",
};

export function DnsPage() {
  const me = useMe();
  const isAdmin = me.role === "admin";
  const showOwner = isAdmin || me.role === "reseller";
  const [searchParams, setSearchParams] = useSearchParams();
  const [adding, setAdding] = useState<AddZoneInitial | "blank" | null>(null);
  const overview = useQuery({ queryKey: ["dns"], queryFn: () => api.get<DnsOverview>("/dns") });

  const requested = (Object.keys(tabParams) as DnsTab[]).find((item) => tabParams[item] === searchParams.get("sekme"));
  const adminOnly = requested === "settings" || requested === "cluster";
  const tab: DnsTab = adminOnly && !isAdmin ? "zones" : (requested ?? "zones");

  if (overview.isPending) return <LoadingBlock />;
  if (overview.isError) return <ErrorBlock message={errorMessage(overview.error)} onRetry={() => void overview.refetch()} />;

  const data = overview.data;
  const tabs: { id: DnsTab; label: string }[] = [
    { id: "zones", label: t("Bölgeler") },
    { id: "cloudflare", label: "Cloudflare" },
    ...(isAdmin ? [{ id: "cluster" as const, label: t("Küme") }, { id: "settings" as const, label: t("Ayarlar") }] : []),
  ];

  return (
    <>
      <PageHeader
        title={t("DNS")}
        description={t("Alan adlarınızın DNS kayıtları; bölgeler bu sunucuda ya da Cloudflare'de barındırılabilir.")}
        actions={
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setAdding("blank")}>
            {t("Bölge ekle")}
          </Button>
        }
      />
      <Tabs
        tabs={tabs}
        value={tab}
        label={t("DNS bölümleri")}
        onChange={(next) => setSearchParams(next === "zones" ? {} : { sekme: tabParams[next] }, { replace: true })}
      />
      {tab === "zones" && <ZonesTab overview={data} isAdmin={isAdmin} showOwner={showOwner} onAdd={() => setAdding("blank")} />}
      {tab === "cloudflare" && <CloudflareTab overview={data} onLink={(domain) => setAdding({ provider: "cloudflare", domain })} />}
      {tab === "cluster" && <DnsCluster localInstalled={data.local_installed} />}
      {tab === "settings" && <SettingsTab localInstalled={data.local_installed} />}
      {adding && (
        <AddZoneModal
          onClose={() => setAdding(null)}
          overview={data}
          isAdmin={isAdmin}
          initial={adding === "blank" ? undefined : adding}
        />
      )}
    </>
  );
}
