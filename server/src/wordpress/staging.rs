//! WordPress hazırlık ortamı: canlı sitenin, değişikliklerin önce denendiği bağlı kopyası.
//!
//! Kopya site kopyalama akışıyla açılır (`clone.rs`) ve `sites.staging_of` ile asıl siteye
//! bağlanır. Buradaki eşitleme iki yönde de aynı işi yapar: "canlıya aktar" hazırlık ortamını
//! canlı sitenin üzerine, "canlıdan yenile" canlı siteyi hazırlık ortamının üzerine yazar.
//! Her adım hesabın kendi Linux kullanıcısıyla yürür; iki site de aynı hesaba aittir.

use std::path::Path;

use super::{backup_before_update, verify_after_update, Wp};
use crate::auth;
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::runas;

/// Eşitlemenin yönü.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Hazırlık ortamı → canlı site.
    Push,
    /// Canlı site → hazırlık ortamı.
    Refresh,
}

/// Nelerin aktarılacağı.
#[derive(Clone, Copy)]
pub struct SyncOptions {
    pub files: bool,
    pub database: bool,
}

/// Hazırlık ortamının önerilen alan adı.
pub fn suggested_domain(domain: &str) -> String {
    format!("staging.{}", domain.trim_start_matches("www."))
}

/// Yeni açılan hazırlık ortamını arama motorlarına kapatır (kopya içerik olarak dizine girmesin).
pub async fn mark(ctx: &JobCtx, site: &Site) -> AppResult<()> {
    let wp = Wp::for_site(site)?;
    wp.run_ok(&["option", "update", "blog_public", "0"]).await?;
    ctx.log("Hazırlık ortamı arama motorlarına kapatıldı.");
    Ok(())
}

/// Sitenin kayıtlı adresi ve içindeki ana makine adı (ör. `https://ornek.com`, `ornek.com`).
async fn site_url(wp: &Wp) -> AppResult<(String, String)> {
    let url = wp.run_ok(&["option", "get", "siteurl"]).await?.trim_end_matches('/').to_string();
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")).unwrap_or("").to_string();
    let plain = !host.is_empty() && host.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':' | '/' | '_'));
    if !plain {
        return Err(AppError::bad("Sitenin WordPress adresi okunamadı"));
    }
    Ok((url, host))
}

/// Dosya eşitlemesi `rsync` ister; yoksa kurulur.
pub(super) async fn ensure_rsync(ctx: &JobCtx) -> AppResult<()> {
    if Path::new("/usr/bin/rsync").is_file() {
        return Ok(());
    }
    ctx.log("rsync kuruluyor");
    crate::components::apt_install(ctx, &["rsync".into()]).await
}

/// `from` sitesindeki WordPress'i `to` sitesinin üzerine yazar.
pub async fn sync(ctx: &JobCtx, state: &AppState, from: &Site, to: &Site, direction: Direction, options: SyncOptions) -> AppResult<()> {
    let src = Wp::for_site(from)?;
    let dst = Wp::for_site(to)?;
    if src.user.uid != dst.user.uid {
        return Err(AppError::bad("Hazırlık ortamı ve asıl site aynı hesapta olmalı"));
    }
    if !src.has_files().await || !dst.has_files().await {
        return Err(AppError::bad("İki sitede de çalışan bir WordPress kurulumu olmalı"));
    }
    let (_, src_host) = site_url(&src).await?;
    let (dst_url, dst_host) = site_url(&dst).await?;
    if src_host == dst_host {
        return Err(AppError::bad("İki site aynı adresi kullanıyor; hazırlık ortamının WordPress adresi kendi alan adı olmalı"));
    }
    // Canlı site arama motorlarına açık, hazırlık ortamı kapalı kalır; veritabanıyla birlikte taşınmaz.
    let visibility = match direction {
        Direction::Push => dst.run_ok(&["option", "get", "blog_public"]).await.unwrap_or_else(|_| "1".to_string()),
        Direction::Refresh => "0".to_string(),
    };
    let visibility = if visibility.trim() == "0" { "0" } else { "1" };

    if direction == Direction::Push {
        backup_before_update(ctx, state, to, "").await?;
    }
    if options.files {
        ensure_rsync(ctx).await?;
    }

    // Aktarım sürerken ziyaretçi yarım hâli görmesin diye hedef bakım kipine alınır.
    let was_in_maintenance = dst.run(&["maintenance-mode", "is-active"]).await.map(|out| out.success()).unwrap_or(false);
    if !was_in_maintenance {
        dst.run_ok(&["maintenance-mode", "activate"]).await?;
    }
    let outcome = transfer(ctx, &src, &dst, &src_host, &dst_url, visibility, options).await;
    if !was_in_maintenance {
        let _ = dst.run(&["maintenance-mode", "deactivate"]).await;
    }
    outcome?;

    if direction == Direction::Push {
        let verified = verify_after_update(ctx, to).await;
        crate::health::rescan_wordpress_site(state, to).await;
        verified?;
    }
    ctx.log(format!("{} → {} aktarımı tamamlandı.", from.domain, to.domain));
    Ok(())
}

