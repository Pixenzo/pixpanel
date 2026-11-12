// Posta kutusunu eski sunucudan içe aktarma: pencere IMAP bağlantı bilgilerini sorar, ardından
// aktarma görevinin çıktısını canlı gösterir. Eski sunucudaki iletilere dokunulmaz.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { Button, Checkbox, ErrorBlock, Input, Modal, Tabs } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

type Security = "ssl" | "starttls";

export function MailImportModal({ mailbox, onClose }: { mailbox: { id: number; address: string }; onClose: () => void }) {
  const queryClient = useQueryClient();
  const [host, setHost] = useState("");
  const [security, setSecurity] = useState<Security>("ssl");
  const [port, setPort] = useState("");
  const [username, setUsername] = useState(mailbox.address);
  const [password, setPassword] = useState("");
  const [verify, setVerify] = useState(true);
  const [jobId, setJobId] = useState<number | null>(null);
  const [result, setResult] = useState<string | null>(null);

  const start = useMutation({
    mutationFn: () =>
      api.post<{ job_id: number }>(`/mail/mailboxes/${mailbox.id}/import`, {
        host: host.trim(),
        security,
        ...(port.trim() ? { port: Number(port) } : {}),
        username: username.trim(),
        password,
        verify_tls: verify,
      }),
    onSuccess: (response) => setJobId(response.job_id),
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    start.mutate();
  }

  function finished(status: string) {
    setResult(status);
    void queryClient.invalidateQueries({ queryKey: ["mail"] });
  }

  if (jobId !== null) {
    return (
      <Modal
        open
        onClose={onClose}
        wide
        title={result === "success" ? t("İletiler aktarıldı") : t("İletiler aktarılıyor")}
        footer={<Button onClick={onClose}>{result === null ? t("Arka planda sürsün") : t("Kapat")}</Button>}
      >
        <JobLog jobId={jobId} onDone={finished} />
        {result === "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">
            {t("Alan adının MX kaydını bu sunucuya çevirdikten sonra aktarmayı bir kez daha çalıştırın; arada eski sunucuya düşen iletiler de gelir.")}
          </p>
        )}
        {result !== null && result !== "success" && (
          <p className="mt-3 text-[13.5px] text-ink-2">{t("Aktarma tamamlanamadı. O ana kadar gelen iletiler kutuda kalır; yeniden başlatırsanız kaldığı yerden sürer.")}</p>
        )}
      </Modal>
    );
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={t("{address} kutusuna ileti aktar", { address: mailbox.address })}
      description={t("Eski sağlayıcıdaki posta kutusunun bütün klasörleri ve iletileri IMAP ile buraya kopyalanır. Eski kutudaki iletilere dokunulmaz; yeniden çalıştırınca yalnızca yeni gelenler alınır.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="mail-import-form" loading={start.isPending}>
            {t("Aktarmayı başlat")}
          </Button>
        </>
      }
    >
      <form id="mail-import-form" onSubmit={submit} className="flex flex-col gap-5">
        <div className="grid grid-cols-[1fr_6.5rem] gap-3">
          <Input
            label={t("Eski posta sunucusu (IMAP)")}
            value={host}
            onChange={(event) => setHost(event.target.value)}
            placeholder="imap.ornek.com"
            className="font-mono text-[13.5px]"
            spellCheck={false}
            autoComplete="off"
            autoFocus
            required
          />
          <Input
            label={t("Port")}
            value={port}
            onChange={(event) => setPort(event.target.value)}
            placeholder={security === "ssl" ? "993" : "143"}
            inputMode="numeric"
            className="font-mono text-[13.5px]"
          />
        </div>
        <Tabs<Security>
          label={t("Bağlantı güvenliği")}
          tabs={[
            { id: "ssl", label: "SSL/TLS" },
            { id: "starttls", label: "STARTTLS" },
          ]}
          value={security}
          onChange={setSecurity}
        />
        <Input
          label={t("Eski kutunun kullanıcı adı")}
          value={username}
          onChange={(event) => setUsername(event.target.value)}
          hint={t("Çoğu sağlayıcıda e-posta adresinin kendisidir.")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
          required
        />
        <Input
          label={t("Eski kutunun parolası")}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          hint={t("Yalnızca bu aktarma için kullanılır; panelde saklanmaz. Gmail ve Outlook gibi sağlayıcılar “uygulama parolası” ister.")}
          autoComplete="off"
          required
        />
        <Checkbox
          label={t("Sunucunun sertifikasını doğrula")}
          hint={t("Eski sunucu kendinden imzalı sertifika kullanıyorsa kapatın.")}
          checked={verify}
          onChange={(event) => setVerify(event.target.checked)}
        />
        {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      </form>
    </Modal>
  );
}
