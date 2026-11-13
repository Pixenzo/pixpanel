import type { FormEvent } from "react";
import { Archive, HeartPulse, ShieldCheck } from "lucide-react";
import { useState } from "react";
import { Button, Input } from "../components/ui";
import { ApiError, api, errorMessage } from "../lib/api";
import { BrandLogo, SupportLink } from "../components/Brand";
import { brandName, useBrandTitle, useBranding } from "../lib/branding";
import { browserLanguage, t } from "../lib/i18n";
import { useSession, useTheme } from "../lib/session";

// Marka panelindeki üç satır: panelin yöneticinin yerine yaptığı işler.
const loginHighlights = [
  { icon: ShieldCheck, title: "Sertifikalar kendiliğinden yenilenir", text: "Süresi dolan SSL sertifikası kalmaz." },
  { icon: HeartPulse, title: "Sağlık denetimi sürekli çalışır", text: "Çöken servis yeniden başlatılır, sorun size bildirilir." },
  { icon: Archive, title: "Yedekler plana göre alınır", text: "Siteler, veritabanları ve postalar tek tıkla geri gelir." },
];

export function LoginPage() {
  const { refresh } = useSession();
  useTheme();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  // İki adımlı doğrulama istenirse ikinci adıma geçilir; kullanıcı adı ve parola bellekte kalır.
  const [needsCode, setNeedsCode] = useState(false);
  const [useRecovery, setUseRecovery] = useState(false);
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      // Hesabın dili henüz belirlenmemişse sunucu ilk girişte tarayıcının dilini kaydeder.
      const language = browserLanguage();
      await api.post("/auth/login", needsCode ? { username, password, code: code.trim(), language } : { username, password, language });
      await refresh();
    } catch (failure) {
      if (failure instanceof ApiError && failure.code === "two_factor_required") {
        // Parola doğru; şimdi doğrulama kodu isteniyor.
        setNeedsCode(true);
        setError(null);
      } else {
        setError(errorMessage(failure));
      }
      setBusy(false);
    }
  }

  function backToCredentials() {
    setNeedsCode(false);
    setUseRecovery(false);
    setCode("");
    setError(null);
  }

  function toggleRecovery() {
    setUseRecovery((current) => !current);
    setCode("");
    setError(null);
  }

  const branding = useBranding();
  useBrandTitle(branding);

  return (
    <div className="grid min-h-dvh lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
      {/* Marka paneli: yalnız geniş ekranda; telefonda form tek başına durur. */}
      <aside className="relative hidden flex-col justify-between overflow-hidden bg-rail px-12 py-10 text-rail-ink lg:flex xl:px-16">
        <div aria-hidden className="pointer-events-none absolute -right-24 -bottom-24 grid grid-cols-2 gap-6 opacity-[0.07]">
          <span className="size-56 rounded-[2.5rem] bg-rail-ink" />
          <span className="size-56 rounded-[2.5rem] bg-rail-accent" />
          <span className="size-56 rounded-[2.5rem] bg-rail-ink" />
          <span className="size-56 rounded-[2.5rem] bg-rail-ink" />
        </div>
        <div className="flex items-center gap-3">
          <BrandLogo className="size-8" onDark />
          <span className="text-[19px] font-semibold tracking-[-0.01em]">{brandName(branding)}</span>
        </div>
        <div className="relative max-w-[30rem]">
          <p className="text-[34px] leading-[1.15] font-semibold tracking-[-0.025em] text-balance">{t("Sunucunuz kendi kendine bakar; siz sitelerinizle ilgilenin.")}</p>
          <ul className="mt-8 flex flex-col gap-4 text-[15px]">
            {loginHighlights.map((item) => (
              <li key={item.title} className="flex gap-3.5">
                <item.icon aria-hidden className="mt-0.5 size-5 shrink-0 text-rail-accent" />
                <span>
                  <span className="font-medium">{t(item.title)}</span>
                  <span className="block opacity-70">{t(item.text)}</span>
                </span>
              </li>
            ))}
          </ul>
        </div>
        <p className="relative font-mono text-[13px] opacity-60">{window.location.hostname}</p>
      </aside>
      <main className="grid place-items-center px-4 py-10 sm:px-6">
      <div className="w-full max-w-[26rem]">
        <div className="mb-8 flex items-center gap-4 lg:hidden">
          <BrandLogo className="size-11" />
          <div className="min-w-0">
            <p className="text-[27px] leading-[1.1] font-semibold tracking-[-0.02em] break-words">{brandName(branding)}</p>
            <p className="mt-1 font-mono text-[13.5px] text-ink-3">{window.location.hostname}</p>
          </div>
        </div>

        <div className="mb-7 hidden lg:block">
          <h1 className="text-[27px] leading-[1.1] font-semibold tracking-[-0.02em]">{t("Oturum açın")}</h1>
          <p className="mt-2 text-ink-2">{t("Panele girmek için hesap bilgilerinizi yazın.")}</p>
        </div>

        <form onSubmit={submit} className="flex flex-col gap-5 rounded-2xl border border-line bg-surface p-6 shadow-card sm:p-8">
          {needsCode ? (
            <>
              <p className="text-ink-2">
                {useRecovery
                  ? t("Kurtarma kodlarınızdan birini girin. Her kod yalnızca bir kez kullanılabilir.")
                  : t("Doğrulama uygulamanızdaki 6 haneli kodu girin.")}
              </p>
              <Input
                label={useRecovery ? t("Kurtarma kodu") : t("Doğrulama kodu")}
                value={code}
                onChange={(event) => setCode(event.target.value)}
                hint={useRecovery ? t("Biçim: xxxxx-xxxxx") : undefined}
                placeholder={useRecovery ? "xxxxx-xxxxx" : "123456"}
                inputMode={useRecovery ? "text" : "numeric"}
                autoComplete="one-time-code"
                autoCapitalize="none"
                spellCheck={false}
                className="font-mono tracking-wider"
                autoFocus
                required
              />
            </>
          ) : (
            <>
              <Input
                label={t("Kullanıcı adı")}
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                autoComplete="username"
                autoCapitalize="none"
                spellCheck={false}
                autoFocus
                required
              />
              <Input
                label={t("Parola")}
                type="password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                autoComplete="current-password"
                required
              />
            </>
          )}
          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-bad">
              {error}
            </p>
          )}
          <Button type="submit" variant="primary" loading={busy} className="mt-1">
            {needsCode ? t("Doğrula") : t("Oturum aç")}
          </Button>
          {needsCode && (
            <div className="flex items-center justify-between gap-3 text-[13.5px]">
              <button type="button" onClick={toggleRecovery} className="text-accent hover:underline">
                {useRecovery ? t("Uygulama kodu kullan") : t("Kurtarma kodu kullan")}
              </button>
              <button type="button" onClick={backToCredentials} className="text-ink-2 hover:text-ink hover:underline">
                {t("Geri dön")}
              </button>
            </div>
          )}
        </form>
        {branding.login_message !== "" && <p className="mt-4 text-center text-[13.5px] whitespace-pre-line text-ink-2">{branding.login_message}</p>}
        <SupportLink className="mt-2 block text-center text-[13.5px] text-accent hover:underline" />

        <p className="mt-4 text-[13px] text-ink-3">
          {t("Parolanızı unuttuysanız sunucuda şu komutla sıfırlayabilirsiniz:")}{" "}
          <code className="font-mono text-ink-2">pixpanel reset-password {username || "admin"}</code>
        </p>
      </div>
      </main>
    </div>
  );
}
