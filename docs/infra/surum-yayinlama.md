# Sürüm yayınlama, kurulum ve güncelleme

## Sürüm paketi

```sh
scripts/release.sh 0.2.0            # dist/release/0.2.0/ altına hazırlar
scripts/release.sh 0.2.0 --publish  # ayrıca GitHub'da v0.2.0 olarak yayınlar
```

Betik arayüzü derler, ikiliyi `--release` ile derler (sürüm numarası `PIXPANEL_VERSION` ile ikiliye gömülür), arşivler, özetleri çıkarır ve imzalar. Çıkan dosyalar:

| Dosya | İçerik |
|---|---|
| `pixpanel-linux-x86_64.tar.gz` | `pixpanel`, `pixpanel.service`, `uninstall.sh` |
| `VERSION` | Sürüm numarası |
| `install.sh` | Kurulum betiği |
| `SHA256SUMS` | Yukarıdakilerin özetleri |
| `SHA256SUMS.sig` | Özet dosyasının ed25519 imzası |

Dosya adlarında sürüm numarası yoktur; böylece `…/releases/latest/download/<dosya>` adresi hep son sürümü verir.

**arm64:** `rustup target add aarch64-unknown-linux-musl` kurulursa betik onu da derler. Bugün kurulu değil; arm64 paketi hiç derlenmedi ve sınanmadı.

## İmzalama anahtarı

- Gizli anahtar: `.secrets/release-key.pem` (depoya girmez; yolu `.env` içinde `PIXPANEL_RELEASE_KEY`).
- Açık anahtar iki yere gömülüdür: `server/src/update.rs` (`RELEASE_PUBLIC_KEY`) ve `install/install.sh` (`PUBLIC_KEY`).
- **Bu anahtarın yedeği yalnızca bu bilgisayarda.** Kaybedilirse kurulu paneller yeni sürümleri kabul etmez; her sunucuda `update_public_key` ayarıyla yeni anahtarı tanıtmak ya da paneli betikle yeniden kurmak gerekir. Çalınırsa sahte sürüm imzalanabilir. Parola yöneticisine bir kopyasını koyun.

## Kurulum

```sh
curl -fsSL https://github.com/Pixenzo/pixpanel/releases/latest/download/install.sh | bash
```

Betik imzayı ve özetleri doğrular, ikiliyi ve servis birimini kurar, ilk yöneticiyi oluşturup parolasını bir kez gösterir. Yeniden çalıştırılırsa yükseltme yapar. Çevrimdışı kurulum: dosyaları sunucuya kopyalayıp `bash install.sh --from-dir <klasör>`.

Kaldırma: `pixpanel-uninstall` (ayar ve veriler kalır) ya da `pixpanel-uninstall --purge`.

## Panelin kendini güncellemesi

Ayarlar → Panel sürümü. Panel günde bir kez `update_url` adresine bakar (yalnızca bildirir); güncellemeyi yönetici başlatır. Akış: imza ve özet doğrulanır → yeni ikili denenir → eski ikili ve veritabanının kopyası saklanır → panel yeniden başlar. Panel art arda 8 saniye sağlıklı kalmazsa eski ikili ve veritabanı geri konur.

`/etc/pixpanel/config.toml` ayarları:

```toml
update_url = "https://github.com/Pixenzo/pixpanel/releases/latest/download"   # varsayılan
# update_public_key = "…"   # yalnızca kendi anahtarınızla imzalanan sürümler için
```

## Depo kapalıyken

`Pixenzo/pixpanel` kapalı olduğu sürece yukarıdaki GitHub adresleri kimlik sormadan indirilemez: tek satırlık kurulum komutu ve paneldeki güncelleme denetimi çalışmaz ("sunucu 404 döndü"). Seçenekler:

1. Depoyu açmak.
2. Yalnızca sürüm dosyalarının durduğu açık bir depo (ör. `Pixenzo/pixpanel-releases`) açıp `update_url` varsayılanını ve `install.sh` içindeki adresi oraya çevirmek.
3. Dosyaları kendi sunucunuzdan yayınlamak (`update_url` ve `--url`).

O zamana kadar kurulum `--from-dir` ile yapılır.

## Sınamalar

- `scripts/e2e/update.sh`: imza/özet reddi, geçerli güncelleme, başlamayan sürümde geri dönüş (test makinesinde, yerel bir sürüm sunucusuna karşı).
- `scripts/e2e/install.sh <sürüm klasörü> <konteyner no…>`: temiz Proxmox konteynerlerinde kurulum, yükseltme ve kaldırma.
- `scripts/e2e/components.sh <sürüm klasörü> <konteyner no…>`: temiz konteynerde panel kurulur, bütün bileşenler panelden sıfırdan kurulup denenir. Sürüm yayınlamadan önce 190 (Debian 13) ve 191 (Ubuntu 26.04) üzerinde koşturulur; konteynerler `scripts/e2e/ct.sh create 190 191` ile kurulur, iş bitince `ct.sh destroy 190 191` ile silinir.
