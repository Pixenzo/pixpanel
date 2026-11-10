// Panelin temel arayüz bileşenleri.

import * as Dialog from "@radix-ui/react-dialog";
import { LoaderCircle, X } from "lucide-react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { useId } from "react";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Düğme ----

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const buttonVariants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-ink hover:bg-accent-hover border-transparent",
  secondary: "bg-surface text-ink border-line-strong hover:bg-sunken",
  ghost: "bg-transparent text-ink-2 border-transparent hover:bg-sunken hover:text-ink",
  danger: "bg-bad text-white border-transparent hover:brightness-110",
};

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "sm" | "md";
  loading?: boolean;
  icon?: ReactNode;
}

export function Button({
  variant = "secondary",
  size = "md",
  loading,
  icon,
  children,
  className,
  disabled,
  type = "button",
  ...rest
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      className={classNames(
        "inline-flex items-center justify-center gap-1.5 rounded-md border font-medium whitespace-nowrap transition-colors",
        "disabled:cursor-not-allowed disabled:opacity-55",
        size === "sm" ? "h-7 px-2.5 text-[13px]" : "h-9 px-3.5",
        buttonVariants[variant],
        className,
      )}
      {...rest}
    >
      {loading ? <Spinner /> : icon}
      {children}
    </button>
  );
}

/** Yalnızca simgeden oluşan düğme; erişilebilirlik için `label` zorunludur. */
export function IconButton({
  label,
  children,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      className={classNames(
        "inline-flex size-8 items-center justify-center rounded-md text-ink-2 transition-colors",
        "hover:bg-sunken hover:text-ink disabled:cursor-not-allowed disabled:opacity-40",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}

export function Spinner({ className }: { className?: string }) {
  return <LoaderCircle aria-hidden className={classNames("size-4 animate-[spin_0.8s_linear_infinite]", className)} />;
}

// ---- Form alanları ----

const controlClass =
  "h-9 w-full rounded-md border border-line-strong bg-surface px-3 text-ink placeholder:text-ink-3 " +
  "transition-colors focus:border-accent focus:outline-2 focus:outline-offset-0 focus:outline-accent/30 " +
  "disabled:bg-sunken disabled:text-ink-3";

interface FieldProps {
  label: string;
  hint?: string;
  error?: string;
}

export function Input({ label, hint, error, className, ...rest }: FieldProps & InputHTMLAttributes<HTMLInputElement>) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-ink-2">
        {label}
      </label>
      <input
        id={id}
        aria-invalid={error ? true : undefined}
        aria-describedby={hint || error ? `${id}-hint` : undefined}
        className={classNames(controlClass, error && "border-bad", className)}
        {...rest}
      />
      {(error || hint) && (
        <p id={`${id}-hint`} className={classNames("text-[12.5px]", error ? "text-bad" : "text-ink-3")}>
          {error || hint}
        </p>
      )}
    </div>
  );
}

export function Select({
  label,
  hint,
  children,
  className,
  ...rest
}: FieldProps & SelectHTMLAttributes<HTMLSelectElement>) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-ink-2">
        {label}
      </label>
      <select id={id} className={classNames(controlClass, "pr-8", className)} {...rest}>
        {children}
      </select>
      {hint && <p className="text-[12.5px] text-ink-3">{hint}</p>}
    </div>
  );
}

export function Checkbox({
  label,
  hint,
  ...rest
}: { label: string; hint?: string } & InputHTMLAttributes<HTMLInputElement>) {
  return (
    <label className="flex items-start gap-2.5">
      <input type="checkbox" className="mt-0.5 size-4 accent-(--accent)" {...rest} />
      <span>
        <span className="text-ink">{label}</span>
        {hint && <span className="block text-[12.5px] text-ink-3">{hint}</span>}
      </span>
    </label>
  );
}

