// Posta alan adı sayfasının "İstemci ayarları" bölümü: elle kurulum için sunucu adı ve port tablosu
// ile posta istemcilerinin ayarı kendiliğinden bulmasını sağlayan otomatik ayarın (autoconfig,
// autodiscover) durumu.

import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Badge, ErrorBlock, LoadingBlock, Mono, Panel, Status, Table, Td, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import type { MailDomain } from "./Mail";
import { MailOwnNamePanel } from "./MailOwnName";

interface NameStatus {
  name: string;
  /** Aynı adla başka bir site var; sunucu bloğu yazılmadı. */
  conflict: boolean;
  https: boolean;
}

interface ClientSettings {
  hostname: string;
  imap_port: number;
  submission_port: number;
  smtps_port: number;
  /** nginx kurulu mu? Kurulu değilse otomatik ayar adresleri sunulamaz. */
  nginx: boolean;
  names: NameStatus[];
  autoconfig_url: string;
  autodiscover_url: string;
  mobileconfig_url: string;
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-ink-2">{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

export function MailClientSettings({ domain }: { domain: MailDomain }) {
  const settings = useQuery({
    queryKey: ["mail-client-settings", domain.id],
    queryFn: () => api.get<ClientSettings>(`/mail/domains/${domain.id}/client-settings`),
  });
  if (settings.isPending) return <LoadingBlock />;
  if (settings.isError) return <ErrorBlock message={errorMessage(settings.error)} onRetry={() => void settings.refetch()} />;
  const data = settings.data;

  return (
    <div className="flex flex-col gap-5">
      <Panel title={t("Elle kurulum")}>
        <p className="mb-3 max-w-[75ch] text-ink-2">{t("Outlook, Thunderbird ya da telefon uygulamasında hesabı bu bilgilerle tanımlayın.")}</p>
        <Table>
          <thead>
            <tr>
              <Th>{t("Sunucu")}</Th>
              <Th>{t("Sunucu adı")}</Th>
              <Th align="right">{t("Port")}</Th>
              <Th>{t("Güvenlik")}</Th>
            </tr>
          </thead>
          <tbody>
            <tr>
              <Td>{t("Gelen posta (IMAP)")}</Td>
              <Td>
                <Mono>{data.hostname}</Mono>
              </Td>
              <Td align="right" className="tabular">
                {data.imap_port}
              </Td>
              <Td>SSL/TLS</Td>
            </tr>
            <tr>
              <Td>{t("Giden posta (SMTP)")}</Td>
              <Td>
                <Mono>{data.hostname}</Mono>
              </Td>
              <Td align="right" className="tabular">
                {data.submission_port}
              </Td>
              <Td>STARTTLS</Td>
            </tr>
            <tr>
              <Td>{t("Giden posta (SMTP, alternatif)")}</Td>
              <Td>
                <Mono>{data.hostname}</Mono>
              </Td>
              <Td align="right" className="tabular">
                {data.smtps_port}
              </Td>
              <Td>SSL/TLS</Td>
            </tr>
          </tbody>
        </Table>
        <dl className="mt-4 grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <Row label={t("Kullanıcı adı")}>{t("E-posta adresinin tamamı")}</Row>
          <Row label={t("Parola")}>{t("Kutunun parolası")}</Row>
        </dl>
      </Panel>

      <MailOwnNamePanel domain={domain} />

      <Panel title={t("Otomatik ayar")}>
        <p className="mb-4 max-w-[75ch] text-ink-2">
          {t(
            "Thunderbird, K-9 Mail, Outlook ve Apple Mail, e-posta adresini yazınca sunucu ayarlarını bu adreslerden kendiliğinden alır. Çalışması için aşağıdaki adların DNS'te bu sunucuyu göstermesi gerekir (DNS kayıtları bölümünde önerilir).",
          )}
        </p>
        {!data.nginx && <p className="mb-4 rounded-md bg-sunken/70 px-3 py-2.5 text-[13.5px] text-ink-2">{t("Otomatik ayar adresleri için nginx gerekir; önce Bileşenler sayfasından kurun.")}</p>}
        <ul className="mb-4 flex flex-col divide-y divide-line">
          {data.names.map((name) => (
            <li key={name.name} className="flex flex-wrap items-center gap-x-3 gap-y-1 py-2.5 first:pt-0 last:pb-0">
              <Mono className="text-[13.5px]">{name.name}</Mono>
              {name.conflict ? (
                <Status tone="warn">{t("Aynı adla bir site var; otomatik ayar sunulmuyor")}</Status>
              ) : data.nginx ? (
                <>
                  <Status tone="ok">{t("Sunuluyor (HTTP)")}</Status>
                  {name.https ? <Badge tone="ok">HTTPS</Badge> : <Badge>{t("HTTPS için sertifika yok")}</Badge>}
                </>
              ) : (
                <Status tone="idle">{t("Sunulmuyor")}</Status>
              )}
            </li>
          ))}
        </ul>
        <dl className="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2">
          <Row label="Thunderbird / K-9">
            <Mono className="break-all text-[13px]">{data.autoconfig_url}</Mono>
          </Row>
          <Row label="Outlook">
            <Mono className="break-all text-[13px]">{data.autodiscover_url}</Mono>
          </Row>
          <Row label={t("Apple (profil)")}>
            <Mono className="break-all text-[13px]">{data.mobileconfig_url}</Mono>
          </Row>
        </dl>
        <p className="mt-3 max-w-[75ch] text-[13px] text-ink-3">
          {t("Outlook yalnızca HTTPS ile çalışır: autodiscover adı için geçerli bir sertifika gerekir (örneğin alan adının joker sertifikası).")}
        </p>
      </Panel>
    </div>
  );
}
