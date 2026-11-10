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
import { DashboardPage } from "./pages/Dashboard";
import { JobsPage } from "./pages/Jobs";
import { LoginPage } from "./pages/Login";
import { ProcessesPage } from "./pages/Processes";
import { ServicesPage } from "./pages/Services";
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
  return (
    <AppShell>
      <Routes>
        <Route path="/" element={isAdmin ? <DashboardPage /> : <Navigate to="/account" replace />} />
        {isAdmin && (
          <>
            <Route path="/services" element={<ServicesPage />} />
            <Route path="/processes" element={<ProcessesPage />} />
            <Route path="/audit" element={<AuditPage />} />
          </>
        )}
        {me.role !== "member" && <Route path="/users" element={<UsersPage />} />}
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
