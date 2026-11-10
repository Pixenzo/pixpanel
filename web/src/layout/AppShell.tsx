// Uygulama iskeleti: koyu yan gezinme, hızlı aramalı üst çubuk ve içerik alanı.

import * as Dropdown from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, LogOut, Menu, Moon, Search, Sun, UserRound, X } from "lucide-react";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { NavLink, useLocation, useNavigate } from "react-router";
import { BrandMark } from "../components/Brand";
import { CommandPalette } from "../components/CommandPalette";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { IconButton, Spinner } from "../components/ui";
import type { JobInfo, Overview } from "../lib/api";
import { api } from "../lib/api";
import { classNames, formatBytes, formatPercent } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe, useSession, useTheme } from "../lib/session";
import { navigation } from "./navigation";

const roleLabels: Record<string, string> = {
  admin: "Yönetici",
  reseller: "Bayi",
  user: "Müşteri",
  member: "Alt hesap",
};

function Sidebar({ onNavigate }: { onNavigate: () => void }) {
  const me = useMe();
  return (
    <nav aria-label={t("Ana gezinme")} className="flex h-full flex-col overflow-y-auto bg-rail text-rail-ink">
      <div className="flex h-13 shrink-0 items-center gap-2.5 border-b border-rail-line px-4">
        <BrandMark onDark />
        <span className="text-[16px] font-semibold tracking-[-0.01em]">PixPanel</span>
      </div>
      <div className="flex flex-1 flex-col gap-5 px-2.5 py-4">
        {navigation.map((group, index) => {
          const items = group.items.filter((item) => item.visible(me));
          if (items.length === 0) return null;
          return (
            <div key={group.title ?? index} className="flex flex-col gap-px">
              {group.title && <p className="px-2.5 pb-1.5 text-[12px] font-medium text-rail-ink-2">{t(group.title)}</p>}
              {items.map((item) => (
                <NavLink
                  key={item.to}
                  to={item.to}
                  onClick={onNavigate}
                  className={({ isActive }) =>
                    classNames(
                      "flex h-8.5 items-center gap-2.5 rounded-md px-2.5 font-medium transition-colors outline-offset-[-2px]",
                      isActive ? "bg-rail-raised text-white" : "text-rail-ink-2 hover:bg-rail-raised/60 hover:text-rail-ink",
                    )
                  }
                >
                  {({ isActive }) => (
                    <>
                      <item.icon className={classNames("size-[16px] shrink-0", isActive && "text-rail-accent")} />
                      {t(item.label)}
                    </>
                  )}
                </NavLink>
              ))}
            </div>
          );
        })}
      </div>
      <p className="shrink-0 border-t border-rail-line px-5 py-3 text-[12px] text-rail-ink-2">Pixenzo</p>
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
    <dl className="tabular hidden items-center gap-4 text-[13px] xl:flex">
      <div className={cell}>
        <dt className="sr-only">{t("Sunucu")}</dt>
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
      className="flex h-8 max-w-64 items-center gap-2 rounded-md bg-accent-soft px-2.5 text-[13px] font-medium text-accent"
    >
      <Spinner className="size-3.5 shrink-0" />
      <span className="truncate">{running.length === 1 ? running[0].title : t("{count} görev çalışıyor", { count: running.length })}</span>
    </button>
  );
}

function UserMenu() {
  const me = useMe();
  const { logout } = useSession();
  const navigate = useNavigate();
  const itemClass = "flex h-8 items-center gap-2 rounded px-2 outline-none select-none data-[highlighted]:bg-sunken";
  return (
    <Dropdown.Root>
      <Dropdown.Trigger className="flex h-8 items-center gap-1.5 rounded-md px-2 font-medium hover:bg-sunken">
        <UserRound className="size-4 text-ink-2" />
        <span className="hidden sm:inline">{me.username}</span>
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

const isMac = /Mac|iPhone|iPad/.test(navigator.platform);

export function AppShell({ children }: { children: ReactNode }) {
  const me = useMe();
  const [theme, toggleTheme] = useTheme();
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const location = useLocation();

  // Sayfa değişince mobil çekmece kapanır.
  useEffect(() => setDrawerOpen(false), [location.pathname]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault();
        setPaletteOpen((open) => !open);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <div className="min-h-dvh lg:pl-56">
      <a
        href="#content"
        className="sr-only focus:not-sr-only focus:fixed focus:top-2 focus:left-2 focus:z-[70] focus:rounded-md focus:bg-surface focus:px-3 focus:py-2"
      >
        {t("İçeriğe geç")}
      </a>

      <aside className="fixed inset-y-0 left-0 z-30 hidden w-56 lg:block">
        <Sidebar onNavigate={() => {}} />
      </aside>

      {drawerOpen && (
        <div className="fixed inset-0 z-40 lg:hidden">
          <button type="button" aria-label={t("Menüyü kapat")} className="absolute inset-0 bg-[rgb(10_16_22/0.5)]" onClick={() => setDrawerOpen(false)} />
          <aside className="absolute inset-y-0 left-0 w-64 shadow-pop">
            <Sidebar onNavigate={() => setDrawerOpen(false)} />
          </aside>
        </div>
      )}

      <header className="sticky top-0 z-20 flex h-13 items-center gap-3 border-b border-line bg-surface px-4 lg:px-6">
        <IconButton label={drawerOpen ? t("Menüyü kapat") : t("Menüyü aç")} className="lg:hidden" onClick={() => setDrawerOpen((open) => !open)}>
          {drawerOpen ? <X className="size-4.5" /> : <Menu className="size-4.5" />}
        </IconButton>
        <button
          type="button"
          onClick={() => setPaletteOpen(true)}
          className="flex h-8 min-w-0 flex-1 items-center gap-2 rounded-md border border-line bg-bg px-2.5 text-ink-3 transition-colors hover:border-line-strong sm:max-w-80"
        >
          <Search aria-hidden className="size-4 shrink-0" />
          <span className="truncate">{t("Site, araç ya da sayfa ara")}</span>
          <kbd className="ml-auto hidden shrink-0 rounded border border-line bg-surface px-1.5 font-mono text-[11.5px] text-ink-2 sm:inline">
            {isMac ? "⌘K" : "Ctrl K"}
          </kbd>
        </button>
        <div className="ml-auto flex items-center gap-1.5">
          {me.role === "admin" && <LiveReadout />}
          <RunningJobs />
          <IconButton label={theme === "dark" ? t("Açık temaya geç") : t("Koyu temaya geç")} onClick={toggleTheme}>
            {theme === "dark" ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </IconButton>
          <UserMenu />
        </div>
      </header>

      <main id="content" className="mx-auto w-full max-w-[1400px] px-4 py-6 lg:px-6">
        <ErrorBoundary resetKey={location.pathname}>{children}</ErrorBoundary>
      </main>

      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} />
    </div>
  );
}
