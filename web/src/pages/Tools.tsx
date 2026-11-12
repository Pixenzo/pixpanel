// Araçlar ve ayarlar: sunucu yönetiminin seyrek açılan sayfaları tek ekranda, konularına göre
// gruplanmış durur. Yan menü böylece her gün kullanılan sayfalara kalır.

import type { LucideIcon } from "lucide-react";
import { Archive, Boxes, Mail, Server, Settings, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { Link } from "react-router";
import { EmptyState, PageHeader, SearchInput } from "../components/ui";
import { t } from "../lib/i18n";

interface ToolLink {
  to: string;
  label: string;
  hint: string;
}

interface ToolGroup {
  title: string;
  icon: LucideIcon;
  items: ToolLink[];
}

const groups: ToolGroup[] = [
  {
    title: "Güvenlik",
    icon: ShieldCheck,
    items: [
      { to: "/firewall", label: "Güvenlik duvarı", hint: "Hangi kapı kime açık" },
      { to: "/fail2ban", label: "Saldırı engelleme", hint: "Hatalı girişleri engelleyen korumalar" },
      { to: "/settings#panel-sertifikasi", label: "Panel sertifikası", hint: "Panelin, postanın ve FTP'nin SSL sertifikası" },
      { to: "/account", label: "İki adımlı doğrulama", hint: "Hesabınız, oturumlar ve API anahtarları" },
      { to: "/audit", label: "Denetim günlüğü", hint: "Kim, ne zaman, ne yaptı" },
    ],
  },
  {
    title: "Sunucu yönetimi",
    icon: Server,
    items: [
      { to: "/server", label: "Sunucu durumu", hint: "İşlemci, bellek, ağ ve disk grafikleri" },
      { to: "/updates", label: "Sistem güncellemeleri", hint: "Bekleyen paketler ve yeniden başlatma" },
      { to: "/components", label: "Bileşenler", hint: "nginx, PHP, veritabanı, posta kurulumu" },
      { to: "/services", label: "Servisler", hint: "Başlat, durdur, yeniden başlat" },
      { to: "/processes", label: "Süreçler", hint: "Çalışan süreçler ve kaynak kullanımı" },
      { to: "/terminal", label: "Terminal", hint: "Tarayıcıdan sunucu kabuğu" },
      { to: "/settings#ip-adresleri", label: "IP adresleri", hint: "Sunucu adresleri ve hesaplara ayırma" },
      { to: "/settings#web-sunucusu", label: "Web sunucusu", hint: "HTTP/3 (QUIC)" },
    ],
  },
  {
    title: "Uygulamalar ve veritabanları",
    icon: Boxes,
    items: [
      { to: "/php", label: "PHP ayarları", hint: "Sürüm başına varsayılanlar ve eklentiler" },
      { to: "/docker", label: "Docker", hint: "Konteynerler, imajlar ve yığınlar" },
      { to: "/redis", label: "Redis", hint: "Bellek, anahtarlar ve hesap örnekleri" },
      { to: "/databases", label: "Veritabanı sunucuları", hint: "MariaDB, PostgreSQL ve yavaş sorgular" },
    ],
  },
  {
    title: "Posta",
    icon: Mail,
    items: [
      { to: "/mail?sekme=istenmeyen-posta", label: "İstenmeyen posta süzgeci", hint: "Eşikler, beyaz ve kara liste" },
      { to: "/mail?sekme=kuyruk", label: "Posta kuyruğu", hint: "Bekleyen ve teslim edilemeyen iletiler" },
      { to: "/mail?sekme=gonderim-siniri", label: "Gönderim sınırı", hint: "Kutu başına saatlik alıcı sınırı" },
      { to: "/mail?sekme=etkinlik", label: "Posta etkinliği", hint: "Son iletiler ve süzgeç kararları" },
      { to: "/settings#eposta-gonderimi", label: "Bildirim e-postası (SMTP)", hint: "Panelin bildirimleri nereden göndereceği" },
    ],
  },
  {
    title: "Yedekleme ve işler",
    icon: Archive,
    items: [
      { to: "/backups", label: "Yedekler", hint: "Yedek yerleri, planlar ve geri yükleme" },
      { to: "/jobs", label: "Görevler", hint: "Süren ve biten arka plan işleri" },
      { to: "/cron", label: "Zamanlı görevler", hint: "Hesapların cron işleri" },
      { to: "/logs", label: "Günlükler", hint: "Site ve servis kayıtları" },
    ],
  },
  {
    title: "Panel",
    icon: Settings,
    items: [
      { to: "/settings#panel-surumu", label: "Panel sürümü", hint: "Güncelleme denetimi ve kurulum" },
      { to: "/settings#marka", label: "Marka", hint: "Panelin adı, logosu ve giriş metni" },
      { to: "/users", label: "Kullanıcılar ve paketler", hint: "Hesaplar, bayiler ve hizmet paketleri" },
      { to: "/account", label: "Bildirimler ve web kancaları", hint: "Olaylarda e-posta, Telegram ya da HTTP isteği" },
      { to: "/dns", label: "DNS kümesi", hint: "İkincil ad sunucuları" },
    ],
  },
];

export function ToolsPage() {
  const [filter, setFilter] = useState("");
  const needle = filter.trim().toLocaleLowerCase("tr");
  const visible = groups
    .map((group) => ({
      ...group,
      items: group.items.filter((item) => !needle || `${t(item.label)} ${t(item.hint)} ${t(group.title)}`.toLocaleLowerCase("tr").includes(needle)),
    }))
    .filter((group) => group.items.length > 0);

  return (
    <div>
      <PageHeader
        title={t("Araçlar ve ayarlar")}
        description={t("Sunucunun ve panelin yönetimi tek yerde. Sık kullandıklarınıza Ctrl K ile her sayfadan ulaşabilirsiniz.")}
        actions={<SearchInput placeholder={t("Araç ya da ayar ara")} aria-label={t("Araç ya da ayar ara")} value={filter} onChange={(event) => setFilter(event.target.value)} autoFocus />}
      />
      {visible.length === 0 ? (
        <EmptyState title={t("Aramanıza uyan araç yok")} description={t("Başka bir sözcük deneyin ya da aramayı temizleyin.")} />
      ) : (
        <div className="grid gap-x-8 gap-y-9 sm:grid-cols-2 xl:grid-cols-3">
          {visible.map((group) => (
            <section key={group.title} aria-label={t(group.title)}>
              <h2 className="mb-2 flex items-center gap-3 text-[16px] font-semibold tracking-[-0.01em]">
                <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-accent-soft text-accent">
                  <group.icon aria-hidden className="size-[18px]" />
                </span>
                {t(group.title)}
              </h2>
              <ul className="flex flex-col">
                {group.items.map((item) => (
                  <li key={`${item.to}-${item.label}`}>
                    <Link to={item.to} className="group -mx-2 flex flex-col rounded-lg px-2 py-1.5 transition-colors hover:bg-accent-soft/50 sm:ml-10">
                      <span className="font-medium text-accent group-hover:underline">{t(item.label)}</span>
                      <span className="text-[13px] text-ink-3">{t(item.hint)}</span>
                    </Link>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}
