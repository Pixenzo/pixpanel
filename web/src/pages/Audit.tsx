import { useInfiniteQuery } from "@tanstack/react-query";
import { Button, EmptyState, ErrorBlock, LoadingBlock, Mono, PageHeader, Panel, Table, Td, Th } from "../components/ui";
import type { AuditEntry } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatDateTime } from "../lib/format";
import { t } from "../lib/i18n";

const PAGE_SIZE = 100;

/** Eylem kodlarının okunur karşılıkları; bilinmeyen kod olduğu gibi gösterilir. */
const actionLabels: Record<string, string> = {
  "auth.login": "Oturum açtı",
  "auth.password_change": "Parolasını değiştirdi",
  "token.create": "API anahtarı oluşturdu",
  "token.delete": "API anahtarı sildi",
  "user.create": "Hesap oluşturdu",
  "user.update": "Hesabı güncelledi",
  "user.delete": "Hesabı sildi",
  "service.action": "Servis işlemi yaptı",
  "component.install": "Bileşen kurdu",
  "node.update": "Node.js sürümünü güncelledi",
  "site.create": "Site oluşturdu",
  "site.update": "Site ayarlarını değiştirdi",
  "site.delete": "Siteyi sildi",
  "ssl.self_signed": "Kendinden imzalı sertifika üretti",
  "ssl.custom": "Sertifika yükledi",
  "ssl.letsencrypt": "Let's Encrypt sertifikası istedi",
  "ssl.disable": "Sertifikayı kaldırdı",
  "process.signal": "Sürece sinyal gönderdi",
  "terminal.open": "Terminal açtı",
  "cron.create": "Zamanlanmış görev oluşturdu",
  "cron.update": "Zamanlanmış görevi değiştirdi",
  "cron.delete": "Zamanlanmış görevi sildi",
  "cron.run": "Zamanlanmış görevi elle çalıştırdı",
  "file.write": "Dosyayı kaydetti",
  "file.mkdir": "Klasör oluşturdu",
  "file.rename": "Yeniden adlandırdı",
  "file.move": "Taşıdı",
  "file.copy": "Kopyaladı",
  "file.delete": "Dosya sildi",
  "file.chmod": "İzinleri değiştirdi",
  "file.compress": "Arşiv oluşturdu",
  "file.extract": "Arşivi açtı",
  "file.upload": "Dosya yükledi",
  "database.create": "Veritabanı oluşturdu",
  "database.delete": "Veritabanını sildi",
  "database.export": "Veritabanını dışa aktardı",
  "database.import": "Veritabanına içe aktardı",
  "database.grant": "Veritabanı erişimi verdi",
  "database.revoke": "Veritabanı erişimini kaldırdı",
  "database.user_create": "Veritabanı kullanıcısı oluşturdu",
  "database.user_password": "Veritabanı kullanıcısının parolasını değiştirdi",
  "database.user_delete": "Veritabanı kullanıcısını sildi",
  "docker.container.start": "Konteyneri başlattı",
  "docker.container.stop": "Konteyneri durdurdu",
  "docker.container.restart": "Konteyneri yeniden başlattı",
  "docker.container.pause": "Konteyneri duraklattı",
  "docker.container.unpause": "Konteyneri sürdürdü",
  "docker.container.kill": "Konteyneri zorla durdurdu",
  "docker.stack.up": "Yığını başlattı",
  "docker.stack.down": "Yığını durdurdu",
  "docker.stack.restart": "Yığını yeniden başlattı",
  "node.start": "Uygulamayı başlattı",
  "node.stop": "Uygulamayı durdurdu",
  "node.restart": "Uygulamayı yeniden başlattı",
  "redis.key.set": "Redis anahtarını değiştirdi",
  "redis.key.expire": "Redis anahtarını değiştirdi",
  "redis.key.persist": "Redis anahtarını değiştirdi",
  "redis.key.rename": "Redis anahtarını değiştirdi",
  "redis.key.hset": "Redis anahtarını değiştirdi",
  "redis.key.hdel": "Redis anahtarını değiştirdi",
  "redis.key.rpush": "Redis anahtarını değiştirdi",
  "redis.key.lset": "Redis anahtarını değiştirdi",
  "auth.2fa.enable": "İki adımlı doğrulamayı açtı",
  "auth.2fa.disable": "İki adımlı doğrulamayı kapattı",
  "auth.2fa.recovery_codes": "Kurtarma kodlarını yeniledi",
  "user.2fa.reset": "Hesabın iki adımlı doğrulamasını sıfırladı",
  "user.access": "SSH erişimini değiştirdi",
  "user.access.key.add": "SSH anahtarı ekledi",
  "user.access.key.remove": "SSH anahtarını kaldırdı",
  "user.access.password": "SSH parolasını değiştirdi",
  "user.impersonate": "Müşteri olarak giriş yaptı",
  "user.impersonate.stop": "Müşteri oturumundan çıktı",
  "package.create": "Paket oluşturdu",
  "package.update": "Paketi değiştirdi",
  "package.delete": "Paketi sildi",
  "site.clone": "Siteyi kopyaladı",
  "site.migrate": "Başka sunucudan site taşıdı",
  "wordpress.security.fix": "WordPress güvenlik düzeltmesi uyguladı",
  "dns.zone.template": "DNS hazır kayıt şablonu uyguladı",
  "database.remote.enable": "Veritabanı uzaktan erişimini açtı",
  "database.remote.disable": "Veritabanı uzaktan erişimini kapattı",
  "database.user.host": "Veritabanı kullanıcısının bağlantı adresini değiştirdi",
  "system.http3": "HTTP/3 ayarını değiştirdi",
  "mail.mailbox.self_password": "Kutu sahibi parolasını değiştirdi",
  "mail.quarantine.release": "Karantinadan ileti bıraktı",
  "mail.quarantine.delete": "Karantinadan ileti sildi",
  "mail.quarantine.bulk_delete": "Karantinadan toplu ileti sildi",
  "component.uninstall": "Bileşen kaldırdı",
  "system.updates.refresh": "Paket listesini yeniledi",
  "system.updates.install": "Sistem güncellemesi kurdu",
  "system.reboot": "Sunucuyu yeniden başlattı",
  "system.reboot.cancel": "Yeniden başlatmayı iptal etti",
  "component.remove": "Bileşen kaldırdı",
  "branding.reseller.update": "Bayi markasını güncelledi",
  "branding.reseller.delete": "Bayi markasını kaldırdı",
  "mail.mailbox.import": "Posta kutusuna ileti aktardı",
  "database.copy": "Veritabanını kopyaladı",
  "database.maintenance": "Veritabanı bakımı yaptı",
  "database.slow_log": "Yavaş sorgu günlüğü ayarını değiştirdi",
  "webhook.create": "Web kancası ekledi",
  "webhook.update": "Web kancasını değiştirdi",
  "webhook.delete": "Web kancasını sildi",
  "webhook.secret.rotate": "Web kancasının gizli anahtarını yeniledi",
  "git.signature.set": "Git web kancası imza anahtarını tanımladı",
  "git.signature.clear": "Git web kancası imza anahtarını kaldırdı",
  "mail.limit.hit": "Posta gönderim sınırına takıldı",
  "mail.limit.settings": "Gönderim sınırı ayarını değiştirdi",
  "dns.dnssec.enable": "DNSSEC'i açtı",
  "dns.dnssec.disable": "DNSSEC'i kapattı",
  "dns.zone.import": "Bölge dosyası içe aktardı",
  "site.cache.purge": "Site önbelleğini temizledi",
  "ftp.create": "FTP hesabı oluşturdu",
  "ftp.update": "FTP hesabını değiştirdi",
  "ftp.delete": "FTP hesabını sildi",
  "git.update": "Git yayınlama ayarını değiştirdi",
  "git.deploy": "Git ile yayınladı",
  "git.remove": "Git bağlantısını kaldırdı",
  "git.webhook.rotate": "Git tetikleyici adresini yeniledi",
  "node.npm": "npm komutu çalıştırdı",
  "python.pip": "pip komutu çalıştırdı",
  "wordpress.install": "WordPress kurdu",
  "wordpress.staging.create": "WordPress hazırlık ortamı açtı",
  "wordpress.staging.push": "Hazırlık ortamını canlıya aktardı",
  "wordpress.staging.refresh": "Hazırlık ortamını canlıdan yeniledi",
  "wordpress.update": "WordPress'i güncelledi",
  "wordpress.extension": "WordPress eklentisi ya da temasında işlem yaptı",
  "wordpress.settings": "WordPress ayarlarını değiştirdi",
  "wordpress.login": "WordPress yönetimine tek tıkla girdi",
  "database.open_tool": "Veritabanı aracını açtı",
  "dns.setup": "DNS sunucusunu yapılandırdı",
  "dns.settings": "DNS ayarlarını değiştirdi",
  "dns.zone.create": "DNS bölgesi oluşturdu",
  "dns.zone.delete": "DNS bölgesini sildi",
  "dns.zone.sync": "DNS bölgesini eşitledi",
  "dns.record.create": "DNS kaydı ekledi",
  "dns.record.update": "DNS kaydını değiştirdi",
  "dns.record.delete": "DNS kaydını sildi",
  "dns.cloudflare.connect": "Cloudflare hesabını bağladı",
  "dns.cloudflare.disconnect": "Cloudflare bağlantısını kaldırdı",
  "dns.cloudflare.purge": "Cloudflare önbelleğini temizledi",
  "dns.cloudflare.settings": "Cloudflare ayarlarını değiştirdi",
  "dns.cluster": "DNS kümesini değiştirdi",
  "dns.replica": "Sunucuyu DNS kümesine ikincil olarak bağladı",
  "dns.replica.detach": "Sunucuyu DNS kümesinden ayırdı",
  "mail.setup": "Posta sunucusunu yapılandırdı",
  "mail.settings": "Posta ayarlarını değiştirdi",
  "mail.domain.create": "Posta alan adı ekledi",
  "mail.domain.update": "Posta alan adını değiştirdi",
  "mail.domain.delete": "Posta alan adını sildi",
  "mail.domain.dns": "Posta DNS kayıtlarını ekledi",
  "mail.mailbox.create": "Posta kutusu oluşturdu",
  "mail.mailbox.update": "Posta kutusunu değiştirdi",
  "mail.mailbox.delete": "Posta kutusunu sildi",
  "mail.alias.create": "Takma ad ekledi",
  "mail.alias.update": "Takma adı değiştirdi",
  "mail.alias.delete": "Takma adı sildi",
  "mail.queue.flush": "Posta kuyruğunu yeniden denetti",
  "mail.queue.delete": "Kuyruktaki postayı sildi",
  "smtp.update": "Bildirim SMTP ayarını değiştirdi",
  "notify.update": "Bildirim ayarlarını değiştirdi",
  "branding.update": "Marka ayarlarını değiştirdi",
  "backup.target.create": "Yedek yeri ekledi",
  "backup.target.update": "Yedek yerini değiştirdi",
  "backup.target.password": "Yedek şifreleme parolasını görüntüledi",
  "backup.target.delete": "Yedek yerini sildi",
  "backup.plan.create": "Yedek planı oluşturdu",
  "backup.plan.update": "Yedek planını değiştirdi",
  "backup.plan.delete": "Yedek planını sildi",
  "backup.run": "Yedek aldı",
  "backup.restore": "Yedekten geri yükledi",
  "backup.snapshot.delete": "Yedeği sildi",
  "docker.container.create": "Konteyner oluşturdu",
  "docker.container.remove": "Konteyneri sildi",
  "docker.image.pull": "İmaj indirdi",
  "docker.image.remove": "İmajı sildi",
  "docker.image.prune": "Kullanılmayan imajları temizledi",
  "docker.network.create": "Docker ağı oluşturdu",
  "docker.network.remove": "Docker ağını sildi",
  "docker.volume.create": "Docker birimi oluşturdu",
  "docker.volume.remove": "Docker birimini sildi",
  "docker.stack.create": "Yığın oluşturdu",
  "docker.stack.update": "Yığını değiştirdi",
  "docker.stack.remove": "Yığını sildi",
  "fail2ban.settings": "Saldırı engelleme ayarlarını değiştirdi",
  "fail2ban.ban": "Adresi engelledi",
  "fail2ban.unban": "Adresin engelini kaldırdı",
  "firewall.apply": "Güvenlik duvarı kurallarını uyguladı",
  "firewall.confirm": "Güvenlik duvarı değişikliğini onayladı",
  "firewall.revert": "Güvenlik duvarı değişikliğini geri aldı",
  "health.fix": "Sağlık merkezinden düzeltme uyguladı",
  "health.settings": "Sağlık denetimi ayarlarını değiştirdi",
  "ip.add": "IP adresi ekledi",
  "ip.update": "IP adresini değiştirdi",
  "ip.remove": "IP adresini kaldırdı",
  "panel.certificate.custom": "Panele sertifika yükledi",
  "panel.certificate.letsencrypt": "Panel için Let's Encrypt sertifikası aldı",
  "panel.certificate.selfsigned": "Panel için kendinden imzalı sertifika üretti",
  "panel.update": "Paneli güncelledi",
  "php.settings": "PHP ayarlarını değiştirdi",
  "php.extension": "PHP eklentisini açtı ya da kapattı",
  "php.extension.install": "PHP eklentisi kurdu",
  "redis.config": "Redis ayarlarını değiştirdi",
  "redis.command": "Redis komutu çalıştırdı",
  "redis.flush": "Redis veritabanını boşalttı",
  "redis.key.delete": "Redis anahtarı sildi",
  "redis.account.enable": "Hesaba özel Redis açtı",
  "redis.account.disable": "Hesaba özel Redis'i kapattı",
  "redis.account.memory": "Hesap Redis'inin bellek sınırını değiştirdi",
  "redis.account.password": "Hesap Redis'inin parolasını yeniledi",
};

