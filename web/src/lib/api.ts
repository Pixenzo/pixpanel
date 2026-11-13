import { getLanguage, t } from "./i18n";
// Panel API'siyle konuşan ince istemci.

const BASE = "/api/v1";

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

/** Oturum düştüğünde uygulamanın giriş ekranına dönmesi için yayılan olay. */
export const UNAUTHORIZED_EVENT = "pixpanel:unauthorized";

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  // Sunucu metinlerini hesabın kendi diline çevirir; bu başlık yalnızca oturum yokken
  // (giriş ekranındaki hata iletileri) kullanılır.
  const headers: Record<string, string> = { "x-pixpanel-csrf": "1", "accept-language": getLanguage() };
  // Dosya yüklemelerinde içerik türünü (sınır değeriyle birlikte) tarayıcı belirler.
  const isForm = body instanceof FormData;
  if (body !== undefined && !isForm) headers["content-type"] = "application/json";

  let response: Response;
  try {
    response = await fetch(BASE + path, {
      method,
      headers,
      credentials: "same-origin",
      body: body === undefined ? undefined : isForm ? body : JSON.stringify(body),
    });
  } catch {
    throw new ApiError(0, "network", t("Panele ulaşılamıyor. Bağlantınızı ve sunucunun çalıştığını denetleyin."));
  }

  if (!response.ok) {
    let code = "unknown";
    let message = `İstek başarısız oldu (${response.status})`;
    try {
      const payload = await response.json();
      code = payload?.error?.code ?? code;
      message = payload?.error?.message ?? message;
    } catch {
      // Gövde JSON değilse genel ileti kullanılır.
    }
    if (response.status === 401 && path !== "/auth/login") {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
    }
    throw new ApiError(response.status, code, message);
  }
  return (await response.json()) as T;
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body ?? {}),
  patch: <T>(path: string, body: unknown) => request<T>("PATCH", path, body),
  put: <T>(path: string, body: unknown) => request<T>("PUT", path, body),
  delete: <T>(path: string) => request<T>("DELETE", path),
  upload: <T>(path: string, form: FormData) => request<T>("POST", path, form),
};

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : t("Beklenmeyen bir hata oluştu");
}

// ---- Türler ----

export type Role = "admin" | "reseller" | "user" | "member";

export interface Me {
  id: number;
  username: string;
  email: string;
  role: Role;
  parent_id: number | null;
  permissions: string[];
  language: string;
  /** Oturumu bu hesap adına başka biri (yönetici ya da bayi) açtıysa onun adı. */
  impersonated_by: string | null;
}

export interface Sample {
  ts: number;
  cpu: number;
  mem_used: number;
  mem_total: number;
  swap_used: number;
  swap_total: number;
  load1: number;
  load5: number;
  load15: number;
  net_rx: number;
  net_tx: number;
}

export interface Disk {
  filesystem: string;
  fs_type: string;
  mount: string;
  total: number;
  used: number;
  available: number;
}

export interface HostInfo {
  hostname: string;
  os: string;
  kernel: string;
  cpu_model: string;
  cpu_cores: number;
  uptime_secs: number;
  virtualization: string;
  panel_version: string;
}

export interface Overview {
  host: HostInfo;
  current: Sample;
  disks: Disk[];
}

export interface Process {
  pid: number;
  ppid: number;
  user: string;
  name: string;
  command: string;
  state: string;
  cpu: number;
  memory: number;
  threads: number;
  elapsed_secs: number;
}

export interface Service {
  name: string;
  description: string;
  load: string;
  active: string;
  sub: string;
  enabled: string;
}

export interface UserInfo {
  id: number;
  username: string;
  email: string;
  role: Role;
  parent_id: number | null;
  parent_username: string | null;
  permissions: string[];
  status: "active" | "suspended";
  language: string;
  created_at: number;
  last_login_at: number | null;
  package_id: number | null;
  package_name: string | null;
  /** Yalnız bayi satırlarında: müşterilerinin toplam kullanımı. */
  customer_totals?: CustomerTotals | null;
}

