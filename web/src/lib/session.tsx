// Oturum durumu ve tema tercihi.

import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { Me } from "./api";
import { ApiError, UNAUTHORIZED_EVENT, api } from "./api";
import { setLanguage } from "./i18n";

interface Session {
  /** `undefined`: henüz bilinmiyor, `null`: oturum yok. */
  me: Me | null | undefined;
  refresh: () => Promise<void>;
  logout: () => Promise<void>;
}

const SessionContext = createContext<Session | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: ["me"],
    queryFn: async () => {
      try {
        return await api.get<Me>("/auth/me");
      } catch (error) {
        if (error instanceof ApiError && error.status === 401) return null;
        throw error;
      }
    },
    retry: false,
    staleTime: Infinity,
  });

  useEffect(() => {
    // Dil hesabın kendi ayarıdır; henüz belirlenmemişse (boş) ekrandaki dil korunur.
    if (query.data?.language) setLanguage(query.data.language);
  }, [query.data]);

  // Herhangi bir istek 401 dönerse oturum düşmüş demektir.
  useEffect(() => {
    const onUnauthorized = () => queryClient.setQueryData(["me"], null);
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, [queryClient]);

  const refresh = useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: ["me"] });
  }, [queryClient]);

  const logout = useCallback(async () => {
    try {
      await api.post("/auth/logout");
    } finally {
      // Önceki kullanıcının verileri bellekte kalmasın.
      queryClient.clear();
      queryClient.setQueryData(["me"], null);
    }
  }, [queryClient]);

  return <SessionContext.Provider value={{ me: query.data, refresh, logout }}>{children}</SessionContext.Provider>;
}

export function useSession(): Session {
  const context = useContext(SessionContext);
  if (!context) throw new Error("SessionProvider eksik");
  return context;
}

/** Oturumu açık kullanıcı; yalnızca korumalı sayfalarda kullanılır. */
export function useMe(): Me {
  const { me } = useSession();
  if (!me) throw new Error("Oturum yok");
  return me;
}

// ---- Tema ----

export type Theme = "light" | "dark";
const THEME_KEY = "pixpanel-theme";

function initialTheme(): Theme {
  const saved = localStorage.getItem(THEME_KEY);
  if (saved === "light" || saved === "dark") return saved;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export function useTheme(): [Theme, () => void] {
  const [theme, setTheme] = useState<Theme>(initialTheme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  const toggle = useCallback(() => {
    setTheme((current) => {
      const next = current === "dark" ? "light" : "dark";
      localStorage.setItem(THEME_KEY, next);
      return next;
    });
  }, []);
  return [theme, toggle];
}
