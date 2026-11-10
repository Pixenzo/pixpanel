import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router";
import { FeedbackProvider } from "./components/feedback";
import { LoadingBlock } from "./components/ui";
import { AppShell } from "./layout/AppShell";
import { ApiError } from "./lib/api";
import { SessionProvider, useSession } from "./lib/session";
import { AccountPage } from "./pages/Account";
import { AuditPage } from "./pages/Audit";
import { ComponentsPage } from "./pages/Components";
import { CronPage } from "./pages/Cron";
import { DashboardPage } from "./pages/Dashboard";
import { DnsPage } from "./pages/Dns";
import { DnsZonePage } from "./pages/DnsZone";
import { DockerPage } from "./pages/Docker";
import { DatabasesPage } from "./pages/Databases";
import { Fail2banPage } from "./pages/Fail2ban";
import { FilesPage } from "./pages/Files";
import { FirewallPage } from "./pages/Firewall";
import { JobsPage } from "./pages/Jobs";
import { LoginPage } from "./pages/Login";
import { LogsPage } from "./pages/Logs";
import { MailPage } from "./pages/Mail";
import { MailDomainPage } from "./pages/MailDomain";
import { PhpPage } from "./pages/Php";
import { ProcessesPage } from "./pages/Processes";
import { RedisPage } from "./pages/Redis";
import { BackupsPage } from "./pages/Backups";
import { ServicesPage } from "./pages/Services";
import { SettingsPage } from "./pages/Settings";
import { SiteDetailPage } from "./pages/SiteDetail";
import { SitesPage } from "./pages/Sites";
import { TerminalPage } from "./pages/Terminal";
import { UsersPage } from "./pages/Users";
import "./styles.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      // Yetki ve "bulunamadı" hataları yeniden denemekle düzelmez.
      retry: (failures, error) => !(error instanceof ApiError && error.status >= 400 && error.status < 500) && failures < 2,
    },
  },
});

function App() {
  const { me } = useSession();
  if (me === undefined) return <LoadingBlock />;
  if (me === null) return <LoginPage />;

  const isAdmin = me.role === "admin";
  const can = (area: string) => me.role !== "member" || me.permissions.includes(area);
  const canSites = can("sites");
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={<Navigate to={canSites ? "/sites" : "/account"} replace />} />
        {isAdmin && (
          <>
            <Route path="/server" element={<DashboardPage />} />
            <Route path="/firewall" element={<FirewallPage />} />
            <Route path="/fail2ban" element={<Fail2banPage />} />
            <Route path="/docker" element={<DockerPage />} />
            <Route path="/php" element={<PhpPage />} />
            <Route path="/redis" element={<RedisPage />} />
            <Route path="/backups" element={<BackupsPage />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="/components" element={<ComponentsPage />} />
            <Route path="/services" element={<ServicesPage />} />
            <Route path="/processes" element={<ProcessesPage />} />
            <Route path="/terminal" element={<TerminalPage />} />
            <Route path="/audit" element={<AuditPage />} />
          </>
        )}
        {me.role !== "member" && <Route path="/users" element={<UsersPage />} />}
        {canSites && <Route path="/sites" element={<SitesPage />} />}
        {canSites && <Route path="/sites/:id/:section?" element={<SiteDetailPage />} />}
        {can("mail") && <Route path="/mail" element={<MailPage />} />}
        {can("mail") && <Route path="/mail/:id" element={<MailDomainPage />} />}
        {can("dns") && <Route path="/dns" element={<DnsPage />} />}
        {can("dns") && <Route path="/dns/:id" element={<DnsZonePage />} />}
        {can("databases") && <Route path="/databases" element={<DatabasesPage />} />}
        {can("files") && <Route path="/files" element={<FilesPage />} />}
        {can("cron") && <Route path="/cron" element={<CronPage />} />}
        {can("logs") && <Route path="/logs" element={<LogsPage />} />}
        <Route path="/jobs" element={<JobsPage />} />
        <Route path="/account" element={<AccountPage />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </AppShell>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <BrowserRouter>
        <FeedbackProvider>
          <SessionProvider>
            <App />
          </SessionProvider>
        </FeedbackProvider>
      </BrowserRouter>
    </QueryClientProvider>
  </StrictMode>,
);