async fn transfer(ctx: &JobCtx, src: &Wp, dst: &Wp, src_host: &str, dst_url: &str, visibility: &str, options: SyncOptions) -> AppResult<()> {
    if options.files {
        ctx.log("== Dosyalar eşitleniyor");
        let from = format!("{}/", src.docroot.display());
        let to = format!("{}/", dst.docroot.display());
        // Ayar dosyası her sitenin kendi veritabanını gösterir; taşınmaz. Hedefte bulunup kaynakta
        // olmayan dosyalar silinir, yüklenen ortam dosyaları (uploads) hariç: canlı siteye hazırlık
        // ortamı açıldıktan sonra yüklenenler kaybolmasın.
        let args = [
            "-a",
            // Yayın klasörünün grubu (web sunucusu) hedefte olduğu gibi kalır.
            "--no-g",
            "--delete",
            "--exclude=/wp-config.php",
            "--exclude=/.maintenance",
            "--exclude=/wp-content/cache/",
            "--filter=P /wp-content/uploads/**",
            "--",
            from.as_str(),
            to.as_str(),
        ];
        let mut cmd = runas::command(&dst.user, &dst.docroot, "rsync", &args);
        ctx.run_streaming(&mut cmd, "rsync", None).await?;
    }

    if options.database {
        ctx.log("== Veritabanı aktarılıyor");
        let tmp = src.user.tmp_dir();
        std::fs::create_dir_all(&tmp).ok();
        let _ = std::os::unix::fs::chown(&tmp, Some(src.user.uid), Some(src.user.gid));
        let dump = tmp.join(format!("pixpanel-hazirlik-{}.sql", auth::random_token(8)));
        let safety = tmp.join(format!("pixpanel-onceki-{}.sql", auth::random_token(8)));
        let dump_text = dump.to_string_lossy().into_owned();
        let safety_text = safety.to_string_lossy().into_owned();

        let moved = async {
            ctx.log("Hedefin bugünkü veritabanı kenara alınıyor (aktarım yarıda kalırsa geri konur)");
            dst.stream(ctx, &["db", "export", &safety_text, "--add-drop-table"], None).await?;
            src.stream(ctx, &["db", "export", &dump_text, "--add-drop-table"], None).await?;

            let loaded = async {
                dst.stream(ctx, &["db", "import", &dump_text], None).await?;
                ctx.log(format!("Adresler çevriliyor: {src_host} → {dst_url}"));
                // Sayfa kurucular adresleri JSON içinde `\/` kaçışıyla da saklar; iki biçim de çevrilir.
                let escaped_url = dst_url.replace('/', "\\/");
                for scheme in ["https", "http"] {
                    let plain = format!("{scheme}://{src_host}");
                    let escaped = format!("{scheme}:\\/\\/{}", src_host.replace('/', "\\/"));
                    for (old, new) in [(plain.as_str(), dst_url), (escaped.as_str(), escaped_url.as_str())] {
                        dst.stream(ctx, &["search-replace", old, new, "--all-tables", "--skip-columns=guid", "--report-changed-only"], None).await?;
                    }
                }
                dst.run_ok(&["option", "update", "blog_public", visibility]).await?;
                AppResult::Ok(())
            }
            .await;
            if let Err(err) = loaded {
                ctx.log(format!("Veritabanı aktarımı yarıda kaldı ({err}); hedefin veritabanı eski hâline döndürülüyor"));
                match dst.stream(ctx, &["db", "import", &safety_text], None).await {
                    Ok(()) => ctx.log("Hedefin veritabanı eski hâline döndü."),
                    Err(restore) => ctx.log(format!("UYARI: eski veritabanı geri yüklenemedi: {restore}")),
                }
                return Err(err);
            }
            AppResult::Ok(())
        }
        .await;
        let _ = std::fs::remove_file(&dump);
        let _ = std::fs::remove_file(&safety);
        moved?;
        let _ = dst.run(&["cache", "flush"]).await;
        let _ = dst.run(&["rewrite", "flush"]).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_domain_is_suggested() {
        assert_eq!(suggested_domain("ornek.com"), "staging.ornek.com");
        assert_eq!(suggested_domain("www.ornek.com"), "staging.ornek.com");
    }
}
