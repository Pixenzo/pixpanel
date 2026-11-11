// Panelin temel arayüz bileşenleri.

import * as Dialog from "@radix-ui/react-dialog";
import { LoaderCircle, X } from "lucide-react";
import type { ButtonHTMLAttributes, InputHTMLAttributes, ReactNode, SelectHTMLAttributes, TextareaHTMLAttributes } from "react";
import { useId, useLayoutEffect, useRef } from "react";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";

// ---- Düğme ----

type ButtonVariant = "primary" | "secondary" | "ghost" | "danger";

const buttonVariants: Record<ButtonVariant, string> = {
  primary: "bg-accent text-accent-ink hover:bg-accent-hover border-transparent shadow-[0_1px_2px_rgb(21_32_43/0.18)]",
  secondary: "bg-surface text-ink border-line-strong hover:bg-sunken shadow-[0_1px_1px_rgb(21_32_43/0.04)]",
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
        "inline-flex items-center justify-center gap-2 rounded-lg border font-medium whitespace-nowrap transition-colors",
        "disabled:cursor-not-allowed disabled:opacity-55",
        size === "sm" ? "h-8 px-3 text-[13.5px] max-sm:h-9" : "h-9.5 px-4 max-sm:h-11",
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
        "inline-flex size-8.5 shrink-0 items-center justify-center rounded-lg text-ink-2 transition-colors max-sm:size-10",
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

// Telefonda 16 px yazı: iOS, daha küçük yazılı alana odaklanınca sayfayı yakınlaştırır.
const controlClass =
  "h-9.5 w-full rounded-lg border border-line-strong bg-surface px-3.5 text-ink placeholder:text-ink-3 max-sm:h-11 max-sm:text-[16px] " +
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
    <div className="flex flex-col gap-2">
      <label htmlFor={id} className="text-[14px] font-medium text-ink">
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
        <p id={`${id}-hint`} className={classNames("text-[13px]", error ? "text-bad" : "text-ink-3")}>
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
    <div className="flex flex-col gap-2">
      <label htmlFor={id} className="text-[14px] font-medium text-ink">
        {label}
      </label>
      <select id={id} className={classNames(controlClass, "pr-8", className)} {...rest}>
        {children}
      </select>
      {hint && <p className="text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}

export function Checkbox({
  label,
  hint,
  ...rest
}: { label: string; hint?: string } & InputHTMLAttributes<HTMLInputElement>) {
  return (
    <label className="flex items-start gap-2.5 max-sm:py-1">
      <input type="checkbox" className="mt-0.5 size-4 shrink-0 accent-(--accent) max-sm:size-5" {...rest} />
      <span>
        <span className="text-ink">{label}</span>
        {hint && <span className="block text-[13px] text-ink-3">{hint}</span>}
      </span>
    </label>
  );
}

/** Tablo üstündeki arama kutusu gibi etiketsiz, küçük girdiler için. */
export function SearchInput({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input type="search" className={classNames(controlClass, "h-8 sm:max-w-64", className)} {...rest} />;
}

/** Aç/kapa düğmesi; `label` ekran okuyucular içindir, görünen metni çağıran yazar. */
export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (next: boolean) => void;
  label: string;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={classNames(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full transition-colors max-sm:h-6 max-sm:w-11",
        "disabled:cursor-not-allowed disabled:opacity-55",
        checked ? "bg-accent" : "bg-line-strong",
      )}
    >
      <span
        aria-hidden
        className={classNames(
          "size-4 rounded-full bg-white shadow-sm transition-transform max-sm:size-5",
          checked ? "translate-x-[1.125rem] max-sm:translate-x-[1.375rem]" : "translate-x-0.5",
        )}
      />
    </button>
  );
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
    <span className={classNames("inline-flex items-center rounded-md px-2 py-0.5 text-[12.5px] font-medium", toneSoft[tone])}>
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
      className="h-2 w-full overflow-hidden rounded-full bg-sunken"
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
    <header className="mb-6 flex flex-wrap items-end justify-between gap-x-8 gap-y-4 sm:mb-8">
      <div className="min-w-0">
        <h1 className="text-[23px] leading-[1.15] font-semibold tracking-[-0.02em] sm:text-[27px]">{title}</h1>
        {description && <p className="mt-2 max-w-[68ch] text-[15.5px] text-ink-2">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2 max-sm:w-full max-sm:[&>*]:flex-1">{actions}</div>}
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
    <section className={classNames("px-panel overflow-hidden rounded-2xl border border-line bg-surface shadow-card", className)}>
      {(title || actions) && (
        <div className="flex min-h-15 flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-line px-4 sm:px-6 py-3 sm:px-6">
          <h2 className="min-w-0 text-[16px] font-semibold tracking-[-0.01em]">{title}</h2>
          {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className={flush ? "" : "p-4 sm:p-6"}>{children}</div>
    </section>
  );
}

export function EmptyState({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-2.5 px-6 py-14 sm:items-center sm:text-center">
      <p className="text-[17px] font-semibold tracking-[-0.01em]">{title}</p>
      {description && <p className="max-w-[54ch] text-ink-2">{description}</p>}
      {action && <div className="mt-3">{action}</div>}
    </div>
  );
}

/** İçerik gelene kadar yerini tutan soluk çubuk. */
export function Skeleton({ className }: { className?: string }) {
  return (
    <span
      aria-hidden
      className={classNames(
        "block animate-[shimmer_1.6s_linear_infinite] rounded bg-[linear-gradient(90deg,var(--sunken)_25%,var(--line)_37%,var(--sunken)_63%)] bg-[length:200%_100%]",
        className,
      )}
    />
  );
}

/** Sayfa verisi yüklenirken gösterilen iskelet; gelecek içeriğin kabaca biçimini taşır. */
export function LoadingBlock() {
  return (
    <div role="status" aria-label={t("Yükleniyor")} className="flex flex-col gap-3 p-4">
      <Skeleton className="h-4 w-40" />
      <Skeleton className="h-3.5 w-full" />
      <Skeleton className="h-3.5 w-11/12" />
      <Skeleton className="h-3.5 w-2/3" />
    </div>
  );
}

export function ErrorBlock({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return (
    <div role="alert" className="flex flex-wrap items-center gap-3 rounded-lg border border-bad/40 bg-bad-soft px-4 sm:px-6 py-3 text-bad">
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

/**
 * Veri tablosu. Dar ekranda her satır bir karta dönüşür: ilk hücre başlık olur, öteki
 * hücreler "sütun adı: değer" çiftleri olarak alt alta dizilir (kurallar `styles.css`
 * içinde `.stack-table`). Sütun adları başlık satırından okunup hücrelere işlendiği
 * için sayfaların ayrıca bir şey yapması gerekmez.
 *
 * `scroll`: çok sütunlu, satırları kıyaslanarak okunan tablolar (süreç listesi gibi)
 * karta dönüşmek yerine yana kayar.
 */
export function Table({ children, className, scroll }: { children: ReactNode; className?: string; scroll?: boolean }) {
  const ref = useRef<HTMLTableElement>(null);

  useLayoutEffect(() => {
    const table = ref.current;
    if (!table || scroll) return;
    const label = () => {
      // İşlem sütununun adı kartta yer kaplamasın: düğmeler kendini anlatır.
      const actions = t("İşlemler");
      const labels = Array.from(table.querySelectorAll("thead th")).map((cell) => {
        const text = cell.textContent?.trim() ?? "";
        return text === actions ? "" : text;
      });
      for (const row of table.querySelectorAll("tbody > tr")) {
        Array.from(row.children).forEach((cell, index) => {
          const text = labels[index] ?? "";
          if (cell.getAttribute("data-label") !== text) cell.setAttribute("data-label", text);
          // İçeriği olmayan hücre kartta boş bir satır bırakmasın.
          const blank = (cell.textContent ?? "").trim() === "" && !cell.querySelector("button, a, input, select, svg, img, [role]");
          cell.toggleAttribute("data-blank", blank);
        });
      }
    };
    label();
    // Satırlar veri geldikçe eklenir; öznitelik yazmak bu gözlemciyi yeniden tetiklemez.
    const observer = new MutationObserver(label);
    observer.observe(table, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [scroll]);

  return (
    <div className={scroll ? "overflow-x-auto" : "sm:overflow-x-auto"}>
      <table ref={ref} className={classNames("w-full border-collapse text-left", !scroll && "stack-table", className)}>
        {children}
      </table>
    </div>
  );
}

export function Th({ children, className, align }: { children?: ReactNode; className?: string; align?: "right" }) {
  return (
    <th
      scope="col"
      className={classNames(
        "border-b border-line bg-sunken/50 px-4 sm:px-6 py-3 text-[13px] font-medium whitespace-nowrap text-ink-2",
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
    <td className={classNames("border-b border-line px-4 sm:px-6 py-3.5 align-middle", align === "right" && "text-right", className)}>
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
            "fixed z-50 flex flex-col border border-line bg-surface shadow-pop",
            // Telefonda ekranın altından açılan sayfa; düğmeler başparmağın ulaştığı yerde durur.
            "max-sm:inset-x-0 max-sm:bottom-0 max-sm:max-h-[92dvh] max-sm:animate-[sheet-in_220ms_cubic-bezier(0.2,0.8,0.2,1)] max-sm:rounded-t-2xl max-sm:border-b-0",
            "sm:left-1/2 sm:w-[calc(100vw-2rem)] sm:-translate-x-1/2 sm:animate-[pop-in_140ms_ease-out] sm:rounded-xl",
            extraWide ? "sm:top-[6vh] sm:max-h-[88vh]" : "sm:top-[12vh] sm:max-h-[80vh]",
            extraWide ? "sm:max-w-5xl" : wide ? "sm:max-w-2xl" : "sm:max-w-md",
          )}
        >
          <span aria-hidden className="mx-auto mt-2 h-1 w-9 shrink-0 rounded-full bg-line-strong sm:hidden" />
          <div className="flex items-start justify-between gap-4 border-b border-line px-4 sm:px-6 py-4 sm:px-6 sm:py-5">
            <div className="min-w-0">
              <Dialog.Title className="text-[18px] leading-snug font-semibold tracking-[-0.01em]">{title}</Dialog.Title>
              {description && <p className="mt-1.5 text-ink-2">{description}</p>}
            </div>
            <Dialog.Close asChild>
              <IconButton label={t("Kapat")} className="-mr-2">
                <X className="size-4" />
              </IconButton>
            </Dialog.Close>
          </div>
          <div className="overflow-y-auto overscroll-contain px-4 sm:px-6 py-5 sm:px-6 sm:py-6">{children}</div>
          {footer && (
            <div className="flex justify-end gap-2.5 border-t border-line bg-sunken/40 px-4 sm:px-6 py-4 max-sm:pb-[calc(1rem+env(safe-area-inset-bottom))] max-sm:[&>*]:flex-1 sm:px-6">
              {footer}
            </div>
          )}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Makine değerleri (yol, IP, komut) için eş aralıklı metin. */
export function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={classNames("font-mono text-[13px]", className)}>{children}</span>;
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
    <div role="tablist" aria-label={label} className="no-scrollbar mb-6 flex gap-1.5 overflow-x-auto overflow-y-hidden border-b border-line max-sm:-mx-4 max-sm:px-4">
      {tabs.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          aria-selected={value === tab.id}
          onClick={() => onChange(tab.id)}
          className={classNames(
            "-mb-px h-11 shrink-0 border-b-2 px-3.5 font-medium whitespace-nowrap transition-colors",
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
    <div className="flex flex-col gap-2">
      <label htmlFor={id} className="text-[14px] font-medium text-ink">
        {label}
      </label>
      <textarea id={id} className={classNames(controlClass, "h-auto min-h-28 py-2 font-mono text-[13px]", className)} {...rest} />
      {hint && <p className="text-[13px] text-ink-3">{hint}</p>}
    </div>
  );
}
