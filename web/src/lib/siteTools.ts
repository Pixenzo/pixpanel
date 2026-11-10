// Bir sitenin araçları: site kartındaki ızgara, site çalışma alanı ve hızlı
// arama aynı listeyi kullanır. Her araç, o anki durumunu tek satırda özetler.

import type { LucideIcon } from "lucide-react";
import {
  ArrowRightLeft,
  CalendarClock,
  Database,
  FileCode2,
  FileText,
  FolderOpen,
  GitBranch,
  Globe,
  ShieldCheck,
  SlidersHorizontal,
  SquareTerminal,
} from "lucide-react";
import type { Tone } from "../components/ui";
import type { Me, Site, SiteOptions, SiteSummary } from "./api";
import { formatRelative } from "./format";
import { t } from "./i18n";

export interface SiteTool {
  id: string;
  label: string;
  icon: LucideIcon;
  to: string;
  status: string;
  tone?: Tone;
}

const sslModeLabels: Record<string, string> = {
  letsencrypt: "Let's Encrypt",
  custom: "Yüklenen sertifika",
  selfsigned: "Kendinden imzalı",
};

function sslStatus(site: Site, summary?: SiteSummary): Pick<SiteTool, "status" | "tone"> {
  if (site.ssl_mode === "none") return { status: t("Sertifika yok"), tone: "warn" };
  const label = t(sslModeLabels[site.ssl_mode] ?? site.ssl_mode);
  if (!summary?.ssl_expires_at) return { status: label };
  const days = Math.floor((summary.ssl_expires_at - Date.now() / 1000) / 86400);
  if (days < 0) return { status: t("{label} · süresi doldu", { label }), tone: "bad" };
  return { status: t("{label} · {days} gün kaldı", { label, days }), tone: days < 15 ? "warn" : undefined };
}

function gitStatus(summary?: SiteSummary): Pick<SiteTool, "status" | "tone"> {
  const git = summary?.git;
  if (!git) return { status: t("Bağlı değil") };
  if (git.running) return { status: t("{branch} · yayınlanıyor", { branch: git.branch }), tone: "accent" };
  if (git.last_status === "failed") return { status: t("{branch} · son yayın başarısız", { branch: git.branch }), tone: "bad" };
  if (git.last_status === "success") return { status: t("{branch} · {when} yayınlandı", { branch: git.branch, when: t(formatRelative(git.last_at)) }) };
  return { status: t("{branch} · henüz yayınlanmadı", { branch: git.branch }) };
}

export function siteTools(site: Site, me: Me, summary?: SiteSummary, options?: SiteOptions): SiteTool[] {
  const can = (area: string) => me.role !== "member" || me.permissions.includes(area);
  const base = `/sites/${site.id}`;
  const tools: SiteTool[] = [];

  if (can("files") && site.kind !== "proxy") {
    tools.push({
      id: "files",
      label: t("Dosyalar"),
      icon: FolderOpen,
      to: `/files?alan=${encodeURIComponent(`user:${site.user_id}`)}&yol=${encodeURIComponent(`/sites/${site.domain}`)}`,
      status: `sites/${site.domain}`,
    });
  }
  if (can("databases")) {
    tools.push({
      id: "databases",
      label: t("Veritabanları"),
      icon: Database,
      to: "/databases",
      status: summary ? (summary.databases === 0 ? t("Henüz yok") : t("{count} veritabanı", { count: summary.databases })) : "",
    });
  }
  if (site.kind === "php") {
    const memory = site.php_settings.memory_limit ?? options?.php_defaults[site.php_version ?? ""]?.memory_limit;
    tools.push({
      id: "php",
      label: t("PHP ayarları"),
      icon: FileCode2,
      to: `${base}/php`,
      status: memory ? t("Sürüm {version} · {memory} bellek", { version: site.php_version ?? "", memory }) : t("Sürüm {version}", { version: site.php_version ?? "" }),
    });
  }
  if (site.kind === "proxy") {
    tools.push({ id: "proxy", label: t("Ters vekil"), icon: ArrowRightLeft, to: `${base}/proxy`, status: site.proxy_target ?? "" });
  }
  if (can("ssl")) {
    tools.push({ id: "ssl", label: t("SSL sertifikası"), icon: ShieldCheck, to: `${base}/ssl`, ...sslStatus(site, summary) });
  }
  tools.push({ id: "git", label: t("Git ile yayınlama"), icon: GitBranch, to: `${base}/git`, ...gitStatus(summary) });
  if (can("logs")) {
    tools.push({
      id: "logs",
      label: t("Günlükler"),
      icon: FileText,
      to: `/logs?kaynak=${encodeURIComponent(`site:${site.id}:access`)}`,
      status: t("Erişim ve hata kayıtları"),
    });
  }
  if (can("cron")) {
    tools.push({
      id: "cron",
      label: t("Zamanlı görevler"),
      icon: CalendarClock,
      to: "/cron",
      status: summary ? (summary.cron_jobs === 0 ? t("Henüz yok") : t("{count} görev", { count: summary.cron_jobs })) : "",
    });
  }
  tools.push({
    id: "settings",
    label: t("Adresler ve yayın"),
    icon: Globe,
    to: `${base}/settings`,
    status: site.aliases.length === 0 ? t("Takma ad yok") : t("{count} takma ad", { count: site.aliases.length }),
  });
  if (me.role === "admin") {
    tools.push({
      id: "advanced",
      label: t("nginx yönergeleri"),
      icon: SlidersHorizontal,
      to: `${base}/advanced`,
      status: site.nginx_custom.trim() ? t("Özel yönerge tanımlı") : t("Varsayılan"),
    });
    tools.push({
      id: "terminal",
      label: t("Terminal"),
      icon: SquareTerminal,
      to: `/terminal?kapsam=${encodeURIComponent(`user:${site.user_id}`)}`,
      status: t("{owner} hesabının kabuğu", { owner: site.owner }),
    });
  }
  return tools;
}
