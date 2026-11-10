// Bir sayfada beklenmeyen bir hata olduğunda panelin tamamının boş ekrana
// düşmesini önler; gezinme çalışmaya devam eder, yalnızca içerik alanı hata gösterir.

import type { ErrorInfo, ReactNode } from "react";
import { Component } from "react";
import { t } from "../lib/i18n";
import { Button } from "./ui";

interface Props {
  /** Değiştiğinde hata durumu sıfırlanır (ör. başka sayfaya geçildiğinde). */
  resetKey: string;
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Sayfa hatası:", error, info.componentStack);
  }

  componentDidUpdate(previous: Props) {
    if (previous.resetKey !== this.props.resetKey && this.state.error) {
      this.setState({ error: null });
    }
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    // Panel güncellendikten sonra açık kalan sekmeler eski dosya adlarını ister.
    const staleBundle = /dynamically imported module|Importing a module script failed/i.test(error.message);
    return (
      <div role="alert" className="flex max-w-[60ch] flex-col items-start gap-3 rounded-lg border border-bad/40 bg-bad-soft px-4 py-4">
        <p className="font-semibold text-bad">{staleBundle ? t("Panel güncellenmiş görünüyor") : t("Bu sayfa görüntülenemedi")}</p>
        <p className="text-ink-2">
          {staleBundle
            ? t("Sayfanın bir parçası yüklenemedi. Sayfayı yenilediğinizde güncel sürüm açılır.")
            : t("Beklenmeyen bir hata oluştu. Sayfayı yenilemeyi deneyin; sorun sürerse hata ayrıntısını yöneticinize iletin.")}
        </p>
        {!staleBundle && <code className="font-mono text-[12.5px] break-all text-ink-2">{error.message}</code>}
        <Button onClick={() => window.location.reload()}>{t("Sayfayı yenile")}</Button>
      </div>
    );
  }
}
