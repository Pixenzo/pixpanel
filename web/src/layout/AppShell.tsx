// Uygulama iskeleti: yan gezinme, üst çubuk ve içerik alanı.

import * as Dropdown from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import {
  Activity,
  Boxes,
  ChevronDown,
  Cog,
  Database,
  FolderOpen,
  Globe,
  LayoutDashboard,
  ListChecks,
  LogOut,
  Menu,
  Moon,
  ScrollText,
  Sun,
  UserRound,
  Users,
  X,
} from "lucide-react";
import type { ComponentType, ReactNode } from "react";
import { useEffect, useState } from "react";
import { NavLink, useLocation, useNavigate } from "react-router";
import { IconButton, Spinner } from "../components/ui";
import type { JobInfo, Me, Overview } from "../lib/api";
import { api } from "../lib/api";
import { classNames, formatBytes, formatPercent } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe, useSession, useTheme } from "../lib/session";

interface NavItem {
  to: string;
  label: string;
  icon: ComponentType<{ className?: string }>;
  visible: (me: Me) => boolean;
}

interface NavGroup {
  title: string | null;
  items: NavItem[];
}

const isAdmin = (me: Me) => me.role === "admin";
/** Alt hesaplar yalnızca kendilerine verilen alanları görür. */
const can = (area: string) => (me: Me) => me.role !== "member" || me.permissions.includes(area);

const navigation: NavGroup[] = [
  {
    title: null,
    items: [{ to: "/", label: "Genel bakış", icon: LayoutDashboard, visible: isAdmin }],
  },
  {
    title: "Hosting",
    items: [
      { to: "/sites", label: "Siteler", icon: Globe, visible: can("sites") },
      { to: "/databases", label: "Veritabanları", icon: Database, visible: can("databases") },
      { to: "/files", label: "Dosyalar", icon: FolderOpen, visible: can("files") },
    ],
  },
  {
    title: "Sunucu",
    items: [
      { to: "/components", label: "Bileşenler", icon: Boxes, visible: isAdmin },
      { to: "/services", label: "Servisler", icon: Cog, visible: isAdmin },
      { to: "/processes", label: "Süreçler", icon: Activity, visible: isAdmin },
      { to: "/jobs", label: "Görevler", icon: ListChecks, visible: () => true },
    ],
  },
  {
    title: "Yönetim",
    items: [
      { to: "/users", label: "Kullanıcılar", icon: Users, visible: (me) => me.role !== "member" },
      { to: "/audit", label: "Denetim günlüğü", icon: ScrollText, visible: isAdmin },
    ],
  },
];

function Logo() {
  return (
    <span className="flex items-center gap-2.5">
      <svg viewBox="0 0 32 32" className="size-6" aria-hidden>
        <rect x="3" y="9" width="26" height="14" rx="7" className="fill-accent" />
        <rect x="16" y="9" width="13" height="14" rx="7" className="fill-ink" />
        <circle cx="10" cy="16" r="2.2" className="fill-surface" />
      </svg>
      <span className="text-[17px] font-semibold tracking-[-0.01em]">Capsule</span>
    </span>
  );
}

function Sidebar({ onNavigate }: { onNavigate: () => void }) {
  const me = useMe();
  return (
    <nav aria-label={t("Ana gezinme")} className="flex h-full flex-col gap-5 overflow-y-auto px-3 py-4">
      <div className="px-2">
        <Logo />
      </div>
      {navigation.map((group, index) => {
        const items = group.items.filter((item) => item.visible(me));
        if (items.length === 0) return null;
        return (
          <div key={group.title ?? index} className="flex flex-col gap-0.5">
            {group.title && <p className="px-2 pb-1 text-[12.5px] font-medium text-ink-3">{t(group.title)}</p>}
            {items.map((item) => (
              <NavLink
                key={item.to}
                to={item.to}
                end={item.to === "/"}
                onClick={onNavigate}
                className={({ isActive }) =>
                  classNames(
                    "relative flex h-9 items-center gap-2.5 rounded-md px-2 font-medium transition-colors",
                    isActive
                      ? "bg-accent-soft text-accent before:absolute before:top-2 before:bottom-2 before:-left-3 before:w-[3px] before:rounded-r before:bg-accent"
                      : "text-ink-2 hover:bg-sunken hover:text-ink",
                  )
                }
              >
                <item.icon className="size-[17px] shrink-0" />
                {t(item.label)}
              </NavLink>
            ))}
          </div>
        );
      })}
    </nav>
  );
}

/** Üst çubuktaki canlı yük göstergesi; yalnızca yöneticiler için. */
function LiveReadout() {
  const { data } = useQuery({
    queryKey: ["overview"],
    queryFn: () => api.get<Overview>("/system/overview"),
    refetchInterval: 5000,
  });
  if (!data) return null;
  const { current, host } = data;
  const memory = current.mem_total > 0 ? (current.mem_used / current.mem_total) * 100 : 0;
  const cell = "flex items-baseline gap-1.5";
  return (
    <dl className="tabular hidden items-center gap-5 text-[13px] md:flex">
      <div className={cell}>
        <dt className="text-ink-3">{t("Sunucu")}</dt>
        <dd className="font-mono font-medium">{host.hostname}</dd>
      </div>
      <div className={cell}>
        <dt className="text-ink-3">{t("İşlemci")}</dt>
        <dd className="w-9 font-mono font-medium">{formatPercent(current.cpu)}</dd>
      </div>
      <div className={cell}>
        <dt className="text-ink-3">{t("Bellek")}</dt>
        <dd className="font-mono font-medium" title={`${formatBytes(current.mem_used)} / ${formatBytes(current.mem_total)}`}>
          {formatPercent(memory)}
        </dd>
      </div>
      <div className={cell}>
        <dt className="text-ink-3">{t("Yük")}</dt>
        <dd className="font-mono font-medium">{current.load1.toFixed(2)}</dd>
      </div>
    </dl>
  );
}

