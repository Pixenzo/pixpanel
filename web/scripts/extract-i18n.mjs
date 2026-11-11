#!/usr/bin/env node
// Arayüz metinlerini çıkarır ve İngilizce sözlükle karşılaştırır.
//
// Kullanım (web/ klasöründen):
//   node scripts/extract-i18n.mjs              özet + eksik/artık anahtar raporu
//   node scripts/extract-i18n.mjs --list       tekilleştirilmiş metin listesini de yazar
//   node scripts/extract-i18n.mjs --json       listeyi JSON olarak yazar
//   node scripts/extract-i18n.mjs --check      eksik anahtar, boş çeviri ya da yer tutucu uyuşmazlığı varsa 1 ile çıkar
//   node scripts/extract-i18n.mjs --candidates değişkenle çevrilmiş olabilecek, ek listede olmayan Türkçe dizgeleri gösterir
//
// Kaynak: `web/src` altındaki .ts/.tsx dosyalarında `t("...")`, `t('...')` ve
// ifadesiz `t(`...`)` çağrıları. `t(değişken)` ile çevrilen metinler kodda
// doğrudan bulunamaz; onlar `scripts/i18n-extra.json` dosyasında elle tutulur.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const webDir = join(scriptDir, "..");
const srcDir = join(webDir, "src");
const extraFile = join(scriptDir, "i18n-extra.json");
const dictionaryFile = join(srcDir, "lib", "locales", "en.ts");

const args = new Set(process.argv.slice(2));

function walk(dir) {
  const files = [];
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) files.push(...walk(path));
    else if (/\.tsx?$/.test(name)) files.push(path);
  }
  return files;
}

/** JS dizge gövdesindeki kaçış dizilerini çözer. */
function unescapeLiteral(body) {
  return body.replace(/\\(u\{([0-9a-fA-F]+)\}|u([0-9a-fA-F]{4})|x([0-9a-fA-F]{2})|\r?\n|[\s\S])/g, (_, seq, braced, four, two) => {
    if (braced) return String.fromCodePoint(parseInt(braced, 16));
    if (four) return String.fromCharCode(parseInt(four, 16));
    if (two) return String.fromCharCode(parseInt(two, 16));
    if (seq === "n") return "\n";
    if (seq === "t") return "\t";
    if (seq === "r") return "\r";
    if (seq === "0") return "\0";
    if (seq.startsWith("\n") || seq.startsWith("\r")) return "";
    return seq;
  });
}

// `t(` öncesinde tanımlayıcı karakteri olmamalı (`format(`, `split(` gibi çağrılar elenir).
const callPattern = /(?<![\w$.])t\(\s*(?:"((?:[^"\\\n]|\\[\s\S])*)"|'((?:[^'\\\n]|\\[\s\S])*)'|`((?:[^`\\$]|\\[\s\S]|\$(?!\{))*)`)\s*[,)]/g;

function extractFromSource() {
  /** @type {Map<string, string[]>} */
  const found = new Map();
  for (const file of walk(srcDir)) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(callPattern)) {
      const text = unescapeLiteral(match[1] ?? match[2] ?? match[3]);
      const where = relative(webDir, file).replaceAll("\\", "/");
      if (!found.has(text)) found.set(text, []);
      found.get(text).push(where);
    }
  }
  return found;
}

function readExtra() {
  try {
    const data = JSON.parse(readFileSync(extraFile, "utf8"));
    // Biçim: { "kaynak dosya / açıklama": ["Türkçe metin", ...], ... } ya da düz dizi.
    const lists = Array.isArray(data) ? [data] : Object.values(data);
    if (!lists.every((list) => Array.isArray(list) && list.every((item) => typeof item === "string"))) {
      throw new Error("metin listeleri bekleniyordu");
    }
    return [...new Set(lists.flat())];
  } catch (error) {
    if (error.code === "ENOENT") return [];
    throw new Error(`${extraFile} okunamadı: ${error.message}`);
  }
}

