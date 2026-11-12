//! Güncellemeden önce alınan yerel geri dönüş noktası: site dosyalarının (yüklenen ortam
//! dosyaları hariç) ve veritabanının hesabın `tmp` klasöründeki geçici kopyası. Güncelleme
//! siteyi bozarsa bu kopya geri konur; her durumda güncelleme bitince silinir.
//! Yedek yerine alınan yedeğin yerini tutmaz: o kalıcıdır, bu yalnızca güncelleme süresince durur.

use std::path::PathBuf;

use super::{staging, Wp, QUICK};
use crate::auth;
use crate::error::{AppError, AppResult};
use crate::hosting::sites::Site;
use crate::jobs::JobCtx;
use crate::sys::runas;

/// Güncelleme öncesi geri dönüş noktası.
pub struct Snapshot {
    dir: PathBuf,
}

/// Yüklenen ortam dosyaları güncellemeyle değişmez ve çok yer tutar; kopyaya girmez.
const EXCLUDES: [&str; 3] = ["--exclude=/wp-content/uploads/", "--exclude=/wp-content/cache/", "--exclude=/.maintenance"];

/// Geri dönüş noktası alır. Alınamazsa (ör. disk dolu) uyarıp `None` döner; güncelleme yine yapılır.
pub async fn take(ctx: &JobCtx, site: &Site) -> Option<Snapshot> {
    match try_take(ctx, site).await {
        Ok(snapshot) => Some(snapshot),
        Err(err) => {
            ctx.log(format!("UYARI: geri dönüş noktası alınamadı ({err}); güncelleme bozarsa site kendiliğinden eski hâline dönemez."));
            None
        }
    }
}

async fn try_take(ctx: &JobCtx, site: &Site) -> AppResult<Snapshot> {
    let wp = Wp::for_site(site)?;
    staging::ensure_rsync(ctx).await?;
    let tmp = wp.user.tmp_dir();
    std::fs::create_dir_all(&tmp).ok();
    let _ = std::os::unix::fs::chown(&tmp, Some(wp.user.uid), Some(wp.user.gid));
    let dir = tmp.join(format!("pixpanel-guncelleme-{}", auth::random_token(8)));
    let snapshot = Snapshot { dir: dir.clone() };
    let files = dir.join("files");
    let made = runas::output(runas::command(&wp.user, &tmp, "mkdir", &["-m", "700", "-p", "--", &files.to_string_lossy()]), QUICK).await?;
    if !made.success() {
        return Err(AppError::internal("geçici klasör açılamadı"));
    }
    ctx.log("== Geri dönüş noktası alınıyor (güncelleme siteyi bozarsa kendiliğinden geri konur)");
    let taken = async {
        let from = format!("{}/", wp.docroot.display());
        let to = format!("{}/", files.display());
        // `--no-g`: hesap `www-data` grubunun üyesi değildir; grup taşınmaya kalkılırsa geri yüklemede
        // yayın klasörünün grubu değişir ve web sunucusu klasöre giremez.
        let mut args = vec!["-a", "--no-g"];
        args.extend(EXCLUDES);
        args.extend(["--", from.as_str(), to.as_str()]);
        ctx.run_streaming(&mut runas::command(&wp.user, &wp.docroot, "rsync", &args), "rsync", None).await?;
        let dump = dir.join("db.sql").to_string_lossy().into_owned();
        wp.stream(ctx, &["db", "export", &dump, "--add-drop-table"], None).await
    }
    .await;
    if let Err(err) = taken {
        discard(site, snapshot).await;
        return Err(err);
    }
    Ok(snapshot)
}

/// Siteyi geri dönüş noktasındaki hâline döndürür.
pub async fn restore(ctx: &JobCtx, site: &Site, snapshot: &Snapshot) -> AppResult<()> {
    let wp = Wp::for_site(site)?;
    ctx.log("== Site güncellemeden önceki hâline döndürülüyor");
    let from = format!("{}/", snapshot.dir.join("files").display());
    let to = format!("{}/", wp.docroot.display());
    let mut args = vec!["-a", "--no-g", "--delete"];
    args.extend(EXCLUDES);
    args.extend(["--", from.as_str(), to.as_str()]);
    ctx.run_streaming(&mut runas::command(&wp.user, &wp.docroot, "rsync", &args), "rsync", None).await?;
    let dump = snapshot.dir.join("db.sql").to_string_lossy().into_owned();
    wp.stream(ctx, &["db", "import", &dump], None).await?;
    let _ = wp.run(&["cache", "flush"]).await;
    Ok(())
}

/// Geri dönüş noktasını siler.
pub async fn discard(site: &Site, snapshot: Snapshot) {
    let Ok(wp) = Wp::for_site(site) else { return };
    let tmp = wp.user.tmp_dir();
    // Yalnızca bu modülün açtığı klasör silinir; ad rastgele üretilmiştir ve `tmp`in içindedir.
    if snapshot.dir.parent() == Some(tmp.as_path()) {
        let _ = runas::output(runas::command(&wp.user, &tmp, "rm", &["-rf", "--", &snapshot.dir.to_string_lossy()]), QUICK).await;
    }
}
