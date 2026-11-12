// Veritabanı sunucusuna uzaktan erişim şeridi (yalnızca yönetici): durum, aç/kapa ve güvenlik duvarı notu.
// Kullanıcı başına adres ayrı yerde (Databases.tsx) seçilir; burası sunucunun dışarıdan bağlantı kabul edip etmediğini yönetir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import { Badge, Button, Mono } from "../components/ui";
import type { DbEngine, DbRemoteStatus } from "../lib/api";
import { api } from "../lib/api";
import { t } from "../lib/i18n";

export function RemoteAccessStrip({ engine, label }: { engine: DbEngine; label: string }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const status = useQuery({
    queryKey: ["databases-remote"],
    queryFn: () => api.get<{ engines: DbRemoteStatus[] }>("/databases/remote"),
  });

  const toggle = useMutation({
    mutationFn: (enabled: boolean) => api.put<{ changed: boolean; tls: boolean | null }>(`/databases/remote/${engine}`, { enabled }),
    onSuccess: (_, enabled) =>
      feedback.success(enabled ? t("{name} uzaktan bağlantılara açıldı", { name: label }) : t("{name} yalnızca yerel bağlantı kabul ediyor", { name: label })),
    onError: (error) => feedback.error(error),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["databases-remote"] });
      void queryClient.invalidateQueries({ queryKey: ["databases"] });
    },
  });

  const info = status.data?.engines.find((item) => item.id === engine);
  if (!info) return null;

  async function change(enabled: boolean) {
    const confirmed = await feedback.confirm({
      title: enabled ? t("{name} uzaktan bağlantılara açılsın mı?", { name: label }) : t("{name} uzaktan bağlantılara kapatılsın mı?", { name: label }),
      description: enabled
        ? t("Veritabanı sunucusu yeniden başlatılır, açık bağlantılar kopar. Sunucu dışarıdaki adreslerde de dinlemeye başlar; hangi kullanıcının nereden bağlanabileceğini kullanıcı satırlarından seçersiniz.")
        : t("Veritabanı sunucusu yeniden başlatılır, açık bağlantılar kopar. Sunucu yalnızca bu makineden gelen bağlantıları kabul eder; kullanıcıların kayıtlı uzak adresleri silinmez, ayar yeniden açılınca geçerli olur."),
      confirmLabel: enabled ? t("Aç ve yeniden başlat") : t("Kapat ve yeniden başlat"),
    });
    if (confirmed) toggle.mutate(enabled);
  }

  const warnings: string[] = [];
  if (info.remote_enabled && info.tls === false) {
    warnings.push(
      info.id === "mariadb"
        ? t("MariaDB’de TLS açık görünmüyor; uzak adresli kullanıcılar TLS şartı yüzünden bağlanamaz. Uzaktan erişimi kapatıp yeniden açın.")
        : t("Sunucuda TLS kapalı; uzak bağlantılar şifresiz gider. PostgreSQL’de ssl ayarını açmanız önerilir."),
    );
  }
  // Panel güvenlik duvarı portunu kendiliğinden açmaz; yalnızca bilgi verir.
  const firewallClosed = info.firewall_enabled && !info.firewall_port_open;

  return (
    <div className="mb-4 rounded-lg border border-line bg-surface px-4 py-3 sm:px-6">
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
          <span className="font-medium text-ink">{t("Uzaktan erişim")}</span>
          <Badge tone={info.remote_enabled ? "warn" : "idle"}>{info.remote_enabled ? t("Açık") : t("Kapalı")}</Badge>
          {info.remote_enabled && info.tls !== false && (
            <span title={t("Uzak adresli kullanıcılar yalnızca şifreli (TLS) bağlanabilir.")}>
              <Badge tone="ok">{t("TLS zorunlu")}</Badge>
            </span>
          )}
          {info.listen && (
            <span className="text-[13.5px] text-ink-3">
              {t("Dinlenen adres")}: <Mono>{info.listen}</Mono>:<Mono>{info.port}</Mono>
            </span>
          )}
          <span className="text-[13.5px] text-ink-3">{t("Uzak adresli kullanıcı: {n}", { n: info.remote_users })}</span>
        </div>
        <Button size="sm" disabled={!info.running} loading={toggle.isPending} onClick={() => void change(!info.remote_enabled)}>
          {info.remote_enabled ? t("Uzaktan erişimi kapat") : t("Uzaktan bağlantıya izin ver")}
        </Button>
      </div>
      {!info.remote_enabled && (
        <p className="mt-1.5 text-[13.5px] text-ink-3">
          {t("Şimdi yalnızca bu sunucudan bağlanılabiliyor. Uzak adres tanımlı kullanıcılar, bu ayar açılana kadar dışarıdan bağlanamaz.")}
        </p>
      )}
      {warnings.map((warning) => (
        <p key={warning} className="mt-1.5 text-[13.5px] text-warn">
          {warning}
        </p>
      ))}
      {firewallClosed && (
        <p className={info.remote_enabled ? "mt-1.5 text-[13.5px] text-warn" : "mt-1.5 text-[13.5px] text-ink-3"}>
          {info.remote_enabled
            ? t("Güvenlik duvarında {port} portu kapalı; dışarıdan bağlanılabilmesi için açılması gerekir.", { port: info.port })
            : t("Güvenlik duvarında {port} portu kapalı; ayarı açtıktan sonra dışarıdan bağlanmak için açmanız gerekir.", { port: info.port })}{" "}
          <Link to="/firewall" className="font-medium text-accent hover:underline">
            {t("Güvenlik duvarı sayfasına git")}
          </Link>
        </p>
      )}
    </div>
  );
}
