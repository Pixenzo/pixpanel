// Çok dillilik: kaynak metin Türkçedir ve aynı zamanda anahtar görevi görür.
// Başka bir dil için sözlüğe "Türkçe metin" → "çeviri" eşlemesi eklenir;
// çevirisi olmayan metin Türkçe gösterilir.

type Dictionary = Record<string, string>;

const dictionaries: Record<string, Dictionary> = {
  en: {},
};

let currentLanguage = "tr";

export function setLanguage(language: string) {
  currentLanguage = language;
  document.documentElement.lang = language;
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