/** Paketin sınırları; `null` sınırsız demektir. */
export interface PackageLimits {
  sites: number | null;
  databases: number | null;
  mail_domains: number | null;
  mailboxes: number | null;
  ftp_accounts: number | null;
  dns_zones: number | null;
  disk_mb: number | null;
  /** İşlemci sınırı: bir çekirdeğin yüzdesi (100 = bir tam çekirdek). */
  cpu_percent: number | null;
  /** Hesabın bütün süreçlerinin toplam bellek sınırı (MB). */
  memory_mb: number | null;
  /** Site başına en çok PHP işçisi (`pm.max_children`). */
  php_workers: number | null;
  /** En çok süreç/iş parçacığı sayısı. */
  processes: number | null;
  /** Aylık trafik sınırı (GB); hesabın bütün sitelerinin takvim ayındaki toplam aktarımı. */
  traffic_gb: number | null;
  /** Sınır dolunca: `notify` (yalnız bildirim) ya da `suspend` (hesabın siteleri askıya alınır); boş = `notify`. */
  traffic_action: "notify" | "suspend" | null;
  /** Hesabın Redis örneğinin bellek sınırı (MB). `null`: pakette sınır yok; 0: pakette Redis yok. */
  redis_memory_mb: number | null;
  /** Hesabın seçebileceği PHP sürümleri; boş: kurulu bütün sürümler. */
  allowed_php_versions: string[];
}

/** Bayinin müşterilerinin toplam kullanımı. */
export interface CustomerTotals {
  accounts: number;
  sites: number;
  databases: number;
  mail_domains: number;
  mailboxes: number;
  disk_bytes: number;
  /** Bu ayki toplam trafik (bayt). */
  traffic_bytes: number;
}

/** Kaynak sınırlarının sunucudaki durumu ve anlık kullanım (yalnız hesabın kaynak sınırı varsa). */
export interface ResourceRuntime {
  enforced: boolean;
  /** Sınırlar uygulanamıyorsa nedeni. */
  warning: string | null;
  memory_bytes: number | null;
  cpu_seconds: number | null;
  /** Önceki ölçümden bu yana işlemci kullanımı (bir çekirdeğin yüzdesi). */
  cpu_percent: number | null;
  tasks: number | null;
}

export interface PackageInfo {
  id: number;
  owner_id: number;
  name: string;
  limits: PackageLimits;
  /** Bu paketi kullanan hesap sayısı. */
  accounts: number;
  created_at: number;
}

export interface AccountUsage {
  package: string | null;
  limits: PackageLimits;
  sites: number;
  databases: number;
  mail_domains: number;
  mailboxes: number;
  ftp_accounts: number;
  dns_zones: number;
  /** İçinde bulunulan ayda sitelerin aktardığı veri (bayt). */
  traffic_bytes: number;
  disk_bytes: number | null;
  /** `disk_bytes` içindeki veritabanları payı (bayt). */
  database_bytes: number | null;
  disk_measured_at: number | null;
  runtime: ResourceRuntime | null;
  /** Aylık trafik sınırı yüzünden şu an askıda olan site sayısı. */
  traffic_suspended_sites: number;
  /** Yalnız bayi hesabında: müşterilerinin toplam kullanımı. */
  customers: CustomerTotals | null;
}

export interface AuditEntry {
  id: number;
  username: string;
  action: string;
  target: string;
  detail: string;
  ip: string;
  created_at: number;
}

export interface JobInfo {
  id: number;
  kind: string;
  title: string;
  status: "running" | "success" | "failed";
  user_id: number | null;
  created_at: number;
  finished_at: number | null;
}

export interface SessionInfo {
  id: number;
  ip: string;
  user_agent: string;
  created_at: number;
  expires_at: number;
  current: boolean;
}

export interface TokenInfo {
  id: number;
  name: string;
  prefix: string;
  read_only: boolean;
  allowed_ips: string;
  last_used_at: number | null;
  expires_at: number | null;
  created_at: number;
  /** Anahtarın erişebildiği alanlar; null: kapsam yok (hesabın bütün yetkileri). */
  scope: string[] | null;
}

