//! Hosting çekirdeği: siteler, sistem kullanıcıları, nginx ve PHP-FPM yapılandırması.

pub mod acme;
pub mod domain;
pub mod ftp;
pub mod nginx;
pub mod node;
pub mod php;
pub mod redis_account;
pub mod sites;
pub mod sshaccess;
pub mod ssl;
pub mod sysuser;

use std::io::Write;
use std::path::Path;

use crate::error::AppResult;

/// Dosyayı önce geçici bir ada yazıp sonra yerine taşır; yarım yazılmış
/// yapılandırma dosyası hiçbir zaman ortaya çıkmaz.
pub fn atomic_write(path: &Path, content: &[u8], mode: u32) -> AppResult<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pixpanel-tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(&tmp)?;
    file.write_all(content)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