/** `en.ts` içindeki `"anahtar": "değer",` satırlarını okur (her kayıt tek satırdır). */
function readDictionary() {
  const entries = new Map();
  let source;
  try {
    source = readFileSync(dictionaryFile, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return entries;
    throw error;
  }
  const linePattern = /^\s*("(?:[^"\\]|\\.)*")\s*:\s*("(?:[^"\\]|\\.)*")\s*,?\s*$/;
  for (const line of source.split("\n")) {
    const match = linePattern.exec(line.replace(/\r$/, ""));
    if (match) entries.set(JSON.parse(match[1]), JSON.parse(match[2]));
  }
  return entries;
}

function placeholders(text) {
  return [...text.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
}

const found = extractFromSource();
const extra = readExtra();
const used = new Set([...found.keys(), ...extra]);
const sorted = [...used].sort((a, b) => a.localeCompare(b, "tr"));

if (args.has("--candidates")) {
  // Ek listede ve çağrılarda olmayan, Türkçe harf içeren dizgeler: değişkenle çevriliyor olabilirler.
  const turkish = /[çğıöşüÇĞİÖŞÜ]/;
  const seen = new Map();
  for (const file of walk(srcDir)) {
    // Yorumlar elenir; aksi hâlde içlerindeki kesme işaretleri dizge sınırlarını bozar.
    const source = readFileSync(file, "utf8")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/^\s*\/\/.*$/gm, "")
      .replace(/\s\/\/ .*$/gm, "");
    for (const match of source.matchAll(/"((?:[^"\\\n]|\\.)*)"|'((?:[^'\\\n]|\\.)*)'|`((?:[^`\\$]|\\[\s\S]|\$(?!\{))*)`/g)) {
      const text = unescapeLiteral(match[1] ?? match[2] ?? match[3]);
      if (!turkish.test(text) || used.has(text)) continue;
      const where = relative(webDir, file).replaceAll("\\", "/");
      if (!seen.has(text)) seen.set(text, new Set());
      seen.get(text).add(where);
    }
  }
  for (const [text, files] of seen) console.log(`${[...files].join(", ")}\t${JSON.stringify(text)}`);
  console.log(`\n${seen.size} aday dizge`);
  process.exit(0);
}

if (args.has("--json")) {
  console.log(JSON.stringify(sorted, null, 2));
  process.exit(0);
}

if (args.has("--list")) {
  for (const text of sorted) console.log(JSON.stringify(text));
  console.log("");
}

const dictionary = readDictionary();
const missing = sorted.filter((text) => !dictionary.has(text));
const unused = [...dictionary.keys()].filter((key) => !used.has(key)).sort((a, b) => a.localeCompare(b, "tr"));
const mismatched = [...dictionary].filter(([key, value]) => used.has(key) && placeholders(key) !== placeholders(value));
const empty = [...dictionary].filter(([, value]) => value.trim() === "").map(([key]) => key);

console.log(`Koddan çıkarılan metin: ${found.size}`);
console.log(`Ek listeden (değişkenle çevrilen): ${extra.length}`);
console.log(`Toplam benzersiz anahtar: ${sorted.length}`);
console.log(`Sözlükteki kayıt: ${dictionary.size}`);

if (missing.length > 0) {
  console.log(`\nEksik anahtarlar (${missing.length}):`);
  for (const text of missing) console.log(`  ${JSON.stringify(text)}`);
}
if (unused.length > 0) {
  console.log(`\nArtık kullanılmayan anahtarlar (${unused.length}):`);
  for (const text of unused) console.log(`  ${JSON.stringify(text)}`);
}
if (mismatched.length > 0) {
  console.log(`\nYer tutucuları uyuşmayan çeviriler (${mismatched.length}):`);
  for (const [key, value] of mismatched) console.log(`  ${JSON.stringify(key)} -> ${JSON.stringify(value)}`);
}
if (empty.length > 0) {
  console.log(`\nBoş çeviriler (${empty.length}):`);
  for (const key of empty) console.log(`  ${JSON.stringify(key)}`);
}

if (args.has("--check") && (missing.length > 0 || mismatched.length > 0 || empty.length > 0)) {
  console.error("\nKontrol başarısız.");
  process.exit(1);
}