export interface ComponentStatus {
  id: string;
  name: string;
  description: string;
  category: "web" | "php" | "node" | "database" | "container" | "mail";
  installed: boolean;
  version: string | null;
  service: string | null;
  running: boolean;
}

/** Kurulu bir Node.js ana sürümünün yama güncellemesi durumu. */
export interface NodeVersionInfo {
  major: string;
  installed: string;
  /** En son yama sürümü; öğrenilemediyse null. */
  latest: string | null;
  update_available: boolean;
  /** Bekleyen güncellemelerden biri güvenlik düzeltmesi içeriyor. */
  security: boolean;
  /** Bu sürümü kullanan Node.js sitesi sayısı. */
  sites: number;
  checked_at: number | null;
}

export type SiteKind = "php" | "static" | "proxy" | "node" | "python";

export interface NodeSettings {
  version: string;
  command: string;
  env: { key: string; value: string }[];
  memory_mb: number;
  /** Yalnız Python siteleri: boş ya da "system" dağıtımın Python'u, aksi hâlde kurulu uv sürümü (ör. "3.12"). */
  python: string;
}

export interface NodeStatus {
  state: string;
  sub_state: string;
  pid: number;
  memory_bytes: number | null;
  restarts: number;
  port: number;
  /** Yalnız Python siteleri: bağımlılıkların kurulabileceği dosya; yoksa null. */
  dependencies: string | null;
}

export interface ProxyRule {
  path: string;
  target: string;
  websocket: boolean;
  strip_prefix: boolean;
}

export interface PhpDirective {
  key: string;
  value: string;
}

/** `null` olan alanlar PHP sürümünün genel ayarını devralır. */
export interface PhpSettings {
  memory_limit: string | null;
  upload_max_filesize: string | null;
  post_max_size: string | null;
  max_execution_time: number | null;
  max_input_vars: number | null;
  display_errors: boolean | null;
  max_children: number;
  allow_exec: boolean;
  open_basedir: boolean;
  custom: PhpDirective[];
}

export interface Site {
  id: number;
  user_id: number;
  owner: string;
  domain: string;
  aliases: string[];
  kind: SiteKind;
  public_dir: string;
  php_version: string | null;
  php_settings: PhpSettings;
  proxy_target: string | null;
  proxy_websocket: boolean;
  ssl_mode: string;
  force_https: boolean;
  hsts: boolean;
  nginx_custom: string;
  status: "active" | "suspended";
  created_at: number;
  node: NodeSettings;
  proxy_rules: ProxyRule[];
  /** Uygulamaya göre hazır web sunucusu ayarı. */
  template: string;
  /** Sitenin bağlandığı sunucu adresi; boşsa bütün adresler. */
  ip: string;
  rules: SiteRules;
  /** Site bir WordPress hazırlık ortamıysa asıl sitenin kimliği. */
  staging_of: number | null;
  /** PHP sürümü sahibinin paketindeki izinli sürümlerin dışında mı (paket sonradan daraltıldıysa)? */
  php_out_of_package: boolean;
}

export type DbEngine = "mariadb" | "postgresql";
export type DbAccess = "full" | "readwrite" | "readonly";

export interface DatabaseInfo {
  id: number;
  engine: DbEngine;
  name: string;
  owner: string;
  size: number | null;
  /** Tablo sayısı (görünümler hariç). */
  tables: number | null;
  grants: { db_user_id: number; name: string; host: string; access: DbAccess }[];
  created_at: number;
}

export interface SlowQueryEntry {
  /** Sorgunun başladığı an (Unix saniyesi); 0 ise günlükte yok. */
  time: number;
  query_time: number;
  lock_time: number;
  rows_sent: number;
  rows_examined: number;
  user: string;
  host: string;
  database: string;
  query: string;
}

