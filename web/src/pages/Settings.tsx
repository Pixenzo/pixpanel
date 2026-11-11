// Panelin genel ayarları: e-posta (SMTP) gönderimi.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { IpAddresses } from "../components/IpAddresses";
import { PanelCertificate } from "../components/PanelCertificate";
import { Button, ErrorBlock, Input, LoadingBlock, PageHeader, Panel, Select } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { PanelUpdate } from "./PanelUpdate";

type SmtpSecurity = "starttls" | "tls" | "none";

interface SmtpSettings {
  host: string;
  port: number;
  security: SmtpSecurity;
  username: string;
  password_set: boolean;
  from: string;
}

/** Bağlantı güvenliğine göre yaygın kullanılan kapı. */
const defaultPorts: Record<SmtpSecurity, number> = {
  starttls: 587,
  tls: 465,
  none: 25,
};

function SmtpForm({ initial }: { initial: SmtpSettings }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [host, setHost] = useState(initial.host);
  const [port, setPort] = useState(initial.port);
  const [security, setSecurity] = useState<SmtpSecurity>(initial.security);
  const [username, setUsername] = useState(initial.username);
  const [password, setPassword] = useState("");
  const [from, setFrom] = useState(initial.from);
  const [testTo, setTestTo] = useState("");

  const dirty =
    host !== initial.host ||
    port !== initial.port ||
    security !== initial.security ||
    username !== initial.username ||
    from !== initial.from ||
    password !== "";

  const save = useMutation({
    mutationFn: () =>
      api.put<SmtpSettings>("/notifications/smtp", {
        host,
        port,
        security,
        username,
        from,
        // Parola yalnızca yazıldıysa gönderilir; aksi hâlde kayıtlı olan korunur.
        ...(password !== "" ? { password } : {}),
      }),
    onSuccess: (saved) => {
      queryClient.setQueryData(["smtp"], saved);
      feedback.success(t("E-posta ayarları kaydedildi"));
    },
  });

  const test = useMutation({
    mutationFn: () => api.post("/notifications/smtp/test", { to: testTo.trim() }),
    onSuccess: () => feedback.success(t("Deneme e-postası gönderildi")),
  });

  /** Güvenlik seçimi değişince, kapı hâlâ önceki varsayılandaysa yenisini öner. */
  function changeSecurity(next: SmtpSecurity) {
    if (port === defaultPorts[security]) setPort(defaultPorts[next]);
    setSecurity(next);
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  function submitTest(event: FormEvent) {
    event.preventDefault();
    if (!dirty) test.mutate();
  }

  const testBlocked = dirty || initial.host === "";

  return (
    <div className="flex max-w-xl flex-col gap-6">
      <form onSubmit={submit} className="flex flex-col gap-4">
        <div className="grid gap-4 sm:grid-cols-[1fr_8rem]">
          <Input
            label={t("Sunucu")}
            value={host}
            onChange={(e) => setHost(e.target.value)}
            placeholder="smtp.ornek.com"
            spellCheck={false}
            autoComplete="off"
            className="font-mono text-[13px]"
          />
          <Input label={t("Kapı (port)")} type="number" min={1} max={65535} value={port} onChange={(e) => setPort(Number(e.target.value))} />
        </div>
        <Select label={t("Bağlantı güvenliği")} value={security} onChange={(e) => changeSecurity(e.target.value as SmtpSecurity)}>
          <option value="starttls">{t("STARTTLS (önerilen, port 587)")}</option>
          <option value="tls">{t("TLS (port 465)")}</option>
          <option value="none">{t("Şifresiz")}</option>
        </Select>
        <div className="grid gap-4 sm:grid-cols-2">
          <Input label={t("Kullanıcı adı")} value={username} onChange={(e) => setUsername(e.target.value)} autoComplete="off" spellCheck={false} />
          <Input
            label={t("Parola")}
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            placeholder={initial.password_set ? t("Kayıtlı parola korunuyor") : undefined}
            autoComplete="new-password"
          />
        </div>
        <Input
          label={t("Gönderen adresi")}
          value={from}
          onChange={(e) => setFrom(e.target.value)}
          hint="PixPanel <panel@ornek.com>"
          spellCheck={false}
        />
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </div>
      </form>

      <form onSubmit={submitTest} className="flex flex-col gap-3 border-t border-line pt-5">
        <h3 className="font-semibold">{t("Deneme e-postası gönder")}</h3>
        <div className="flex flex-wrap items-end gap-2">
          <div className="min-w-56 flex-1">
            <Input
              label={t("Alıcı adres")}
              type="email"
              value={testTo}
              onChange={(e) => {
                setTestTo(e.target.value);
                if (test.isError) test.reset();
              }}
              placeholder="siz@ornek.com"
              required
            />
          </div>
          <Button type="submit" loading={test.isPending} disabled={testBlocked || testTo.trim() === ""}>
            {t("Gönder")}
          </Button>
        </div>
        <p className="text-[12.5px] text-ink-3">
          {dirty
            ? t("Kaydedilmemiş değişiklikler var; denemek için önce kaydedin.")
            : initial.host === ""
              ? t("Önce SMTP sunucusunu girip kaydedin.")
              : t("Deneme, kaydedilmiş ayarlarla yapılır.")}
        </p>
        {test.isError && <ErrorBlock message={errorMessage(test.error)} />}
      </form>
    </div>
  );
}

export function SettingsPage() {
  const smtp = useQuery({ queryKey: ["smtp"], queryFn: () => api.get<SmtpSettings>("/notifications/smtp") });

  return (
    <>
      <PageHeader title={t("Ayarlar")} description={t("Panelin genel ayarları.")} />
      <PanelCertificate />
      <PanelUpdate />
      <IpAddresses />
      <Panel title={t("E-posta gönderimi (SMTP)")}>
        <p className="mb-4 max-w-[70ch] text-ink-2">
          {t("Panel, bildirim e-postalarını (başarısız dağıtımlar, sertifika sorunları gibi) bu sunucu üzerinden gönderir.")}
        </p>
        {smtp.isPending && <LoadingBlock />}
        {smtp.isError && <ErrorBlock message={errorMessage(smtp.error)} onRetry={() => void smtp.refetch()} />}
        {smtp.data && <SmtpForm key={JSON.stringify(smtp.data)} initial={smtp.data} />}
      </Panel>
    </>
  );
}
