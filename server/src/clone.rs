//! Site kopyalama: bir sitenin ayarlarını ve dosyalarını yeni bir alan adına çoğaltır.
//! WordPress sitelerinde veritabanı da kopyalanır ve adresler yeni alan adına çevrilir;
//! böylece kopya, asıl siteye dokunmadan denemeler yapılabilen bağımsız bir sitedir.

use std::time::Duration;

use crate::error::{AppError, AppResult};
use crate::hosting::sites::{self, Site};
use crate::hosting::sysuser;
use crate::jobs::JobCtx;
use crate::state::AppState;
use crate::sys::runas;

/// Kaynak sitenin ayarlarından kopyaya taşınanlar. Alan adına, sertifikaya ve yöneticinin
/// elle yazdığı yönergelere bağlı olanlar taşınmaz.
pub fn inherit_settings(source: &Site, target: &mut Site) {
    target.public_dir = source.public_dir.clone();
    target.php_settings = source.php_settings.clone();
    target.proxy_websocket = source.proxy_websocket;
    target.proxy_rules = source.proxy_rules.clone();
    target.template = source.template.clone();
    target.ip = source.ip.clone();
    target.node = source.node.clone();
    target.rules = source.rules.clone();
    // Asıl ad tercihi takma ada bağlıdır; kopyanın takma adı yoktur.
    target.rules.www = String::new();
    // Bakımdaki bir sitenin kopyası bakımda açılmaz; kopya zaten kimsenin bilmediği bir adrestedir.
    target.rules.maintenance = false;
}

/// Dosyaları kaynak sitenin klasöründen hedefe, hesabın kendi kullanıcısıyla kopyalar.
async fn copy_files(ctx: &JobCtx, source: &Site, target: &Site) -> AppResult<()> {
    let user = sysuser::lookup(&target.owner).ok_or_else(|| AppError::internal("Hesabın Linux kullanıcısı bulunamadı"))?;
    let from = format!("{}/.", source.root().display());
    let to = format!("{}/", target.root().display());
    ctx.log(format!("Dosyalar kopyalanıyor: {} → {}", source.root().display(), target.root().display()));
    // `-a`: izinler, zamanlar ve sembolik bağlar korunur. Hedefteki karşılama sayfasının üzerine yazılır.
    let mut cmd = runas::command(&user, &target.root(), "cp", &["-a", "--", &from, &to]);
    ctx.run_streaming(&mut cmd, "cp", None).await?;
    let size = runas::output(runas::command(&user, &target.root(), "du", &["-sh", "--", "."]), Duration::from_secs(120)).await;
    if let Ok(output) = size {
        if let Some(total) = output.stdout.split_whitespace().next() {
            ctx.log(format!("Kopyalanan boyut: {total}"));
        }
    }
    Ok(())
}

/// Kopyalama görevinin gövdesi. Hedef site kaydı ve boş klasörü önceden hazırlanmıştır.
/// Başarısız olursa yarım kalan kopya (site, dosyaları, açılan veritabanı) geri alınır.
pub async fn run(ctx: &JobCtx, state: &AppState, source: &Site, target: &Site, wordpress: bool) -> AppResult<()> {
    let outcome = async {
        copy_files(ctx, source, target).await?;
        if wordpress {
            crate::wordpress::clone_database(ctx, state, source, target).await?;
        }
        // Dosyalar yerine geldikten sonra servisler (PHP havuzu, Node.js uygulaması) yeniden kurulur.
        let _guard = state.sys_lock.lock().await;
        sites::provision(target).await
    }
    .await;
    match outcome {
        Ok(()) => {
            ctx.log(format!("{} hazır: http://{}", target.domain, target.domain));
            if !wordpress && source.kind == "php" {
                ctx.log("Not: site bir veritabanı kullanıyorsa kopya hâlâ aynı veritabanına bağlıdır. Bağımsız çalışması için yeni bir veritabanı açıp uygulamanın ayarını güncelleyin.");
            }
            Ok(())
        }
        Err(err) => {
            ctx.log(format!("Kopyalama başarısız oldu, yarım kalan site kaldırılıyor: {err}"));
            let _guard = state.sys_lock.lock().await;
            if let Err(cleanup) = sites::deprovision(target, true).await {
                tracing::warn!(error = %cleanup, domain = %target.domain, "yarım kopya temizlenemedi");
            }
            let _ = sites::delete_row(&state.db, target.id);
            Err(err)
        }
    }
}
