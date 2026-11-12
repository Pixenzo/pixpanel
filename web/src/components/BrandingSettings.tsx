// Marka formu: Ayarlar'daki sunucu geneli marka ve bayinin kendi markası aynı bileşenden geçer.
// Ad, logo, giriş sayfası metni, destek bağlantısı ve (yalnız bayi markasında) panel alan adı.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ChangeEvent, FormEvent } from "react";
import { useRef, useState } from "react";
import { api, errorMessage } from "../lib/api";
import type { Branding } from "../lib/branding";
import { DEFAULT_NAME, useBranding } from "../lib/branding";
import { t } from "../lib/i18n";
import { BrandMark } from "./Brand";
import { useFeedback } from "./feedback";
import { Button, ErrorBlock, Input, Panel, Textarea } from "./ui";

const LOGO_TYPES = ["image/png", "image/jpeg", "image/webp", "image/svg+xml"];
/// Sunucudaki sınırla uyumlu: base64 kodlaması dosyayı üçte bir büyütür.
const MAX_LOGO_FILE_BYTES = 140 * 1024;

/** Formun değerleri; `panel_domain` yalnızca bayi markasında vardır. */
export interface BrandingValues extends Branding {
  panel_domain: string;
}

export function BrandingForm({
  initial,
  onSubmit,
  onSaved,
  withDomain,
}: {
  initial: BrandingValues;
  /** Değerleri sunucuya yazar. */
  onSubmit: (values: BrandingValues) => Promise<unknown>;
  /** Kayıttan sonra çağrılır (önbellek tazeleme gibi). */
  onSaved: () => void;
  /** Panel alan adı alanını göster (bayi markası). */
  withDomain?: boolean;
}) {
  const feedback = useFeedback();
  const fileInput = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(initial.name);
  const [logo, setLogo] = useState(initial.logo);
  const [loginMessage, setLoginMessage] = useState(initial.login_message);
  const [supportUrl, setSupportUrl] = useState(initial.support_url);
  const [panelDomain, setPanelDomain] = useState(initial.panel_domain);
  const [logoError, setLogoError] = useState<string | null>(null);

  const dirty =
    name !== initial.name ||
    logo !== initial.logo ||
    loginMessage !== initial.login_message ||
    supportUrl !== initial.support_url ||
    (withDomain === true && panelDomain !== initial.panel_domain);

  const save = useMutation({
    mutationFn: () => onSubmit({ name, logo, login_message: loginMessage, support_url: supportUrl, panel_domain: withDomain ? panelDomain : "" }),
    onSuccess: () => {
      feedback.success(t("Marka ayarları kaydedildi"));
      onSaved();
    },
  });

  function chooseLogo(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    if (!LOGO_TYPES.includes(file.type)) {
      setLogoError(t("Logo PNG, JPEG, WebP ya da SVG biçiminde olmalı."));
      return;
    }
    if (file.size > MAX_LOGO_FILE_BYTES) {
      setLogoError(t("Logo çok büyük; 140 KB'ın altında bir görsel seçin."));
      return;
    }
    const reader = new FileReader();
    reader.onload = () => {
      setLogo(String(reader.result));
      setLogoError(null);
    };
    reader.onerror = () => setLogoError(t("Dosya okunamadı."));
    reader.readAsDataURL(file);
  }

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <form onSubmit={submit} className="flex max-w-xl flex-col gap-5">
      <Input
        label={t("Panel adı")}
        value={name}
        onChange={(event) => setName(event.target.value)}
        placeholder={DEFAULT_NAME}
        hint={t("Giriş sayfasında, yan menüde ve tarayıcı sekmesinde görünür. Boş bırakırsanız {name} yazar.", { name: DEFAULT_NAME })}
        maxLength={40}
      />

      <div className="flex flex-col gap-2">
        <p className="text-[13.5px] font-medium text-ink-2">{t("Logo")}</p>
        <div className="flex flex-wrap items-center gap-4">
          <div className="grid size-14 shrink-0 place-items-center rounded-lg border border-line bg-sunken/60">
            {logo === "" ? <BrandMark className="size-7" /> : <img src={logo} alt="" className="max-h-10 max-w-10 object-contain" />}
          </div>
          <div className="flex flex-wrap gap-2">
            <Button onClick={() => fileInput.current?.click()}>{logo === "" ? t("Logo yükle") : t("Logoyu değiştir")}</Button>
            {logo !== "" && (
              <Button variant="ghost" onClick={() => setLogo("")}>
                {t("Logoyu kaldır")}
              </Button>
            )}
          </div>
          <input ref={fileInput} type="file" accept={LOGO_TYPES.join(",")} onChange={chooseLogo} className="sr-only" tabIndex={-1} aria-label={t("Logo dosyası")} />
        </div>
        <p className="text-[13px] text-ink-3">{t("Kare ya da kareye yakın, saydam zeminli bir görsel en iyi sonucu verir. Koyu yan menüde de okunaklı olmasına dikkat edin.")}</p>
        {logoError && <ErrorBlock message={logoError} />}
      </div>

      <Textarea
        label={t("Giriş sayfası metni")}
        value={loginMessage}
        onChange={(event) => setLoginMessage(event.target.value)}
        placeholder={t("Örn. Destek için: destek@ornek.com")}
        hint={t("Giriş formunun altında gösterilir. Herkes görebilir; gizli bilgi yazmayın.")}
        className="min-h-20 font-sans text-[14px]"
        maxLength={300}
      />

      <Input
        label={t("Destek bağlantısı")}
        value={supportUrl}
        onChange={(event) => setSupportUrl(event.target.value)}
        placeholder="https://destek.ornek.com"
        hint={t("İsteğe bağlı. https:// ya da mailto: ile başlayan bir adres; yan menünün altında \"Destek\" bağlantısı olarak görünür.")}
        maxLength={200}
        inputMode="url"
        autoComplete="off"
      />

      {withDomain && (
        <Input
          label={t("Panel alan adı")}
          value={panelDomain}
          onChange={(event) => setPanelDomain(event.target.value)}
          placeholder="panel.ornek.com"
          hint={t("İsteğe bağlı. Müşterileriniz panele bu adresten girerse giriş sayfasında markanız görünür. Alan adının bu sunucuya yönlendirilmesi ve sertifikası ayrıca kurulmalıdır; bir alan adı yalnızca tek bayiye bağlanabilir.")}
          maxLength={253}
          autoComplete="off"
          spellCheck={false}
        />
      )}

      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div>
        <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty}>
          {t("Değişiklikleri kaydet")}
        </Button>
      </div>
    </form>
  );
}

/** Ayarlar sayfasındaki sunucu geneli marka (yalnız yönetici). */
export function BrandingSettings() {
  const queryClient = useQueryClient();
  const branding = useBranding();
  return (
    <Panel title={t("Marka")}>
      <p className="mb-4 max-w-[70ch] text-ink-2">{t("Paneli kendi adınız ve logonuzla sunun; müşterileriniz giriş sayfasında ve panelin içinde sizin markanızı görür.")}</p>
      <BrandingForm
        key={JSON.stringify(branding)}
        initial={{ ...branding, panel_domain: "" }}
        onSubmit={(values) => api.put<Branding>("/branding", { name: values.name, logo: values.logo, login_message: values.login_message, support_url: values.support_url })}
        onSaved={() => void queryClient.invalidateQueries({ queryKey: ["branding"] })}
      />
    </Panel>
  );
}
