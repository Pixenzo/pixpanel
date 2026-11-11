// Ayarlar sayfasındaki "Marka" bölümü: panelin adı, logosu ve giriş sayfası metni.

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

function BrandingForm({ initial }: { initial: Branding }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const fileInput = useRef<HTMLInputElement>(null);
  const [name, setName] = useState(initial.name);
  const [logo, setLogo] = useState(initial.logo);
  const [loginMessage, setLoginMessage] = useState(initial.login_message);
  const [logoError, setLogoError] = useState<string | null>(null);

  const dirty = name !== initial.name || logo !== initial.logo || loginMessage !== initial.login_message;

  const save = useMutation({
    mutationFn: () => api.put<Branding>("/branding", { name, logo, login_message: loginMessage }),
    onSuccess: (saved) => {
      queryClient.setQueryData(["branding"], saved);
      feedback.success(t("Marka ayarları kaydedildi"));
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

      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      <div>
        <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty}>
          {t("Değişiklikleri kaydet")}
        </Button>
      </div>
    </form>
  );
}

export function BrandingSettings() {
  const branding = useBranding();
  return (
    <Panel title={t("Marka")}>
      <p className="mb-4 max-w-[70ch] text-ink-2">{t("Paneli kendi adınız ve logonuzla sunun; müşterileriniz giriş sayfasında ve panelin içinde sizin markanızı görür.")}</p>
      <BrandingForm key={JSON.stringify(branding)} initial={branding} />
    </Panel>
  );
}
