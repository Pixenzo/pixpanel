// Çok dillilik: kaynak metin Türkçedir ve aynı zamanda anahtar görevi görür.
// Başka bir dil için sözlüğe "Türkçe metin" → "çeviri" eşlemesi eklenir;
// çevirisi olmayan metin Türkçe gösterilir.

import { en } from "./locales/en";

type Dictionary = Record<string, string>;

const dictionaries: Record<string, Dictionary> = {
  en,
};

const LANGUAGE_KEY = "pixpanel-language";

/** Son bilinen dil tarayıcıda saklanır; böylece sayfa açılırken ilk çizim doğru dille yapılır. */
function storedLanguage(): string {
  try {
    return localStorage.getItem(LANGUAGE_KEY) ?? "tr";
  } catch {
    return "tr";
  }
}

let currentLanguage = storedLanguage();
document.documentElement.lang = currentLanguage;

/**
 * Etkin dili değiştirir. `t` çağıran bileşenler kendiliğinden yeniden çizilmediği için
 * dil gerçekten değişiyorsa (ör. oturum açıldıktan sonra kullanıcının dili farklı çıktı)
 * sayfa bir kez yeniden yüklenir. Saklama yapılamıyorsa döngüye girmemek için yüklenmez.
 */
export function setLanguage(language: string) {
  if (language === currentLanguage) return;
  currentLanguage = language;
  document.documentElement.lang = language;
  try {
    localStorage.setItem(LANGUAGE_KEY, language);
    if (localStorage.getItem(LANGUAGE_KEY) === language) window.location.reload();
  } catch {
    // Saklama kapalıysa dil bu oturumda yalnızca sonradan çizilen metinlerde etkili olur.
  }
}

/** Etkin dilin kodu (`tr`, `en`); tarih ve sayı biçimleri için. */
export function getLanguage(): string {
  return currentLanguage;
}

/** Metni etkin dile çevirir; `{ad}` biçimindeki yer tutucuları doldurur. */
export function t(text: string, values?: Record<string, string | number>): string {
  let result = dictionaries[currentLanguage]?.[text] ?? text;
  if (values) {
    for (const [key, value] of Object.entries(values)) {
      result = result.replaceAll(`{${key}}`, String(value));
    }
  }
  return result;
}