export interface SlowQueryLog {
  enabled: boolean;
  /** Eşik (saniye). */
  threshold: number;
  thresholds: number[];
  file: string;
  /** Günlük dosyaya yazılıyor mu (değilse kayıtlar okunamaz). */
  file_output: boolean;
  file_size: number;
  /** Ayar yeniden başlatmaya dayanıyor mu. */
  persistent: boolean;
  entries: SlowQueryEntry[];
}

export interface DbUserInfo {
  id: number;
  engine: DbEngine;
  name: string;
  host: string;
  owner: string;
  created_at: number;
}

/** Bir veritabanı motorunun uzaktan erişim durumu (yönetici görünümü; `GET /databases/remote`). */
export interface DbRemoteStatus {
  id: DbEngine;
  label: string;
  installed: boolean;
  running: boolean;
  /** Panelin yazdığı sunucu ayarı var mı. */
  remote_enabled: boolean;
  /** Sunucunun dinlediği adres; sunucu çalışmıyorsa yok. */
  listen: string | null;
  port: number;
  local_only: boolean | null;
  /** PostgreSQL: sunucuda TLS açık mı. */
  tls: boolean | null;
  /** Yalnızca bu sunucu dışında bir adresten bağlanabilen kullanıcı sayısı. */
  remote_users: number;
  firewall_enabled: boolean;
  firewall_port_open: boolean;
}

export interface DatabaseOverview {
  engines: { id: DbEngine; label: string; installed: boolean; running: boolean; remote_enabled: boolean }[];
  databases: DatabaseInfo[];
  users: DbUserInfo[];
  name_prefix: string;
  tools: { phpmyadmin: boolean; adminer: boolean };
}

export interface CertInfo {
  subject: string;
  issuer: string;
  names: string[];
  not_before: number;
  not_after: number;
  self_signed: boolean;
}

export interface SslStatus {
  mode: "none" | "selfsigned" | "custom" | "letsencrypt" | "letsencrypt_dns" | "letsencrypt_wildcard";
  force_https: boolean;
  hsts: boolean;
  certificate: CertInfo | null;
}

export interface PhpGlobal {
  memory_limit: string;
  upload_max_filesize: string;
  post_max_size: string;
  max_execution_time: number;
  max_input_vars: number;
  display_errors: boolean;
  timezone: string;
  opcache: boolean;
  opcache_memory: number;
  custom: PhpDirective[];
}

export interface SiteOptions {
  nginx_installed: boolean;
  php_versions: string[];
  /** Hesabın paketinin izin verdiği PHP sürümleri; boş: kısıt yok (yönetici her zaman boş görür). */
  php_allowed: string[];
  owners: { id: number; username: string }[];
  php_defaults: Record<string, PhpGlobal>;
  node_versions: string[];
  /** Sistemdeki Python sürümü; kurulu değilse boş. */
  python_versions: string[];
  /** Yöneticinin uv ile kurduğu Python sürümleri. */
  python_runtimes: string[];
  /** Hesabın sitelerini bağlayabileceği sunucu adresleri. */
  ips: { address: string; label: string; user_id: number | null }[];
}

export interface SiteRedirect {
  from: string;
  to: string;
  /** 301 (kalıcı) ya da 302 (geçici). */
  code: number;
  /** Yalnızca tam bu adres mi, yoksa altındaki adresler de mi? */
  exact: boolean;
}

export interface ProtectedArea {
  path: string;
  label: string;
  /** Parola yalnızca gönderilirken bulunur; sunucu geri göndermez. */
  users: { name: string; password?: string }[];
}

/** Sitenin yönlendirme, koruma ve erişim kuralları. */
export interface SiteRules {
  /** Asıl ad tercihi: "www", "apex" ya da boş. */
  www: string;
  redirects: SiteRedirect[];
  protected: ProtectedArea[];
  blocked: string[];
  allowed: string[];
  maintenance: boolean;
  maintenance_allowed: string[];
  hotlink_protection: boolean;
  error_pages: { not_found: string; forbidden: string; server_error: string };
  /** Durağan dosyaların tarayıcıda saklanma süresi: "" (varsayılan, 30 gün), "off", "1h", "1d", "7d", "30d", "1y". */
  browser_cache: string;
  /** Sunucu önbelleği (PHP ve vekil siteleri); süre: "1m", "5m", "10m", "30m", "1h", "6h", "1d". */
  server_cache: { enabled: boolean; ttl: string };
  /** Yalnızca Cloudflare adreslerinden gelen bağlantılar kabul edilir. */
  cloudflare_only: boolean;
}

