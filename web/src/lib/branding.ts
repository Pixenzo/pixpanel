// Panelin markası (ad, logo, giriş sayfası metni, destek bağlantısı). Giriş sayfasında da
// gösterildiği için oturum gerektirmeyen uçtan okunur; oturum açıksa sunucu hesabın bağlı
// olduğu bayinin markasını, oturum yoksa isteğin geldiği alan adına bağlı bayinin markasını döner.

import { useQuery } from "@tanstack/react-query";
import { useEffect } from "react";
import { api } from "./api";
import { useSession } from "./session";

export interface Branding {
  /** Boşsa panelin kendi adı gösterilir. */
  name: string;
  /** Veri adresi biçiminde görsel; boşsa panelin kendi işareti. */
  logo: string;
  login_message: string;
  /** `https://` ya da `mailto:` adresi; boşsa "Destek" bağlantısı gösterilmez. */
  support_url: string;
}

export const DEFAULT_NAME = "PixPanel";

const EMPTY: Branding = { name: "", logo: "", login_message: "", support_url: "" };

export function useBranding(): Branding {
  const { me } = useSession();
  // Marka hesaba göre değişir: oturum açılınca ya da hesap değişince yeniden okunur.
  const query = useQuery({
    queryKey: ["branding", me ? me.id : 0],
    queryFn: () => api.get<Branding>("/branding"),
    staleTime: 5 * 60_000,
    retry: false,
  });
  return query.data ?? EMPTY;
}

/** Görünen ad: marka bir ad verdiyse o, yoksa panelin kendi adı. */
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
