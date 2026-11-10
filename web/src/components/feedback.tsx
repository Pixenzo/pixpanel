// Bildirimler (toast) ve onay pencereleri için uygulama geneli sağlayıcı.

import { CircleAlert, CircleCheck } from "lucide-react";
import type { ReactNode } from "react";
import { createContext, useCallback, useContext, useMemo, useRef, useState } from "react";
import { errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import { Button, Input, Modal } from "./ui";

interface Toast {
  id: number;
  kind: "success" | "error";
  message: string;
}

interface ConfirmOptions {
  title: string;
  description?: string;
  confirmLabel: string;
  danger?: boolean;
  /** Verilirse kullanıcı bu metni yazmadan onaylayamaz (geri dönüşsüz işlemler için). */
  typeToConfirm?: string;
}

interface Feedback {
  success: (message: string) => void;
  error: (error: unknown) => void;
  confirm: (options: ConfirmOptions) => Promise<boolean>;
}

const FeedbackContext = createContext<Feedback | null>(null);

export function useFeedback(): Feedback {
  const context = useContext(FeedbackContext);
  if (!context) throw new Error("FeedbackProvider eksik");
  return context;
}

export function FeedbackProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [pending, setPending] = useState<ConfirmOptions | null>(null);
  const [typed, setTyped] = useState("");
  const resolver = useRef<((value: boolean) => void) | null>(null);
  const nextId = useRef(1);

  const push = useCallback((kind: Toast["kind"], message: string) => {
    const id = nextId.current++;
    setToasts((list) => [...list, { id, kind, message }]);
    // Hata iletileri okunabilsin diye daha uzun kalır.
    window.setTimeout(() => setToasts((list) => list.filter((toast) => toast.id !== id)), kind === "error" ? 8000 : 3500);
  }, []);

  const settle = useCallback((value: boolean) => {
    resolver.current?.(value);
    resolver.current = null;
    setPending(null);
    setTyped("");
  }, []);

  const feedback = useMemo<Feedback>(
    () => ({
      success: (message) => push("success", message),
      error: (error) => push("error", errorMessage(error)),
      confirm: (options) =>
        new Promise<boolean>((resolve) => {
          resolver.current = resolve;
          setPending(options);
        }),
    }),
    [push],
  );

  const blocked = Boolean(pending?.typeToConfirm) && typed !== pending?.typeToConfirm;

  return (
    <FeedbackContext.Provider value={feedback}>
      {children}

      <Modal
        open={pending !== null}
        onClose={() => settle(false)}
        title={pending?.title ?? ""}
        footer={
          <>
            <Button onClick={() => settle(false)}>{t("Vazgeç")}</Button>
            <Button variant={pending?.danger ? "danger" : "primary"} disabled={blocked} onClick={() => settle(true)}>
              {pending?.confirmLabel}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-4">
          {pending?.description && <p className="text-ink-2">{pending.description}</p>}
          {pending?.typeToConfirm && (
            <Input
              label={t("Onaylamak için “{text}” yazın", { text: pending.typeToConfirm })}
              value={typed}
              onChange={(event) => setTyped(event.target.value)}
              autoComplete="off"
              autoFocus
            />
          )}
        </div>
      </Modal>

      <div aria-live="polite" className="pointer-events-none fixed right-4 bottom-4 z-[60] flex w-[min(24rem,calc(100vw-2rem))] flex-col gap-2">
        {toasts.map((toast) => (
          <div
            key={toast.id}
            role={toast.kind === "error" ? "alert" : "status"}
            className={classNames(
              "pointer-events-auto flex animate-[pop-in_160ms_ease-out] items-start gap-2.5 rounded-lg border bg-surface px-3.5 py-3 shadow-pop",
              toast.kind === "error" ? "border-bad/50" : "border-line",
            )}
          >
            {toast.kind === "error" ? (
              <CircleAlert aria-hidden className="mt-0.5 size-4 shrink-0 text-bad" />
            ) : (
              <CircleCheck aria-hidden className="mt-0.5 size-4 shrink-0 text-ok" />
            )}
            <span className="min-w-0 break-words">{toast.message}</span>
          </div>
        ))}
      </div>
    </FeedbackContext.Provider>
  );
}
