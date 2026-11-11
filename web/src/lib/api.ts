import { t } from "./i18n";
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
  const headers: Record<string, string> = { "x-pixpanel-csrf": "1" };
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
  disk_bytes: number | null;
  disk_measured_at: number | null;
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

export type SiteKind = "php" | "static" | "proxy" | "node";

export interface NodeSettings {
  version: string;
  command: string;
  env: { key: string; value: string }[];
  memory_mb: number;
}

export interface NodeStatus {
  state: string;
  sub_state: string;
  pid: number;
  memory_bytes: number | null;
  restarts: number;
  port: number;
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
}

export type DbEngine = "mariadb" | "postgresql";
export type DbAccess = "full" | "readwrite" | "readonly";

export interface DatabaseInfo {
  id: number;
  engine: DbEngine;
  name: string;
  owner: string;
  size: number | null;
  grants: { db_user_id: number; name: string; host: string; access: DbAccess }[];
  created_at: number;
}

export interface DbUserInfo {
  id: number;
  engine: DbEngine;
  name: string;
  host: string;
  owner: string;
  created_at: number;
}

export interface DatabaseOverview {
  engines: { id: DbEngine; label: string; installed: boolean; running: boolean }[];
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
  owners: { id: number; username: string }[];
  php_defaults: Record<string, PhpGlobal>;
  node_versions: string[];
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

export interface WordPressStatus {
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