export function AuditPage() {
  const audit = useInfiniteQuery({
    queryKey: ["audit"],
    queryFn: ({ pageParam }) =>
      api.get<AuditEntry[]>(`/system/audit?limit=${PAGE_SIZE}${pageParam ? `&before_id=${pageParam}` : ""}`),
    initialPageParam: 0,
    getNextPageParam: (lastPage) => (lastPage.length === PAGE_SIZE ? lastPage[lastPage.length - 1].id : undefined),
  });

  const entries = audit.data?.pages.flat() ?? [];

  return (
    <>
      <PageHeader title={t("Denetim günlüğü")} description={t("Panelde kimin, ne zaman, hangi işlemi yaptığının kaydı.")} />
      {audit.isError && <ErrorBlock message={errorMessage(audit.error)} onRetry={() => void audit.refetch()} />}
      {audit.isPending && <LoadingBlock />}
      {audit.data && (
        <Panel flush>
          {entries.length === 0 ? (
            <EmptyState title={t("Henüz kayıt yok")} />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Zaman")}</Th>
                  <Th>{t("Kullanıcı")}</Th>
                  <Th>{t("İşlem")}</Th>
                  <Th>{t("Hedef")}</Th>
                  <Th>{t("IP adresi")}</Th>
                </tr>
              </thead>
              <tbody>
                {entries.map((entry) => (
                  <tr key={entry.id} className="hover:bg-sunken/50">
                    <Td className="whitespace-nowrap text-ink-2">{formatDateTime(entry.created_at)}</Td>
                    <Td>
                      <Mono>{entry.username}</Mono>
                    </Td>
                    <Td>{actionLabels[entry.action] ? t(actionLabels[entry.action]) : <Mono>{entry.action}</Mono>}</Td>
                    <Td>
                      {entry.target && <Mono>{entry.target}</Mono>}
                      {entry.detail && <span className="ml-2 text-ink-3">{entry.detail}</span>}
                    </Td>
                    <Td>
                      <Mono className="text-ink-2">{entry.ip}</Mono>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
          {audit.hasNextPage && (
            <div className="p-3">
              <Button onClick={() => void audit.fetchNextPage()} loading={audit.isFetchingNextPage}>
                {t("Daha eski kayıtları yükle")}
              </Button>
            </div>
          )}
        </Panel>
      )}
    </>
  );
}