/** Bir hesabın kendi Redis örneği. Parola yalnızca hesabın kendi görünümünde bulunur. */
export interface RedisAccountInstance {
  user_id: number;
  username: string;
  running: boolean;
  host: string;
  port: number;
  socket: string;
  memory_mb: number;
  created_at: number;
  used_memory: number | null;
  keys: number | null;
  version: string;
  password?: string;
  /** Yönetici listesinde: bellek sınırı paketten geliyorsa örnek başına değiştirilemez. */
  memory_from_package?: boolean;
  /** Yönetici listesinde: hesabın paketi. */
  package?: string | null;
  /** Yönetici listesinde: paket Redis'e izin veriyor mu? */
  allowed?: boolean;
}

export interface RedisAccountView {
  /** Redis bu sunucuda kurulu mu? */
  available: boolean;
  /** Paket Redis'e izin veriyor mu (`redis_memory_mb` 0 değilse)? Değilse örnek açılamaz, açık örnek durdurulmuştur. */
  allowed: boolean;
  /** Hesabın paketinin adı. */
  package: string | null;
  /** Bellek sınırı paketten geliyorsa değeri (MB). */
  package_memory_mb: number | null;
  instance: RedisAccountInstance | null;
}

/** Bu sunucudan bölge çeken bir ikincil ad sunucusu. */
export interface DnsClusterSecondary {
  address: string;
  /** İkincil bir PixPanel ise panelinin adresi; boşsa bölge listesi eşitlenmez. */
  panel_url: string;
  token_set: boolean;
  fingerprint: string;
  last_sync_at: number | null;
  last_error: string;
  synced_zones: number | null;
  conflicts: string[];
}

export interface DnsCluster {
  local_installed: boolean;
  secondaries: DnsClusterSecondary[];
  /** Bu sunucunun ikincil olarak bölge çektiği birinciller. */
  replica_of: { primary: string; zones: number }[];
}

/** Sunucunun bir IP adresi ve panelin onun hakkında bildikleri. */
export interface ServerAddress {
  address: string;
  prefix: number;
  interface: string;
  family: "ipv4" | "ipv6";
  present: boolean;
  managed: boolean;
  label: string;
  public_address: string;
  user_id: number | null;
  owner: string | null;
  site_count: number;
}

export interface ServerAddresses {
  addresses: ServerAddress[];
  interfaces: string[];
  /** Sitelerin DNS kayıtlarında varsayılan olarak kullanılan adres. */
  dns_address: string;
}

export interface SiteSummary {
  site_id: number;
  ssl_expires_at: number | null;
  git: {
    branch: string;
    running: boolean;
    last_status: "running" | "success" | "failed" | null;
    last_at: number | null;
    last_subject: string | null;
  } | null;
  databases: number;
  cron_jobs: number;
  /** Panelin bu sitede fark ettiği sorunlar (en önemlisi başta). */
  issues: { severity: "info" | "warning" | "critical"; label: string }[];
  /** Son 24 saatin erişilebilirlik yüzdesi; site yoklanmıyorsa `null`. */
  uptime_day: number | null;
  /** Son başarılı yoklamanın yanıt süresi (ms). */
  response_millis: number | null;
  /** Site klasörünün boyutu (bayt); henüz ölçülmediyse `null`. */
  disk_bytes: number | null;
  /** Bu ayki trafik (bayt). */
  month_bytes: number;
}

export interface UptimeBucket {
  at: number;
  checks: number;
  failures: number;
  millis: number;
}

export interface SiteUptime {
  uptime_day: number | null;
  uptime_week: number | null;
  average_millis: number | null;
  /** Son yoklamanın HTTP durum kodu; yanıt alınamadıysa 0. */
  last_status: number | null;
  last_checked_at: number | null;
  buckets: UptimeBucket[];
}

