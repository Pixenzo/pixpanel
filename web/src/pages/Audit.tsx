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
