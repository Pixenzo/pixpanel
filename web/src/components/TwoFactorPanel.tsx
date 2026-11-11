// Hesabım sayfasındaki iki adımlı doğrulama (TOTP) paneli ve kurulum sihirbazı.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, Download } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Button, Checkbox, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Status } from "./ui";

interface TwoFactorStatus {
  enabled: boolean;
  recovery_codes_left: number;
}

interface SetupData {
  /** Elle girilebilen base32 anahtar. */
  secret: string;
  uri: string;
  /** Tam bir SVG belgesi. */
  qr_svg: string;
}

interface RecoveryCodesResult {
  recovery_codes: string[];
}

/** Kurtarma kodu sayısı bu değere ya da altına inince uyarı verilir. */
const LOW_CODES_THRESHOLD = 3;

/** Panoya kopyalar; tarayıcı izin vermezse hata bildirimi gösterir. */
function useCopy() {
  const feedback = useFeedback();
  return async (text: string, message: string) => {
    try {
      await navigator.clipboard.writeText(text);
      feedback.success(message);
    } catch (error) {
      feedback.error(error);
    }
  };
}

// ---- Kurtarma kodları ----

/** Kodları bir kez gösteren pencere; kullanıcı kaydettiğini onaylamadan "Bitir" açılmaz. */
function RecoveryCodesModal({ title, codes, onClose }: { title: string; codes: string[]; onClose: () => void }) {
  const copy = useCopy();
  const [saved, setSaved] = useState(false);

  function download() {
    const body = `${t("PixPanel kurtarma kodları")}\n${t("Her kod yalnızca bir kez kullanılabilir.")}\n\n${codes.join("\n")}\n`;
    const url = URL.createObjectURL(new Blob([body], { type: "text/plain;charset=utf-8" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = "pixpanel-recovery-codes.txt";
    link.click();
    URL.revokeObjectURL(url);
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      footer={
        <Button variant="primary" disabled={!saved} onClick={onClose}>
          {t("Bitir")}
        </Button>
      }
    >
      <div className="flex flex-col gap-5">
        <p role="alert" className="rounded-md bg-warn-soft px-3 py-2 text-warn">
          {t("Bu kodlar yalnızca şimdi gösterilir ve her biri yalnızca bir kez kullanılabilir. Telefonunuzu kaybederseniz hesabınıza bunlarla girebilirsiniz.")}
        </p>
        <ul className="grid grid-cols-2 gap-x-4 gap-y-1.5 rounded-md border border-line bg-sunken p-3">
          {codes.map((code) => (
            <li key={code}>
              <Mono className="text-[13.5px]">{code}</Mono>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap gap-2">
          <Button size="sm" icon={<Copy className="size-3.5" />} onClick={() => void copy(codes.join("\n"), t("Kodlar panoya kopyalandı"))}>
            {t("Kopyala")}
          </Button>
          <Button size="sm" icon={<Download className="size-3.5" />} onClick={download}>
            {t("İndir (.txt)")}
          </Button>
        </div>
        <Checkbox label={t("Kodları güvenli bir yere kaydettim")} checked={saved} onChange={(event) => setSaved(event.target.checked)} />
      </div>
    </Modal>
  );
}

// ---- Parola isteyen küçük pencere ----

function PasswordConfirmModal<T>({
  title,
  description,
  warning,
  confirmLabel,
  danger,
  action,
  onSuccess,
  onClose,
}: {
  title: string;
  description: string;
  warning?: string;
  confirmLabel: string;
  danger?: boolean;
  action: (password: string) => Promise<T>;
  onSuccess: (result: T) => void;
  onClose: () => void;
}) {
  const [password, setPassword] = useState("");
  const run = useMutation({ mutationFn: () => action(password), onSuccess });

  function submit(event: FormEvent) {
    event.preventDefault();
    run.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      title={title}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant={danger ? "danger" : "primary"} type="submit" form="two-factor-confirm" loading={run.isPending}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <form id="two-factor-confirm" onSubmit={submit} className="flex flex-col gap-5">
        <p className="text-ink-2">{description}</p>
        {warning && <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">{warning}</p>}
        <Input
          label={t("Mevcut parola")}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          autoComplete="current-password"
          autoFocus
          required
        />
        {run.isError && <ErrorBlock message={errorMessage(run.error)} />}
      </form>
    </Modal>
  );
}

// ---- Etkinleştirme sihirbazı ----

function EnableWizard({ onClose }: { onClose: () => void }) {
  const queryClient = useQueryClient();
  const copy = useCopy();
  const [setup, setSetup] = useState<SetupData | null>(null);
  const [codes, setCodes] = useState<string[] | null>(null);
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");

  const start = useMutation({
    mutationFn: () => api.post<SetupData>("/auth/2fa/setup", { password }),
    onSuccess: (data) => {
      setSetup(data);
      // Parola bellekte gereğinden uzun kalmasın.
      setPassword("");
    },
  });

  const enable = useMutation({
    mutationFn: () => api.post<RecoveryCodesResult>("/auth/2fa/enable", { code }),
    onSuccess: (result) => {
      setCodes(result.recovery_codes);
      void queryClient.invalidateQueries({ queryKey: ["two-factor"] });
    },
  });

  // 3. adım: kurtarma kodları.
  if (codes) {
    return <RecoveryCodesModal title={t("Kurtarma kodlarınız")} codes={codes} onClose={onClose} />;
  }

  // 2. adım: QR kodu ve doğrulama.
  if (setup) {
    return (
      <Modal
        open
        onClose={onClose}
        title={t("Uygulamayı bağlayın")}
        description={t("2. adım: QR kodunu okutun ve uygulamanın ürettiği kodu girin.")}
        footer={
          <>
            <Button onClick={onClose}>{t("Vazgeç")}</Button>
            <Button variant="primary" type="submit" form="two-factor-enable" loading={enable.isPending} disabled={code.length !== 6}>
              {t("Doğrula ve etkinleştir")}
            </Button>
          </>
        }
      >
        <form
          id="two-factor-enable"
          onSubmit={(event) => {
            event.preventDefault();
            enable.mutate();
          }}
          className="flex flex-col gap-5"
        >
          <div className="flex justify-center">
            {/* Karanlık temada da QR kodu siyah üzerine beyaz kalmalı. */}
            <div className="rounded-md border border-line bg-white p-2">
              <img
                alt={t("İki adımlı doğrulama QR kodu")}
                src={"data:image/svg+xml;utf8," + encodeURIComponent(setup.qr_svg)}
                className="size-44"
              />
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <p className="text-[13.5px] font-medium text-ink-2">{t("Kodu okutamıyorsanız bu anahtarı elle girin")}</p>
            <div className="flex items-center gap-2 rounded-md border border-line bg-sunken p-2.5">
              <code className="min-w-0 flex-1 font-mono text-[13px] break-all">{setup.secret}</code>
              <IconButton label={t("Kopyala")} onClick={() => void copy(setup.secret, t("Anahtar panoya kopyalandı"))}>
                <Copy className="size-4" />
              </IconButton>
            </div>
          </div>
          <Input
            label={t("Doğrulama kodu")}
            value={code}
            onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 6))}
            hint={t("Uygulamada görünen 6 haneli kod")}
            placeholder="123456"
            inputMode="numeric"
            autoComplete="one-time-code"
            className="font-mono tracking-wider"
            autoFocus
            required
          />
          {enable.isError && <ErrorBlock message={errorMessage(enable.error)} />}
        </form>
      </Modal>
    );
  }

  // 1. adım: parola.
  return (
    <Modal
      open
      onClose={onClose}
      title={t("İki adımlı doğrulamayı etkinleştir")}
      description={t("1. adım: kimliğinizi parolanızla doğrulayın.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="two-factor-start" loading={start.isPending}>
            {t("Devam")}
          </Button>
        </>
      }
    >
      <form
        id="two-factor-start"
        onSubmit={(event) => {
          event.preventDefault();
          start.mutate();
        }}
        className="flex flex-col gap-5"
      >
        <Input
          label={t("Mevcut parola")}
          type="password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          autoComplete="current-password"
          autoFocus
          required
        />
        {start.isError && <ErrorBlock message={errorMessage(start.error)} />}
      </form>
    </Modal>
  );
}

// ---- Panel ----

export function TwoFactorPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [enabling, setEnabling] = useState(false);
  const [regenerating, setRegenerating] = useState(false);
  const [disabling, setDisabling] = useState(false);
  const [newCodes, setNewCodes] = useState<string[] | null>(null);

  const status = useQuery({ queryKey: ["two-factor"], queryFn: () => api.get<TwoFactorStatus>("/auth/2fa") });
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["two-factor"] });

  function renderStatus(data: TwoFactorStatus) {
    if (!data.enabled) {
      return (
        <div className="flex max-w-prose flex-col items-start gap-3">
          <p className="text-ink-2">
            {t(
              "Girişte parolanıza ek olarak telefonunuzdaki uygulamanın ürettiği kod istenir. Google Authenticator, Aegis, 1Password gibi uygulamalarla çalışır. Etkinleştirdiğinizde diğer cihazlardaki oturumlarınız kapatılır.",
            )}
          </p>
          <Button variant="primary" onClick={() => setEnabling(true)}>
            {t("Etkinleştir")}
          </Button>
        </div>
      );
    }

    const left = data.recovery_codes_left;
    const low = left <= LOW_CODES_THRESHOLD;
    return (
      <div className="flex flex-col items-start gap-3">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1">
          <Status tone="ok">{t("Açık")}</Status>
          <span className={low ? "font-medium text-warn" : "text-ink-2"}>
            {left === 0
              ? t("Hiç kurtarma kodunuz kalmadı")
              : low
                ? t("Yalnızca {n} kurtarma kodunuz kaldı", { n: left })
                : t("{n} kullanılmamış kurtarma kodu", { n: left })}
          </span>
        </div>
        {low && <p className="text-ink-2">{t("Telefonunuza erişemez hâle gelmeden önce kurtarma kodlarınızı yenilemenizi öneririz.")}</p>}
        <div className="flex flex-wrap gap-2">
          <Button onClick={() => setRegenerating(true)}>{t("Kurtarma kodlarını yenile")}</Button>
          <Button variant="danger" onClick={() => setDisabling(true)}>
            {t("Kapat")}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <Panel title={t("İki adımlı doğrulama")}>
      {status.isPending && <LoadingBlock />}
      {status.isError && <ErrorBlock message={errorMessage(status.error)} onRetry={() => void status.refetch()} />}
      {status.data && renderStatus(status.data)}

      {enabling && (
        <EnableWizard
          onClose={() => {
            setEnabling(false);
            void refresh();
          }}
        />
      )}

      {regenerating && (
        <PasswordConfirmModal
          title={t("Kurtarma kodlarını yenile")}
          description={t("Eski kurtarma kodlarınız geçersiz olur ve 10 yeni kod üretilir. Devam etmek için parolanızı girin.")}
          confirmLabel={t("Kodları yenile")}
          action={(password) => api.post<RecoveryCodesResult>("/auth/2fa/recovery-codes", { password })}
          onSuccess={(result) => {
            setRegenerating(false);
            setNewCodes(result.recovery_codes);
            void refresh();
          }}
          onClose={() => setRegenerating(false)}
        />
      )}

      {newCodes && <RecoveryCodesModal title={t("Yeni kurtarma kodlarınız")} codes={newCodes} onClose={() => setNewCodes(null)} />}

      {disabling && (
        <PasswordConfirmModal
          title={t("İki adımlı doğrulamayı kapat")}
          description={t("Devam etmek için parolanızı girin.")}
          warning={t("Kapattığınızda hesabınız yalnızca parolayla korunur ve kurtarma kodlarınız silinir.")}
          confirmLabel={t("İki adımlı doğrulamayı kapat")}
          danger
          action={(password) => api.post<{ ok: boolean }>("/auth/2fa/disable", { password })}
          onSuccess={() => {
            setDisabling(false);
            feedback.success(t("İki adımlı doğrulama kapatıldı"));
            void refresh();
          }}
          onClose={() => setDisabling(false)}
        />
      )}
    </Panel>
  );
}
