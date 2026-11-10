# Capsule

Debian 13 / Ubuntu için açık kaynak sunucu ve hosting yönetim paneli. Rust arka uç (tek statik ikili dosya, arayüz içine gömülü) + React arayüz.

İlerleme ve yol haritası: `docs/DURUM.md` (her anlamlı adımdan sonra güncellenir).
Açık kaynak panellerden çıkarılan somut notlar: `docs/arastirma-notlari.md`.

## Dil kuralları

- Kod İngilizce: değişken, fonksiyon, dosya, rota, JSON anahtarı, tablo/sütun adı.
- Açıklamalar Türkçe: yorumlar, belgeler, kullanıcıya görünen metinler ve hata iletileri.
- Arayüzde her metin `t("Türkçe metin")` ile sarılır; Türkçe kaynak metin aynı zamanda çeviri anahtarıdır (`web/src/lib/i18n.ts`).

## Yapı

- `server/` — Rust (axum + rusqlite). `src/api/` HTTP uçları, `src/sys/` işletim sistemiyle konuşan katman, `src/jobs.rs` uzun görevler.
- `web/` — React 19 + Vite + Tailwind 4 + TanStack Query. `src/components/ui.tsx` ortak bileşenler.
- `install/` — systemd birimi ve kurulum betiği.
- `scripts/deploy-dev.sh` — arayüzü ve arka ucu derler, test makinesine (root@192.168.1.25, Debian 13 LXC) yükler.

## Geliştirme

- Derleme yerelde yapılır: `cargo zigbuild --target x86_64-unknown-linux-musl` (zig ve cargo-zigbuild pip ile kurulu; yollar `deploy-dev.sh` içinde).
- Arayüzü canlı görmek: `cd web && pnpm dev` (API istekleri test makinesine vekillenir).
- Test paneli: https://192.168.1.25:8443 — test hesabı bilgileri `.env` dosyasında (depoya girmez).

## Güvenlik kuralları (pazarlık yok)

Diğer panellerin açıklarının ortak kökü kabuk dizesi birleştirmek ve kullanıcı girdisini ayrıştırılan dosyalara serbestçe yazmaktı. Bu yüzden:

- Komutlar yalnızca `sys::exec` üzerinden, program + argüman dizisi olarak çalıştırılır. `sh -c` ve dize birleştirme yasak.
- Kullanıcıdan gelen her ad (alan adı, kullanıcı adı, veritabanı adı, yol) kullanılmadan önce beyaz liste kuralıyla doğrulanır.
- Yapılandırma dosyaları şablondan üretilir; kullanıcı metni şablona ancak doğrulanıp kaçışlandıktan sonra girer. Yazılan nginx yapılandırması `nginx -t` ile sınanır, hata varsa eski hâli geri konur.
- Kullanıcı dosyalarına erişim, o kullanıcının uid'siyle çalışan alt süreç üzerinden yapılır (yol denetimine güvenilmez).
- Her API ucu `AuthUser` alır; kaynak sahipliği `owner_id()` ile, alt hesap yetkisi `require(alan)` ile denetlenir.
- Değişiklik yapan her işlem `auth::audit` ile denetim günlüğüne yazılır.
- IP tabanlı kararlar yalnızca gerçek bağlantı adresine bakar; `X-Forwarded-For` gibi başlıklara güvenilmez.

## Veritabanı

SQLite, `server/src/db.rs` içindeki `MIGRATIONS` dizisi. Var olan göç asla değiştirilmez; yenisi sona eklenir.
