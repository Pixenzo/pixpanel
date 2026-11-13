# Komut satırı

Panelde yapılan her işlem sunucuda `pixpanel` komutuyla da yapılabilir; betiklerde ve otomasyonda kullanmak içindir.

Komutlar **root** olarak çalıştırılır ve işi kendileri yapmaz: çalışan panelin API'sini çağırırlar. Bu yüzden arayüzdeki doğrulamalar, yetki denetimi ve denetim günlüğü komut satırında da aynen geçerlidir. Parola sorulmaz; komut, yalnızca sunucunun kendisinden geçerli kısa ömürlü bir anahtar üretir ve işi bitince siler. Panel servisi çalışmıyorsa komutlar "panele bağlanılamadı" hatasıyla durur.

## Dil

Komut satırı kurulumda seçilen dille çalışır (`install.sh --lang tr`; seçilmezse İngilizce). Dil `/etc/pixpanel/language` dosyasında durur ve şöyle değiştirilir:

```sh
pixpanel setlang tr     # ya da en
```

Yardım metinleri, tablo başlıkları, hata iletileri ve panelden gelen iletiler (görev günlükleri, sağlık bulguları) bu dilde yazılır. Panelin arayüz dili bundan etkilenmez; o her hesabın kendi ayarıdır. clap'in kendi kalıp metinleri (`Usage:`, `Print help`) her iki dilde de İngilizcedir.

## Hazır komutlar

```sh
pixpanel site list
pixpanel site add ornek.com                       # durağan site
pixpanel site add ornek.com --kind php            # kurulu en yeni PHP sürümüyle
pixpanel site add ornek.com --kind php --php 8.3 --owner musteri
pixpanel site add api.ornek.com --kind proxy --target http://127.0.0.1:3000
pixpanel site remove ornek.com                    # dosyalar kalır
pixpanel site remove ornek.com --delete-files

pixpanel user list
pixpanel user add musteri                         # parola üretilir ve bir kez yazdırılır
pixpanel user add bayi --role reseller --email bayi@ornek.com

pixpanel health                                   # bulguları yazar; kritik bulgu varsa 1 ile çıkar
```

## Her şey için: `pixpanel api`

Hazır komutu olmayan işlemler doğrudan API ucuyla yapılır. Yol, `/api/v1` sonrasındaki kısımdır; yanıt JSON olarak yazılır.

```sh
pixpanel api GET /sites
pixpanel api PATCH /sites/12 --data '{"template":"laravel"}'
pixpanel api POST /databases --data @veritabani.json     # gövde dosyadan
cat govde.json | pixpanel api POST /dns/zones --data -   # gövde standart girdiden
```

- `--as HESAP`: isteği o hesabın yetkisiyle yapar (varsayılan: ilk yönetici). Bir müşterinin neyi görebildiğini denemek ya da kaynağı onun adına açmak için.
- `--wait`: yanıt bir görev başlattıysa (`job_id`) bitmesini bekler, günlüğünü yazar; görev başarısızsa 1 ile çıkar.

```sh
pixpanel api POST /components/redis/install --wait
```

## Çıkış kodları

Başarıda 0; panel hata döndürdüyse (4xx/5xx), görev başarısız olduysa ya da panele ulaşılamadıysa 1. `pixpanel api` hata yanıtının gövdesini yine standart çıktıya yazar, böylece betik hata kodunu okuyabilir:

```sh
if ! pixpanel site add "$domain" --kind php; then
  echo "site açılamadı" >&2
fi
```

## Yönetim komutları (panel kapalıyken de çalışır)

```sh
pixpanel create-admin --username admin                 # --language tr|en: hesabın panel dili
pixpanel reset-password KULLANICI
pixpanel disable-two-factor KULLANICI
```
