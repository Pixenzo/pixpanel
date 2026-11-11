// Bildirim ayarları: hangi olaylarda, hangi kanaldan (e-posta, Telegram) haber verileceği.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useState } from "react";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Button, Checkbox, ErrorBlock, Input, LoadingBlock, Panel } from "./ui";

interface NotificationEvent {
  id: string;
  label: string;
}

interface NotificationSettingsData {
  email_enabled: boolean;
  email_to: string;
  telegram_enabled: boolean;
  telegram_token_set: boolean;
  telegram_chat_id: string;
  events: string[];
  available_events: NotificationEvent[];
  smtp_configured: boolean;
}

type Channel = "email" | "telegram";

function sameSet(a: string[], b: string[]): boolean {
  return a.length === b.length && [...a].sort().join("\n") === [...b].sort().join("\n");
}

function NotificationForm({ initial }: { initial: NotificationSettingsData }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [events, setEvents] = useState(initial.events);
  const [emailEnabled, setEmailEnabled] = useState(initial.email_enabled);
  const [emailTo, setEmailTo] = useState(initial.email_to);
  const [telegramEnabled, setTelegramEnabled] = useState(initial.telegram_enabled);
  const [telegramToken, setTelegramToken] = useState("");
  const [telegramChatId, setTelegramChatId] = useState(initial.telegram_chat_id);

  const dirty =
    !sameSet(events, initial.events) ||
    emailEnabled !== initial.email_enabled ||
    emailTo !== initial.email_to ||
    telegramEnabled !== initial.telegram_enabled ||
    telegramChatId !== initial.telegram_chat_id ||
    telegramToken !== "";

  const save = useMutation({
    mutationFn: () =>
      api.put<NotificationSettingsData>("/notifications", {
        email_enabled: emailEnabled,
        email_to: emailTo,
        telegram_enabled: telegramEnabled,
        telegram_chat_id: telegramChatId,
        events,
        // Anahtar yalnızca yazıldıysa gönderilir; aksi hâlde kayıtlı olan korunur.
        ...(telegramToken !== "" ? { telegram_token: telegramToken } : {}),
      }),
    onSuccess: (saved) => {
      queryClient.setQueryData(["notifications"], saved);
      feedback.success(t("Bildirim ayarları kaydedildi"));
    },
  });

  const test = useMutation({
    mutationFn: (channel: Channel) => api.post("/notifications/test", { channel }),
    onSuccess: () => feedback.success(t("Deneme bildirimi gönderildi")),
    onError: (error) => feedback.error(error),
  });

  function toggleEvent(id: string, checked: boolean) {
    setEvents((current) => (checked ? [...current.filter((item) => item !== id), id] : current.filter((item) => item !== id)));
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  // Deneme, kaydedilmiş ayarlarla yapılır; bu yüzden bekleyen değişiklik varsa kapalıdır.
  const emailTestable = !dirty && initial.email_enabled && initial.email_to !== "" && initial.smtp_configured;
  const telegramTestable = !dirty && initial.telegram_enabled && initial.telegram_token_set && initial.telegram_chat_id !== "";

  return (
    <form onSubmit={submit} className="flex max-w-xl flex-col gap-6">
      <fieldset className="flex flex-col gap-3">
        <legend className="mb-2 font-semibold">{t("Hangi olaylarda haber verilsin?")}</legend>
        {initial.available_events.map((item) => (
          <Checkbox key={item.id} label={t(item.label)} checked={events.includes(item.id)} onChange={(e) => toggleEvent(item.id, e.target.checked)} />
        ))}
      </fieldset>

      <fieldset className="flex flex-col gap-3 border-t border-line pt-5">
        <legend className="mb-2 font-semibold">{t("E-posta")}</legend>
        {!initial.smtp_configured && (
          <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
            {t("Sunucu yöneticisi henüz e-posta gönderimini yapılandırmadı; ayarlar yapılana kadar e-posta bildirimi gönderilemez.")}
          </p>
        )}
        <Checkbox label={t("E-posta ile haber ver")} checked={emailEnabled} onChange={(e) => setEmailEnabled(e.target.checked)} />
        <Input
          label={t("E-posta adresi")}
          type="email"
          value={emailTo}
          onChange={(e) => setEmailTo(e.target.value)}
          placeholder="siz@ornek.com"
          autoComplete="email"
          spellCheck={false}
        />
      </fieldset>

      <fieldset className="flex flex-col gap-3 border-t border-line pt-5">
        <legend className="mb-2 font-semibold">{t("Telegram")}</legend>
        <Checkbox label={t("Telegram ile haber ver")} checked={telegramEnabled} onChange={(e) => setTelegramEnabled(e.target.checked)} />
        <div className="grid gap-4 sm:grid-cols-2">
          <Input
            label={t("Bot anahtarı (token)")}
            type="password"
            value={telegramToken}
            onChange={(e) => setTelegramToken(e.target.value)}
            placeholder={initial.telegram_token_set ? t("Kayıtlı anahtar korunuyor") : undefined}
            autoComplete="off"
            spellCheck={false}
          />
          <Input
            label={t("Sohbet kimliği (chat id)")}
            value={telegramChatId}
            onChange={(e) => setTelegramChatId(e.target.value)}
            placeholder="123456789"
            autoComplete="off"
            spellCheck={false}
            className="font-mono text-[13.5px]"
          />
        </div>
        <div className="rounded-md border border-line bg-sunken px-3 py-2.5 text-[13.5px] text-ink-2">
          <p className="mb-1 font-medium text-ink">{t("Nasıl ayarlanır?")}</p>
          <ol className="list-decimal space-y-0.5 pl-5">
            <li>{t("{bot} ile bir bot oluşturun ve verdiği anahtarı (token) kopyalayın.", { bot: "@BotFather" })}</li>
            <li>{t("Botunuza herhangi bir mesaj gönderin (ya da bir gruba ekleyin).")}</li>
            <li>
              {t("Sohbet kimliğinizi öğrenin, örneğin {bot} ile. Kimlik sayıdır; gruplarda {sign} ile başlar.", { bot: "@userinfobot", sign: "-" })}
            </li>
          </ol>
        </div>
      </fieldset>

      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}

      <div className="flex flex-wrap items-center gap-2 border-t border-line pt-5">
        <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty}>
          {t("Değişiklikleri kaydet")}
        </Button>
        <Button loading={test.isPending && test.variables === "email"} disabled={!emailTestable} onClick={() => test.mutate("email")}>
          {t("Deneme e-postası gönder")}
        </Button>
        <Button loading={test.isPending && test.variables === "telegram"} disabled={!telegramTestable} onClick={() => test.mutate("telegram")}>
          {t("Telegram denemesi gönder")}
        </Button>
        {dirty && <span className="text-[13px] text-ink-3">{t("Denemeler kayıtlı ayarlarla yapılır; önce kaydedin.")}</span>}
      </div>
    </form>
  );
}

/** Hesabım sayfasına gömülen, kendi kendine yeten bildirim bölümü. */
export function NotificationSettings() {
  const settings = useQuery({ queryKey: ["notifications"], queryFn: () => api.get<NotificationSettingsData>("/notifications") });

  return (
    <Panel title={t("Bildirimler")}>
      {settings.isPending && <LoadingBlock />}
      {settings.isError && <ErrorBlock message={errorMessage(settings.error)} onRetry={() => void settings.refetch()} />}
      {settings.data && <NotificationForm key={JSON.stringify(settings.data)} initial={settings.data} />}
    </Panel>
  );
}