/** Çalışan görev varsa üst çubukta gösterilir. */
function RunningJobs() {
  const navigate = useNavigate();
  const { data } = useQuery({
    queryKey: ["jobs"],
    queryFn: () => api.get<JobInfo[]>("/jobs"),
    refetchInterval: 4000,
  });
  const running = data?.filter((job) => job.status === "running") ?? [];
  if (running.length === 0) return null;
  return (
    <button
      type="button"
      onClick={() => navigate("/jobs")}
      className="flex h-8 items-center gap-2 rounded-md bg-accent-soft px-2.5 text-[13px] font-medium text-accent"
    >
      <Spinner className="size-3.5" />
      {running.length === 1 ? running[0].title : t("{count} görev çalışıyor", { count: running.length })}
    </button>
  );
}

const roleLabels: Record<string, string> = {
  admin: "Yönetici",
  reseller: "Bayi",
  user: "Müşteri",
  member: "Alt hesap",
};

function UserMenu() {
  const me = useMe();
  const { logout } = useSession();
  const navigate = useNavigate();
  const itemClass =
    "flex h-8 items-center gap-2 rounded px-2 outline-none select-none data-[highlighted]:bg-sunken";
  return (
    <Dropdown.Root>
      <Dropdown.Trigger className="flex h-8 items-center gap-1.5 rounded-md px-2 font-medium hover:bg-sunken">
        <UserRound className="size-4 text-ink-2" />
        {me.username}
        <ChevronDown className="size-3.5 text-ink-3" />
      </Dropdown.Trigger>
      <Dropdown.Portal>
        <Dropdown.Content
          align="end"
          sideOffset={6}
          className="z-50 min-w-52 animate-[pop-in_120ms_ease-out] rounded-lg border border-line bg-surface p-1 shadow-pop"
        >
          <div className="px-2 py-1.5">
            <p className="font-medium">{me.username}</p>
            <p className="text-[12.5px] text-ink-3">{t(roleLabels[me.role])}</p>
          </div>
          <Dropdown.Separator className="my-1 h-px bg-line" />
          <Dropdown.Item className={itemClass} onSelect={() => navigate("/account")}>
            <UserRound className="size-4 text-ink-2" /> {t("Hesabım")}
          </Dropdown.Item>
          <Dropdown.Item className={itemClass} onSelect={() => void logout()}>
            <LogOut className="size-4 text-ink-2" /> {t("Oturumu kapat")}
          </Dropdown.Item>
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  const me = useMe();
  const [theme, toggleTheme] = useTheme();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const location = useLocation();

  // Sayfa değişince mobil çekmece kapanır.
  useEffect(() => setDrawerOpen(false), [location.pathname]);

  return (
    <div className="min-h-dvh lg:pl-56">
      <a
        href="#content"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-[70] focus:rounded-md focus:bg-surface focus:px-3 focus:py-2"
      >
        {t("İçeriğe geç")}
      </a>

      <aside className="fixed inset-y-0 left-0 z-30 hidden w-56 border-r border-line bg-surface lg:block">
        <Sidebar onNavigate={() => {}} />
      </aside>

      {drawerOpen && (
        <div className="fixed inset-0 z-40 lg:hidden">
          <button
            type="button"
            aria-label={t("Menüyü kapat")}
            className="absolute inset-0 bg-[rgb(10_16_22/0.5)]"
            onClick={() => setDrawerOpen(false)}
          />
          <aside className="absolute inset-y-0 left-0 w-64 border-r border-line bg-surface shadow-pop">
            <Sidebar onNavigate={() => setDrawerOpen(false)} />
          </aside>
        </div>
      )}

      <header className="sticky top-0 z-20 flex h-13 items-center gap-3 border-b border-line bg-surface/95 px-4 backdrop-blur lg:px-6">
        <IconButton label={drawerOpen ? t("Menüyü kapat") : t("Menüyü aç")} className="lg:hidden" onClick={() => setDrawerOpen((open) => !open)}>
          {drawerOpen ? <X className="size-4.5" /> : <Menu className="size-4.5" />}
        </IconButton>
        {me.role === "admin" && <LiveReadout />}
        <div className="ml-auto flex items-center gap-1.5">
          <RunningJobs />
          <IconButton label={theme === "dark" ? t("Açık temaya geç") : t("Koyu temaya geç")} onClick={toggleTheme}>
            {theme === "dark" ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </IconButton>
          <UserMenu />
        </div>
      </header>

      <main id="content" className="mx-auto w-full max-w-[1320px] px-4 py-6 lg:px-6">
        {children}
      </main>
    </div>
  );
}
