// WordPress sıkılaştırma denetimi: sitenin ayarlarını güvenlik kontrol listesine göre gösterir,
// düzeltilebilenleri tek tıkla düzeltir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCw, ShieldCheck } from "lucide-react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, ErrorBlock, LoadingBlock, Panel, Status } from "../components/ui";
import type { Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";

interface SecurityItem {
  id: string;
  title: string;
  ok: boolean;
  detail: string;
  fixable: boolean;
  /** Bilgi maddesi: özetteki sayıma girmez. */
  info: boolean;
}

interface SecurityAudit {
  items: SecurityItem[];
  passed: number;
  total: number;
}

interface FixResult {
  fixed: string[];
  skipped: string[];
  failed: { id: string; error: string }[];
  audit: SecurityAudit;
}

/** Sorunlu maddeler üstte, ardından bilgi, en altta tamam olanlar. */
function sorted(items: SecurityItem[]): SecurityItem[] {
  const rank = (item: SecurityItem) => (!item.ok && !item.info ? 0 : item.info ? 1 : 2);
  return [...items].sort((a, b) => rank(a) - rank(b));
}

export function SecurityPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const queryKey = ["wordpress-security", site.id];
  // Denetim çekirdek dosyalarını okuduğu için birkaç saniye sürebilir; sık yinelenmesin.
  const audit = useQuery({ queryKey, queryFn: () => api.get<SecurityAudit>(`/sites/${site.id}/wordpress/security`), staleTime: 60000 });

  const fix = useMutation({
    mutationFn: (ids: string[]) => api.post<FixResult>(`/sites/${site.id}/wordpress/security/fix`, { ids }),
    onSuccess: (result) => {
      queryClient.setQueryData(queryKey, result.audit);
      // Şablon değiştiyse sitenin kaydı da değişmiştir.
      void queryClient.invalidateQueries({ queryKey: ["site"] });
      if (result.fixed.length > 0) {
        feedback.success(result.fixed.length === 1 ? t("1 madde düzeltildi") : t("{count} madde düzeltildi", { count: result.fixed.length }));
      }
      if (result.failed.length > 0) {
        const names = result.failed.map((failure) => `${t(titleOf(result.audit, failure.id))}: ${failure.error}`).join("\n");
        feedback.error(new Error(names));
      }
    },
    onError: (error) => feedback.error(error),
  });

  async function run(ids: string[]) {
    // Anahtarların yenilenmesi, sitede açık bütün oturumları kapatır.
    if (ids.includes("salts")) {
      const confirmed = await feedback.confirm({
        title: t("Güvenlik anahtarları yenilensin mi?"),
        description: t("Sitede oturumu açık olan herkes (siz dahil) yeniden giriş yapmak zorunda kalır."),
        confirmLabel: t("Yenile"),
      });
      if (!confirmed) return;
    }
    fix.mutate(ids);
  }

  if (audit.isPending) {
    return (
      <Panel title={t("Güvenlik denetimi")}>
        <LoadingBlock />
      </Panel>
    );
  }
  if (audit.isError) {
    return (
      <Panel title={t("Güvenlik denetimi")}>
        <ErrorBlock message={errorMessage(audit.error)} onRetry={() => void audit.refetch()} />
      </Panel>
    );
  }

  const data = audit.data;
  const fixable = data.items.filter((item) => item.fixable && !item.ok);
  const busy = (id: string) => fix.isPending && fix.variables?.includes(id);

  return (
    <Panel
      flush
      title={t("Güvenlik denetimi")}
      actions={
        <>
          <Button size="sm" loading={audit.isFetching && !fix.isPending} icon={<RefreshCw className="size-4" />} onClick={() => void audit.refetch()}>
            {t("Yeniden denetle")}
          </Button>
          {fixable.length > 0 && (
            <Button size="sm" variant="primary" loading={fix.isPending && fix.variables?.length === fixable.length} icon={<ShieldCheck className="size-4" />} onClick={() => void run(fixable.map((item) => item.id))}>
              {t("Hepsini düzelt")}
            </Button>
          )}
        </>
      }
    >
      <p className="border-b border-line px-4 py-3 sm:px-6">
        <Status tone={data.passed === data.total ? "ok" : "warn"}>{t("{total} maddeden {passed} tanesi tamam", { total: data.total, passed: data.passed })}</Status>
      </p>
      <ul className="divide-y divide-line">
        {sorted(data.items).map((item) => (
          <li key={item.id} className="flex flex-wrap items-start gap-x-3 gap-y-2 px-4 py-3 sm:px-6">
            <div className="min-w-0 flex-1 basis-64">
              <p className="flex flex-wrap items-center gap-2 font-medium">
                <span aria-hidden className={item.info ? "text-ink-2" : item.ok ? "text-ok" : "text-warn"}>
                  {item.info ? "•" : item.ok ? "✓" : "!"}
                </span>
                {t(item.title)}
                {item.info && <Badge>{t("Bilgi")}</Badge>}
                {!item.ok && !item.info && <Badge tone="warn">{t("Sorunlu")}</Badge>}
              </p>
              <p className="mt-0.5 text-[13.5px] text-ink-2">{item.detail}</p>
            </div>
            {item.fixable && !item.ok && (
              <Button size="sm" loading={busy(item.id)} disabled={fix.isPending} onClick={() => void run([item.id])}>
                {t("Düzelt")}
              </Button>
            )}
          </li>
        ))}
      </ul>
    </Panel>
  );
}

function titleOf(audit: SecurityAudit, id: string): string {
  return audit.items.find((item) => item.id === id)?.title ?? id;
}
