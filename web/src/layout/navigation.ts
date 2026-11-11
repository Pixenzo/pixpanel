// Yan gezinme ve hızlı aramanın paylaştığı sayfa listesi.

import type { LucideIcon } from "lucide-react";
import {
  Activity,
  Archive,
  Boxes,
  CalendarClock,
  Cog,
  Container,
  Database,
  DatabaseZap,
  FileCode2,
  FileText,
  FolderOpen,
  Gauge,
  HeartPulse,
  Globe,
  ListChecks,
  Mail,
  Network,
  ScrollText,
  Settings,
  ShieldBan,
  ShieldCheck,
  SquareTerminal,
  Users,
} from "lucide-react";
import type { Me } from "../lib/api";

export interface NavItem {
  to: string;
  label: string;
  icon: LucideIcon;
  visible: (me: Me) => boolean;
}

export interface NavGroup {
  title: string | null;
  items: NavItem[];
}

const isAdmin = (me: Me) => me.role === "admin";
/** Alt hesaplar yalnızca kendilerine verilen alanları görür. */
const can = (area: string) => (me: Me) => me.role !== "member" || me.permissions.includes(area);

export const navigation: NavGroup[] = [
  {
    title: null,
    items: [
      { to: "/overview", label: "Genel bakış", icon: HeartPulse, visible: isAdmin },
      { to: "/sites", label: "Siteler", icon: Globe, visible: can("sites") },
      { to: "/mail", label: "Posta", icon: Mail, visible: can("mail") },
      { to: "/dns", label: "DNS", icon: Network, visible: can("dns") },
      { to: "/databases", label: "Veritabanları", icon: Database, visible: can("databases") },
      { to: "/files", label: "Dosyalar", icon: FolderOpen, visible: can("files") },
      { to: "/cron", label: "Zamanlı görevler", icon: CalendarClock, visible: can("cron") },
      { to: "/logs", label: "Günlükler", icon: FileText, visible: can("logs") },
      { to: "/backups", label: "Yedekler", icon: Archive, visible: (me) => !isAdmin(me) && can("backups")(me) },
    ],
  },
  {
    title: "Sunucu",
    items: [
      { to: "/server", label: "Sunucu durumu", icon: Gauge, visible: isAdmin },
      { to: "/firewall", label: "Güvenlik duvarı", icon: ShieldCheck, visible: isAdmin },
      { to: "/fail2ban", label: "Saldırı engelleme", icon: ShieldBan, visible: isAdmin },
      { to: "/docker", label: "Docker", icon: Container, visible: isAdmin },
      { to: "/php", label: "PHP", icon: FileCode2, visible: isAdmin },
      { to: "/redis", label: "Redis", icon: DatabaseZap, visible: isAdmin },
      { to: "/backups", label: "Yedekler", icon: Archive, visible: isAdmin },
      { to: "/components", label: "Bileşenler", icon: Boxes, visible: isAdmin },
      { to: "/services", label: "Servisler", icon: Cog, visible: isAdmin },
      { to: "/processes", label: "Süreçler", icon: Activity, visible: isAdmin },
      { to: "/terminal", label: "Terminal", icon: SquareTerminal, visible: isAdmin },
    ],
  },
  {
    title: "Yönetim",
    items: [
      { to: "/users", label: "Kullanıcılar", icon: Users, visible: (me) => me.role !== "member" },
      { to: "/jobs", label: "Görevler", icon: ListChecks, visible: () => true },
      { to: "/audit", label: "Denetim günlüğü", icon: ScrollText, visible: isAdmin },
      { to: "/settings", label: "Ayarlar", icon: Settings, visible: isAdmin },
    ],
  },
];
