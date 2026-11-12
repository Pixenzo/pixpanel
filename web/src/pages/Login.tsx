import type { FormEvent } from "react";
import { useState } from "react";
import { Button, Input } from "../components/ui";
import { ApiError, api, errorMessage } from "../lib/api";
import { BrandLogo, SupportLink } from "../components/Brand";
import { brandName, useBrandTitle, useBranding } from "../lib/branding";
import { t } from "../lib/i18n";
import { useSession, useTheme } from "../lib/session";

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
      await api.post("/auth/login", needsCode ? { username, password, code: code.trim() } : { username, password });
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
    <main className="grid min-h-dvh place-items-center px-4 sm:px-6 py-10">
      <div className="w-full max-w-[26rem]">
        <div className="mb-8 flex items-center gap-4">
          <BrandLogo className="size-11" />
          <div className="min-w-0">
            <h1 className="text-[27px] leading-[1.1] font-semibold tracking-[-0.02em] break-words">{brandName(branding)}</h1>
            <p className="mt-1 font-mono text-[13.5px] text-ink-3">{window.location.hostname}</p>
          </div>
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
  );
}
