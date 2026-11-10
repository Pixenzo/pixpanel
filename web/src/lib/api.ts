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
    throw new ApiError(0, "network", "Panele ulaşılamıyor. Bağlantınızı ve sunucunun çalıştığını denetleyin.");
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
  return error instanceof Error ? error.message : "Beklenmeyen bir hata oluştu";
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
  category: "web" | "php" | "database" | "container";
  installed: boolean;
  version: string | null;
  service: string | null;
  running: boolean;
}

export type SiteKind = "php" | "static" | "proxy";

export interface PhpDirective {
  key: string;
  value: string;
}

export interface PhpSettings {
  memory_limit: string;
  upload_max_filesize: string;
  post_max_size: string;
  max_execution_time: number;
  max_input_vars: number;
  display_errors: boolean;
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
  mode: "none" | "selfsigned" | "custom" | "letsencrypt";
  force_https: boolean;
  hsts: boolean;
  certificate: CertInfo | null;
}

export interface SiteOptions {
  nginx_installed: boolean;
  php_versions: string[];
  owners: { id: number; username: string }[];
}