/** Tablo üstündeki arama kutusu gibi etiketsiz, küçük girdiler için. */
export function SearchInput({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input type="search" className={classNames(controlClass, "h-8 max-w-64", className)} {...rest} />;
}

// ---- Durum göstergeleri ----

export type Tone = "ok" | "warn" | "bad" | "idle" | "accent";

const toneText: Record<Tone, string> = {
  ok: "text-ok",
  warn: "text-warn",
  bad: "text-bad",
  idle: "text-ink-3",
  accent: "text-accent",
};

const toneSoft: Record<Tone, string> = {
  ok: "bg-ok-soft text-ok",
  warn: "bg-warn-soft text-warn",
  bad: "bg-bad-soft text-bad",
  idle: "bg-sunken text-ink-2",
  accent: "bg-accent-soft text-accent",
};

export const toneFill: Record<Tone, string> = {
  ok: "bg-ok",
  warn: "bg-warn",
  bad: "bg-bad",
  idle: "bg-idle",
  accent: "bg-accent",
};

/** Durumu küçük bir LED ve metinle gösterir. */
export function Status({ tone, children }: { tone: Tone; children: ReactNode }) {
  return (
    <span className={classNames("inline-flex items-center gap-1.5 font-medium", toneText[tone])}>
      <span aria-hidden className={classNames("size-2 rounded-full", toneFill[tone])} />
      {children}
    </span>
  );
}

export function Badge({ tone = "idle", children }: { tone?: Tone; children: ReactNode }) {
  return (
    <span className={classNames("inline-flex items-center rounded px-1.5 py-0.5 text-[12px] font-medium", toneSoft[tone])}>
      {children}
    </span>
  );
}

/** Doluluk çubuğu; eşiklere göre kendiliğinden renk değiştirir. */
export function Meter({ value, label }: { value: number; label: string }) {
  const percent = Math.max(0, Math.min(100, value));
  const tone: Tone = percent >= 90 ? "bad" : percent >= 75 ? "warn" : "accent";
  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuenow={Math.round(percent)}
      aria-valuemin={0}
      aria-valuemax={100}
      className="h-1.5 w-full overflow-hidden rounded-full bg-sunken"
    >
      <div className={classNames("h-full rounded-full transition-[width] duration-500", toneFill[tone])} style={{ width: `${percent}%` }} />
    </div>
  );
}

// ---- Yerleşim ----