export interface GitConfig {
  site_id: number;
  repo_url: string;
  branch: string;
  auto_deploy: boolean;
  script: string;
  webhook_token: string;
  public_key: string;
  created_at: number;
}

export interface Deployment {
  id: number;
  site_id: number;
  source: "manual" | "webhook" | "rollback";
  commit_hash: string;
  commit_subject: string;
  commit_author: string;
  status: "running" | "success" | "failed";
  job_id: number | null;
  started_at: number;
  finished_at: number | null;
}

export interface GitView {
  config: GitConfig | null;
  webhook_path: string | null;
  deployments: Deployment[];
  running: boolean;
}

// ---- Sağlık merkezi ----

export type HealthSeverity = "info" | "warning" | "critical";

export interface HealthFinding {
  /** `kural:hedef` biçiminde kalıcı anahtar. */
  key: string;
  rule: string;
  severity: HealthSeverity;
  title: string;
  detail: string;
  link: string | null;
  /** Tek tıkla düzeltilebiliyorsa düğmenin adı. */
  fix: string | null;
  /** Panel bunu kendiliğinden düzeltmeyi deniyor. */
  auto: boolean;
  since: number;
}

export interface HealthSettings {
  restart_services: boolean;
  issue_certificates: boolean;
  check_sites: boolean;
  notify: boolean;
  security_updates: boolean;
  update_wordpress: boolean;
}

export interface HealthAction {
  id: number;
  created_at: number;
  rule: string;
  target: string;
  summary: string;
  automatic: boolean;
  success: boolean;
  detail: string;
}

export interface HealthView {
  report: { checked_at: number; checks: number; findings: HealthFinding[] };
  settings: HealthSettings;
  actions: HealthAction[];
  interval_secs: number;
}

// ---- WordPress ----

export interface WordPressExtension {
  name: string;
  title: string;
  /** `active`, `inactive`, `must-use`, `dropin`... */
  status: string;
  /** `available` ise güncelleme var. */
  update: string;
  version: string;
  update_version: string | null;
  /** `on` ise kendiliğinden güncellenir. */
  auto_update: string;
}

/** WordPress uç adresi; alt klasördeki kurulumlar `?path=` ile seçilir (kök kurulumda boş). */
export function wordpressUrl(siteId: number, suffix: string, path: string): string {
  const query = path ? `?path=${encodeURIComponent(path)}` : "";
  return `/sites/${siteId}/wordpress${suffix}${query}`;
}

/** Sitenin belge kökü altında bulunan bir WordPress kurulumu. */
export interface WordPressInstall {
  /** Belge köküne göre klasör; kök kurulumda boş. */
  path: string;
  version: string;
  /** Tahmini adres. */
  url: string;
  is_root: boolean;
}

export interface WordPressStatus {
  /** Kurulumun klasörü; kök kurulumda boş. */
  path: string;
  installed: boolean;
  can_install: boolean;
  database_ready: boolean;
  version: string;
  core_update: string | null;
  url: string;
  title: string;
  maintenance: boolean;
  auto_core: "minor" | "all" | "off";
  plugins: WordPressExtension[];
  themes: WordPressExtension[];
  problem: string | null;
}

// ---- Hesabın kendi yedekleri ----

export interface MySnapshot {
  target_id: number;
  id: string;
  short_id: string;
  time: number;
  /** `site`, `db` ya da `mail`. */
  kind: string;
  name: string;
  size: number | null;
  /** Elle alınmış yedek; hesap kendisi silebilir. */
  manual: boolean;
}

export interface MyBackups {
  /** Yedekleme kurulu ve en az bir yedek yeri tanımlı. */
  available: boolean;
  snapshots: MySnapshot[];
  sites: { id: number; name: string }[];
  databases: { id: number; name: string }[];
  mail_domains: { id: number; name: string }[];
  manual_count: number;
  manual_limit: number;
}

// ---- SFTP ve SSH erişimi ----

