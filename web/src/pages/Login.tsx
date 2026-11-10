import type { FormEvent } from "react";
import { useState } from "react";
import { Button, Input } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useSession, useTheme } from "../lib/session";

export function LoginPage() {
  const { refresh } = useSession();
  useTheme();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.post("/auth/login", { username, password });
      await refresh();
    } catch (failure) {
      setError(errorMessage(failure));
      setBusy(false);
    }
  }

  return (
    <main className="grid min-h-dvh place-items-center px-4 py-10">
      <div className="w-full max-w-sm">
        <div className="mb-7 flex items-center gap-3">
          <svg viewBox="0 0 32 32" className="size-9" aria-hidden>
            <rect x="3" y="9" width="26" height="14" rx="7" className="fill-accent" />
            <rect x="16" y="9" width="13" height="14" rx="7" className="fill-ink" />
            <circle cx="10" cy="16" r="2.2" className="fill-bg" />
          </svg>
          <div>
            <h1 className="text-[22px] leading-tight font-semibold tracking-[-0.01em]">PixPanel</h1>
            <p className="font-mono text-[12.5px] text-ink-3">{window.location.hostname}</p>
          </div>
        </div>

        <form onSubmit={submit} className="flex flex-col gap-4 rounded-xl border border-line bg-surface p-6">
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
          {error && (
            <p role="alert" className="rounded-md bg-bad-soft px-3 py-2 text-bad">
              {error}
            </p>
          )}
          <Button type="submit" variant="primary" loading={busy} className="mt-1">
            {t("Oturum aç")}
          </Button>
        </form>

        <p className="mt-4 text-[12.5px] text-ink-3">
          {t("Parolanızı unuttuysanız sunucuda şu komutla sıfırlayabilirsiniz:")}{" "}
          <code className="font-mono text-ink-2">pixpanel reset-password {username || "admin"}</code>
        </p>
      </div>
    </main>
  );
}