export function PageHeader({
  title,
  description,
  actions,
}: {
  title: string;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="mb-5 flex flex-wrap items-end justify-between gap-x-6 gap-y-3">
      <div className="min-w-0">
        <h1 className="text-[22px] leading-tight font-semibold tracking-[-0.01em]">{title}</h1>
        {description && <p className="mt-1 max-w-[70ch] text-ink-2">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}

export function Panel({
  title,
  actions,
  children,
  className,
  flush,
}: {
  title?: ReactNode;
  actions?: ReactNode;
  children: ReactNode;
  className?: string;
  /** İçerik kenarlara yapışsın (tablolar için). */
  flush?: boolean;
}) {
  return (
    <section className={classNames("rounded-lg border border-line bg-surface", className)}>
      {(title || actions) && (
        <div className="flex min-h-11 items-center justify-between gap-3 border-b border-line px-4 py-2">
          <h2 className="font-semibold">{title}</h2>
          {actions && <div className="flex items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className={flush ? "" : "p-4"}>{children}</div>
    </section>
  );
}

export function EmptyState({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-2 px-4 py-10 sm:items-center sm:text-center">
      <p className="font-semibold">{title}</p>
      {description && <p className="max-w-[52ch] text-ink-2">{description}</p>}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}

export function LoadingBlock() {
  return (
    <div className="flex items-center gap-2 px-4 py-10 text-ink-3">
      <Spinner /> {t("Yükleniyor")}
    </div>
  );
}

export function ErrorBlock({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-3 rounded-lg border border-bad/40 bg-bad-soft px-4 py-3 text-bad">
      <span className="min-w-0 flex-1">{message}</span>
      {onRetry && (
        <Button size="sm" onClick={onRetry}>
          {t("Yeniden dene")}
        </Button>
      )}
    </div>
  );
}

// ---- Tablo ----

export function Table({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <div className="overflow-x-auto">
      <table className={classNames("w-full border-collapse text-left", className)}>{children}</table>
    </div>
  );
}

export function Th({ children, className, align }: { children?: ReactNode; className?: string; align?: "right" }) {
  return (
    <th
      scope="col"
      className={classNames(
        "border-b border-line bg-sunken/60 px-4 py-2 text-[12.5px] font-medium whitespace-nowrap text-ink-2",
        align === "right" && "text-right",
        className,
      )}
    >
      {children}
    </th>
  );
}

export function Td({ children, className, align }: { children?: ReactNode; className?: string; align?: "right" }) {
  return (
    <td className={classNames("border-b border-line px-4 py-2 align-middle", align === "right" && "text-right", className)}>
      {children}
    </td>
  );
}

// ---- Pencere ----

export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  wide,
  extraWide,
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  description?: string;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  /** Kural kurucu, günlük görüntüleyici gibi geniş içerikler için. */
  extraWide?: boolean;
}) {
  return (
    <Dialog.Root open={open} onOpenChange={(next) => !next && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-40 animate-[fade-in_120ms_ease-out] bg-[rgb(10_16_22/0.5)]" />
        <Dialog.Content
          aria-describedby={undefined}
          className={classNames(
            "fixed left-1/2 z-50 flex w-[calc(100vw-2rem)] -translate-x-1/2 flex-col",
            extraWide ? "top-[6vh] max-h-[88vh]" : "top-[12vh] max-h-[80vh]",
            "animate-[pop-in_140ms_ease-out] rounded-xl border border-line bg-surface shadow-pop",
            extraWide ? "max-w-5xl" : wide ? "max-w-2xl" : "max-w-md",
          )}
        >
          <div className="flex items-start justify-between gap-4 border-b border-line px-5 py-3.5">
            <div>
              <Dialog.Title className="text-[16px] font-semibold">{title}</Dialog.Title>
              {description && <p className="mt-0.5 text-ink-2">{description}</p>}
            </div>
            <Dialog.Close asChild>
              <IconButton label={t("Kapat")} className="-mr-2">
                <X className="size-4" />
              </IconButton>
            </Dialog.Close>
          </div>
          <div className="overflow-y-auto px-5 py-4">{children}</div>
          {footer && <div className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</div>}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Makine değerleri (yol, IP, komut) için eş aralıklı metin. */
export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={classNames("font-mono text-[12.5px]", className)}>{children}</span>;
}

// ---- Sekmeler ----

export function Tabs<T extends string>({
  tabs,
  value,
  onChange,
  label,
}: {
  tabs: { id: T; label: string }[];
  value: T;
  onChange: (next: T) => void;
  label: string;
}) {
  return (
    <div role="tablist" aria-label={label} className="mb-4 flex gap-1 overflow-x-auto overflow-y-hidden border-b border-line">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          aria-selected={value === tab.id}
          onClick={() => onChange(tab.id)}
          className={classNames(
            "-mb-px h-9 border-b-2 px-3 font-medium whitespace-nowrap transition-colors",
            value === tab.id ? "border-accent text-accent" : "border-transparent text-ink-2 hover:text-ink",
          )}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}

export function Textarea({
  label,
  hint,
  className,
  ...rest
}: FieldProps & TextareaHTMLAttributes<HTMLTextAreaElement>) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1.5">
      <label htmlFor={id} className="text-[13px] font-medium text-ink-2">
        {label}
      </label>
      <textarea id={id} className={classNames(controlClass, "h-auto min-h-28 py-2 font-mono text-[12.5px]", className)} {...rest} />
      {hint && <p className="text-[12.5px] text-ink-3">{hint}</p>}
    </div>
  );
}
