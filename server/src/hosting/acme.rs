//! ACME istemcisi: Let's Encrypt (ya da uyumlu bir sağlayıcı) üzerinden
//! ücretsiz sertifika alma.
//!
//! Doğrulama HTTP-01 ile yapılır: sağlayıcının istediği dosya, bütün
//! sitelerin nginx yapılandırmasında ortak olan
//! `/.well-known/acme-challenge/` klasörüne yazılır.

use std::path::{Path, PathBuf};

use instant_acme::{
    Account, AccountBuilder, AccountCredentials, AuthorizationStatus, ChallengeType, Identifier, NewAccount, NewOrder,
    OrderStatus, RetryPolicy,
};

use crate::config::Config;
use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::hosting::atomic_write;
use crate::hosting::nginx::ACME_ROOT;
use crate::jobs::JobCtx;

fn acme_error(err: instant_acme::Error) -> AppError {
    AppError::internal(format!("sertifika sağlayıcısı hata döndü: {err}"))
}

fn builder(cfg: &Config) -> AppResult<AccountBuilder> {
    match &cfg.acme_root_ca {
        Some(path) => Account::builder_with_root(path),
        None => Account::builder(),
    }
    .map_err(acme_error)
}

/// Sağlayıcıdaki hesabı açar; ilk kullanımda hesap oluşturulup kimlik
/// bilgileri veritabanına kaydedilir.
async fn account(db: &Db, cfg: &Config) -> AppResult<Account> {
    let key = format!("acme_account:{}", cfg.acme_directory);
    if let Some(saved) = db.setting(&key)? {
        let credentials: AccountCredentials =
            serde_json::from_str(&saved).map_err(|err| AppError::internal(format!("ACME hesabı okunamadı: {err}")))?;
        return builder(cfg)?.from_credentials(credentials).await.map_err(acme_error);
    }
    let email = db.setting("acme_email")?.filter(|value| !value.trim().is_empty()).map(|value| format!("mailto:{}", value.trim()));
    let contact: Vec<&str> = email.iter().map(String::as_str).collect();
    let (account, credentials) = builder(cfg)?
        .create(
            &NewAccount { contact: &contact, terms_of_service_agreed: true, only_return_existing: false },
            cfg.acme_directory.clone(),
            None,
        )
        .await
        .map_err(acme_error)?;
    let serialized = serde_json::to_string(&credentials).map_err(|err| AppError::internal(err.to_string()))?;
    db.set_setting(&key, &serialized)?;
    Ok(account)
}

/// Doğrulama dosyasının yolu. Belirteç sağlayıcıdan geldiği için dosya adı
/// olarak kullanılmadan önce karakterleri denetlenir.
fn challenge_path(token: &str) -> AppResult<PathBuf> {
    let safe = !token.is_empty()
        && token.len() <= 128
        && token.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !safe {
        return Err(AppError::internal("sertifika sağlayıcısı geçersiz bir doğrulama belirteci gönderdi"));
    }
    Ok(Path::new(ACME_ROOT).join(".well-known/acme-challenge").join(token))
}

/// Verilen alan adları için sertifika alır; (sertifika zinciri, özel anahtar) döner.
pub async fn issue(ctx: &JobCtx, db: &Db, cfg: &Config, names: &[String]) -> AppResult<(String, String)> {
    ctx.log(format!("Sertifika isteniyor: {}", names.join(", ")));
    let account = account(db, cfg).await?;
    let identifiers: Vec<Identifier> = names.iter().map(|name| Identifier::Dns(name.clone())).collect();
    let mut order = account.new_order(&NewOrder::new(&identifiers)).await.map_err(acme_error)?;

    let mut challenge_files: Vec<PathBuf> = Vec::new();
    let outcome = async {
        let mut authorizations = order.authorizations();
        while let Some(result) = authorizations.next().await {
            let mut authorization = result.map_err(acme_error)?;
            if authorization.status == AuthorizationStatus::Valid {
                continue;
            }
            let mut challenge = authorization
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| AppError::internal("sağlayıcı HTTP doğrulaması sunmuyor"))?;
            let path = challenge_path(&challenge.token)?;
            atomic_write(&path, challenge.key_authorization().as_str().as_bytes(), 0o644)?;
            challenge_files.push(path);
            ctx.log(format!("{} için doğrulama dosyası hazırlandı", challenge.identifier()));
            challenge.set_ready().await.map_err(acme_error)?;
        }

        ctx.log("Sağlayıcının alan adlarını doğrulaması bekleniyor");
        let status = order.poll_ready(&RetryPolicy::default()).await.map_err(|err| {
            AppError::bad(format!(
                "Alan adı doğrulanamadı. Alan adının DNS kaydının bu sunucuyu gösterdiğinden ve 80. portun \
                 dışarıdan erişilebilir olduğundan emin olun. Sağlayıcının yanıtı: {err}"
            ))
        })?;
        if status != OrderStatus::Ready {
            return Err(AppError::bad(
                "Alan adı doğrulanamadı. Alan adının DNS kaydının bu sunucuyu gösterdiğinden ve 80. portun dışarıdan erişilebilir olduğundan emin olun.",
            ));
        }
        ctx.log("Doğrulama tamam, sertifika hazırlanıyor");
        let private_key = order.finalize().await.map_err(acme_error)?;
        let chain = order.poll_certificate(&RetryPolicy::default()).await.map_err(acme_error)?;
        Ok((chain, private_key))
    }
    .await;

    for path in challenge_files {
        let _ = std::fs::remove_file(path);
    }
    outcome
}
