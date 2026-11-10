// Güvenlik duvarı ile saldırı engelleme sayfalarını tek başlık altında toplayan sekme şeridi.

import { NavLink } from "react-router";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

const SECURITY_PAGES = [
  { to: "/firewall", label: "Güvenlik duvarı" },
  { to: "/fail2ban", label: "Saldırı engelleme" },
] as const;

export function SecurityTabs() {
  return (
    <nav aria-label={t("Güvenlik")} className="no-scrollbar mb-4 flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line">
      {SECURITY_PAGES.map((page) => (
        <NavLink
          key={page.to}
          to={page.to}
          className={({ isActive }) =>
            classNames(
              "-mb-px flex h-9 items-center border-b-2 px-3 font-medium whitespace-nowrap transition-colors",
              isActive ? "border-accent text-accent" : "border-transparent text-ink-2 hover:text-ink",
            )
          }
        >
          {t(page.label)}
        </NavLink>
      ))}
    </nav>
  );
}
