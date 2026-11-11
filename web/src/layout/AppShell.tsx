// Uygulama iskeleti: koyu yan gezinme, hızlı aramalı üst çubuk ve içerik alanı.

import * as Dropdown from "@radix-ui/react-dropdown-menu";
import { useQuery } from "@tanstack/react-query";
import { ChevronDown, LogOut, Menu, Moon, Search, Sun, UserRound } from "lucide-react";
import type { ReactNode } from "react";
import { useEffect, useState } from "react";
import { NavLink, useLocation, useNavigate } from "react-router";
import { BrandLogo, BrandName } from "../components/Brand";
import { CommandPalette } from "../components/CommandPalette";
import { ErrorBoundary } from "../components/ErrorBoundary";
import { Button, IconButton, Spinner } from "../components/ui";
import type { Overview } from "../lib/api";
import { api } from "../lib/api";
import { useBrandTitle, useBranding } from "../lib/branding";
import { classNames, formatBytes, formatPercent } from "../lib/format";
import { t } from "../lib/i18n";
import type { LiveState } from "../lib/live";
import { useFallbackInterval, useJobs, useLiveState } from "../lib/live";
import { useMe, useSession, useTheme } from "../lib/session";
import type { NavItem } from "./navigation";
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
        <BrandLogo onDark />
        <span className="min-w-0 truncate text-[16px] font-semibold tracking-[-0.01em]">
          <BrandName />
        </span>
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
                      "flex h-8.5 items-center gap-2.5 rounded-md px-2.5 font-medium transition-colors outline-offset-[-2px] max-lg:h-11 max-lg:text-[15px]",
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
  const refetchInterval = useFallbackInterval(5000);
  const { data } = useQuery({
    queryKey: ["overview"],
    queryFn: () => api.get<Overview>("/system/overview"),
    refetchInterval,
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
  const { data } = useJobs();
  const running = data?.filter((job) => job.status === "running") ?? [];
  if (running.length === 0) return null;
  return (
    <button
      type="button"
      onClick={() => navigate("/jobs")}
      aria-label={t("{count} görev çalışıyor", { count: running.length })}
      className="flex h-8 max-w-64 items-center gap-2 rounded-md bg-accent-soft px-2.5 text-[13px] font-medium text-accent"
    >
      <Spinner className="size-3.5 shrink-0" />
      <span className="truncate max-sm:hidden">{running.length === 1 ? running[0].title : t("{count} görev çalışıyor", { count: running.length })}</span>
      <span className="tabular sm:hidden">{running.length}</span>
    </button>
  );
}

const liveLabels: Record<LiveState, string> = {
  live: "Canlı: değişiklikler anında yansır",
  connecting: "Sunucuya bağlanılıyor",
  offline: "Bağlantı koptu, yeniden deneniyor",
};

/** Canlı bağlantının durumu; koptuğunda kullanıcı ekrandaki verinin eskiyebileceğini bilir. */
function LiveIndicator() {
  const state = useLiveState();
  const label = t(liveLabels[state]);
  return (
    <span
      role="status"
      title={label}
      className={classNames(
        "flex h-8 items-center gap-1.5 rounded-md px-2 text-[13px] font-medium",
        state === "offline" ? "bg-warn-soft text-warn" : "text-ink-3",
      )}
    >
      <span className="relative flex size-2">
        {state === "live" && <span aria-hidden className="absolute inset-0 animate-[live-ping_2.4s_ease-out_infinite] rounded-full bg-ok" />}
        <span
          aria-hidden
          className={classNames("relative size-2 rounded-full", state === "live" ? "bg-ok" : state === "offline" ? "bg-warn" : "bg-idle")}
        />
      </span>
      <span className={state === "offline" ? "max-sm:sr-only" : "sr-only"}>{state === "offline" ? t("Bağlantı koptu") : label}</span>
    </span>
  );
}

/** Telefonda alt çubukta duran sayfalar (görünür olan ilk dördü); geri kalanı "Menü"den açılır. */
const QUICK_ROUTES = ["/overview", "/sites", "/server", "/databases", "/mail", "/jobs"];
/** Alt çubuğa sığması için kısaltılan adlar. */
const QUICK_LABELS: Record<string, string> = { "/overview": "Durum", "/server": "Sunucu", "/databases": "Veritabanı" };

function BottomBar({ onOpenMenu, menuOpen }: { onOpenMenu: () => void; menuOpen: boolean }) {
  const me = useMe();
  const all = navigation.flatMap((group) => group.items).filter((item) => item.visible(me));
  const quick = QUICK_ROUTES.map((route) => all.find((item) => item.to === route))
    .filter((item): item is NavItem => Boolean(item))
    .slice(0, 4);
  const cell = "flex min-w-0 flex-1 flex-col items-center justify-center gap-0.5 text-[11.5px] font-medium";
  return (
    <nav
      aria-label={t("Hızlı gezinme")}
      className="fixed inset-x-0 bottom-0 z-30 flex h-[calc(3.75rem+env(safe-area-inset-bottom))] border-t border-line bg-surface/95 pb-[env(safe-area-inset-bottom)] backdrop-blur lg:hidden"
    >
      {quick.map((item) => (
        <NavLink key={item.to} to={item.to} className={({ isActive }) => classNames(cell, isActive ? "text-accent" : "text-ink-2")}>
          <item.icon aria-hidden className="size-5" />
          <span className="max-w-full truncate px-1">{t(QUICK_LABELS[item.to] ?? item.label)}</span>
        </NavLink>
      ))}
      <button type="button" aria-expanded={menuOpen} onClick={onOpenMenu} className={classNames(cell, menuOpen ? "text-accent" : "text-ink-2")}>
        <Menu aria-hidden className="size-5" />
        <span>{t("Menü")}</span>
      </button>
    </nav>
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

/** Başka hesap adına oturumdayken her sayfanın üstünde durur: kimin gözünden bakıldığını
 *  unutturmaz ve tek tıkla kendi hesabına döndürür. */
function ImpersonationBar({ by, username }: { by: string; username: string }) {
  const [leaving, setLeaving] = useState(false);
  const [failed, setFailed] = useState(false);
  async function leave() {
    setLeaving(true);
    setFailed(false);
    try {
      await api.post("/auth/impersonate/stop");
      window.location.assign("/users");
    } catch {
      setLeaving(false);
      setFailed(true);
    }
  }
  return (
    <div role="status" className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1.5 border-b border-warn/40 bg-warn-soft px-3 py-2 text-[13px] text-warn">
      <span>
        {t("{by} olarak {username} hesabının panelindesiniz. Yaptığınız her işlem bu hesaba uygulanır.", { by, username })}
        {failed && ` ${t("Geri dönülemedi; yeniden deneyin.")}`}
      </span>
      <Button size="sm" onClick={() => void leave()} loading={leaving}>
        {t("Kendi hesabıma dön")}
      </Button>
    </div>
  );
}

export function AppShell({ children }: { children: ReactNode }) {
  const me = useMe();
  const [theme, toggleTheme] = useTheme();
  useBrandTitle(useBranding());
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
    <div className="min-h-dvh max-lg:pb-[calc(3.75rem+env(safe-area-inset-bottom))] lg:pl-56">
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
          <aside className="absolute inset-y-0 left-0 w-72 max-w-[85vw] animate-[drawer-in_180ms_ease-out] shadow-pop">
            <Sidebar onNavigate={() => setDrawerOpen(false)} />
          </aside>
        </div>
      )}

      <header className="sticky top-0 z-20 flex h-13 items-center gap-2 border-b border-line bg-surface/95 px-3 backdrop-blur sm:gap-3 sm:px-4 lg:px-6">
        <span className="flex shrink-0 items-center lg:hidden">
          <BrandLogo />
        </span>
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
          <LiveIndicator />
          <IconButton label={theme === "dark" ? t("Açık temaya geç") : t("Koyu temaya geç")} onClick={toggleTheme}>
            {theme === "dark" ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </IconButton>
          <UserMenu />
        </div>
      </header>

      {me.impersonated_by !== null && <ImpersonationBar by={me.impersonated_by} username={me.username} />}

      <main id="content" className="mx-auto w-full max-w-[1400px] px-3 py-4 sm:px-4 sm:py-6 lg:px-6">
        <ErrorBoundary resetKey={location.pathname}>{children}</ErrorBoundary>
      </main>

      <BottomBar menuOpen={drawerOpen} onOpenMenu={() => setDrawerOpen((open) => !open)} />
      <CommandPalette open={paletteOpen} onClose={() => setPaletteOpen(false)} />
    </div>
  );
}
