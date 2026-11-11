// Panelin markası (ad, logo, giriş sayfası metni). Giriş sayfasında da gösterildiği için
// oturum gerektirmeyen uçtan okunur.

import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "./api";

export interface Branding {
  /** Boşsa panelin kendi adı gösterilir. */
  name: string;
  /** Veri adresi biçiminde görsel; boşsa panelin kendi işareti. */
  logo: string;
  login_message: string;
}

export const DEFAULT_NAME = "PixPanel";

export function useBranding(): Branding {
  const query = useQuery({ queryKey: ["branding"], queryFn: () => api.get<Branding>("/branding"), staleTime: 5 * 60_000, retry: false });
  return query.data ?? { name: "", logo: "", login_message: "" };
}

/** Görünen ad: yönetici bir ad verdiyse o, yoksa panelin kendi adı. */
export function brandName(branding: Branding): string {
  return branding.name === "" ? DEFAULT_NAME : branding.name;
}

/** Tarayıcı sekmesinin başlığını marka adıyla eşler. */
export function useBrandTitle(branding: Branding) {
  const name = brandName(branding);
  useEffect(() => {
    document.title = name;
  }, [name]);
}
