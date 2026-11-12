// Web sunucusunun sunucu geneli ayarları: HTTP/3 (QUIC).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useFeedback } from "./feedback";
import { LoadingBlock, Panel, Switch } from "./ui";
import { api } from "../lib/api";
import { t } from "../lib/i18n";

interface Http3State {
  supported: boolean;
  enabled: boolean;
  failed?: string[];
}

export function WebServerSettings() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const state = useQuery({ queryKey: ["http3"], queryFn: () => api.get<Http3State>("/system/http3") });
  const save = useMutation({
    mutationFn: (enabled: boolean) => api.put<Http3State>("/system/http3", { enabled }),
    onSuccess: (result) => {
      queryClient.setQueryData(["http3"], result);
      if (result.failed && result.failed.length > 0) {
        feedback.error(t("Ayar şu sitelere uygulanamadı: {sites}", { sites: result.failed.join(", ") }));
      } else {
        feedback.success(result.enabled ? t("HTTP/3 açıldı") : t("HTTP/3 kapatıldı"));
      }
    },
    onError: (error) => feedback.error(error),
  });

  return (
    <Panel title={t("Web sunucusu")} className="mb-4">
      {state.isPending && <LoadingBlock />}
      {state.data && (
        <div className="flex items-start gap-4">
          <div className="min-w-0 flex-1">
            <p className="font-medium">HTTP/3 (QUIC)</p>
            <p className="mt-0.5 max-w-[70ch] text-[13.5px] text-ink-2">
              {state.data.supported
                ? t("Sertifikalı siteler HTTP/3 üzerinden de yanıt verir; özellikle mobil ve kesintili bağlantılarda sayfalar daha hızlı açılır. Güvenlik duvarında UDP 443 açık olmalıdır (Web siteleri kümesi bunu kapsar). Belirli bir sunucu adresine bağlı siteler HTTP/2 ile devam eder.")
                : t("Kurulu nginx HTTP/3 desteğiyle derlenmemiş; bu sunucuda açılamaz.")}
            </p>
          </div>
          <Switch label="HTTP/3" checked={state.data.enabled} disabled={!state.data.supported || save.isPending} onChange={(next) => save.mutate(next)} />
        </div>
      )}
    </Panel>
  );
}