export interface AccountAccess {
  /** Sunucuda sshd kurulu mu? */
  available: boolean;
  mode: "none" | "sftp" | "shell";
  username: string;
  port: number;
  password_set: boolean;
  keys: { id: number; name: string; fingerprint: string; created_at: number }[];
  can_change_mode: boolean;
  can_grant_shell: boolean;
}

// ---- Site istatistikleri ----

export interface SiteDayStats {
  /** `YYYYMMDD` */
  day: number;
  requests: number;
  pages: number;
  visitors: number;
  bytes: number;
  bots: number;
  s2xx: number;
  s3xx: number;
  s4xx: number;
  s5xx: number;
}

export interface SiteStats {
  days: SiteDayStats[];
  top_paths: { path: string; hits: number }[];
  /** İçinde bulunulan ayda aktarılan veri (bayt). */
  month_bytes: number;
}

/** Sunucu yükünün geçmişindeki bir nokta: o aralığın ortalaması. */
export interface ArchivePoint {
  ts: number;
  cpu: number;
  /** Bellek kullanımı (yüzde). */
  mem: number;
  load1: number;
  net_rx: number;
  net_tx: number;
}

// ---- İşletim geçmişi: servis ölçümleri, haftalık özet, kara liste, varsayılan yedek planı, veritabanı çöp kutusu ----

export type ServiceRange = "day" | "week" | "month";

/** Servis ölçüm geçmişindeki bir nokta (aralığa göre seyreltilmiş ortalama). */
export interface ServicePoint {
  ts: number;
  /** Bellek (bayt). */
  memory: number | null;
  /** İşlemci (tek çekirdeğe göre yüzde). */
  cpu: number | null;
  tasks: number | null;
  /** Aralıkta servis hata durumunda görüldü mü? */
  failed: boolean;
  restarts: number;
}

export interface ServiceTrend {
  growth_bytes: number;
  growth_ratio: number;
  r2: number;
  span_days: number;
}

export interface ServiceSummary {
  restarts_24h: number;
  restarts_7d: number;
  trend: ServiceTrend | null;
  leak_suspect: boolean;
  latest: ServicePoint | null;
}

export interface ServiceHistory {
  unit: string;
  /** Bu servisin ölçüm geçmişi tutuluyor mu? */
  monitored: boolean;
  range: ServiceRange;
  points: ServicePoint[];
  summary: ServiceSummary | null;
}

export interface WeeklyReportSettings {
  enabled: boolean;
  /** 1 = Pazartesi … 7 = Pazar. */
  weekday: number;
  /** Gece yarısından itibaren dakika (sunucu saati). */
  at_minute: number;
  email_to: string;
}

export interface WeeklyReportView {
  settings: WeeklyReportSettings;
  next_run_at: number | null;
  last: { at: number; delivered: number; errors: string[] };
  smtp_configured: boolean;
}

export interface WeeklyReportDelivery {
  subject: string;
  text: string;
  html: string;
  emailed: string[];
  telegram: number;
  errors: string[];
}

export type BlacklistVerdict = { kind: "listed"; codes: string[] } | { kind: "clean" } | { kind: "refused" } | { kind: "unknown" };

export interface BlacklistView {
  mail_installed: boolean;
  lists: string[];
  checked_at: number;
  entries: { ip: string; list: string; label: string; verdict: BlacklistVerdict }[];
  /** Genel adres olmadığı için sorgulanmayan adresler. */
  skipped: string[];
}

export interface BackupDefaultPlan {
  enabled: boolean;
  target_id: number | null;
  frequency: "daily" | "weekly" | "monthly";
  at_minute: number;
  weekday: number;
  month_day: number;
  keep_daily: number;
  keep_weekly: number;
  keep_monthly: number;
  plan_id: number | null;
}

/** Silinirken dökümü alınmış, 7 gün boyunca geri getirilebilen veritabanı. */
export interface DatabaseTrashItem {
  id: number;
  user_id: number;
  owner: string;
  engine: string;
  name: string;
  size: number;
  deleted_at: number;
  expires_at: number;
}
