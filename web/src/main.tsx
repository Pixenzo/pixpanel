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
import { DashboardPage } from "./pages/Dashboard";
import { DatabasesPage } from "./pages/Databases";
import { JobsPage } from "./pages/Jobs";
import { LoginPage } from "./pages/Login";
import { ProcessesPage } from "./pages/Processes";
import { ServicesPage } from "./pages/Services";
import { SiteDetailPage } from "./pages/SiteDetail";
import { SitesPage } from "./pages/Sites";
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
        <Route path="/" element={isAdmin ? <DashboardPage /> : <Navigate to={canSites ? "/sites" : "/account"} replace />} />
        {isAdmin && (
          <>
            <Route path="/components" element={<ComponentsPage />} />
            <Route path="/services" element={<ServicesPage />} />
            <Route path="/processes" element={<ProcessesPage />} />
            <Route path="/audit" element={<AuditPage />} />
          </>
        )}
        {me.role !== "member" && <Route path="/users" element={<UsersPage />} />}
        {canSites && <Route path="/sites" element={<SitesPage />} />}
        {canSites && <Route path="/sites/:id" element={<SiteDetailPage />} />}
        {can("databases") && <Route path="/databases" element={<DatabasesPage />} />}
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
