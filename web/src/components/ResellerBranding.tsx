// Bayi bazında marka: bayi kendi markasını Hesabım'da düzenler; yönetici Kullanıcılar'da bayinin
// düzenleme penceresinde markanın özetini görür ve kaldırabilir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, errorMessage } from "../lib/api";
import type { Branding } from "../lib/branding";
import { t } from "../lib/i18n";
import { BrandMark } from "./Brand";
import { BrandingForm } from "./BrandingSettings";
import { useFeedback } from "./feedback";
import { Button, ErrorBlock, LoadingBlock, Mono, Panel } from "./ui";

/** Sunucunun bayi markası yanıtı: marka alanları, panel alan adı ve kayıtlı bir marka olup olmadığı. */
interface ResellerBrand extends Branding {
  panel_domain: string;
  defined: boolean;
}

function useResellerBrand(resellerId: number) {
  return useQuery({ queryKey: ["reseller-brand", resellerId], queryFn: () => api.get<ResellerBrand>(`/branding/resellers/${resellerId}`) });
}

/** Marka değişince hem bu bayinin kaydı hem de görünen marka (`/branding`) tazelenir. */
function useRefreshBrand(resellerId: number) {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: ["reseller-brand", resellerId] });
    void queryClient.invalidateQueries({ queryKey: ["branding"] });
  };
}

function useRemoveBrand(resellerId: number) {
  const feedback = useFeedback();
  const refresh = useRefreshBrand(resellerId);
  return useMutation({
    mutationFn: () => api.delete(`/branding/resellers/${resellerId}`),
    onSuccess: () => feedback.success(t("Marka kaldırıldı; sunucunun markası geçerli")),
    onError: (error) => feedback.error(error),
    onSettled: refresh,
  });
}

/** Hesabım sayfasındaki "Marka" bölümü (bayi). Müşterileriniz ve onların alt hesapları bu markayı görür. */
export function ResellerBrandingPanel({ resellerId }: { resellerId: number }) {
  const feedback = useFeedback();
  const brand = useResellerBrand(resellerId);
  const refresh = useRefreshBrand(resellerId);
  const remove = useRemoveBrand(resellerId);

  async function confirmRemove() {
    const confirmed = await feedback.confirm({
      title: t("Markanız kaldırılsın mı?"),
      description: t("Müşterileriniz sunucunun genel markasını görmeye başlar; panel alan adı bağlantınız da kalkar."),
      confirmLabel: t("Markayı kaldır"),
      danger: true,
    });
    if (confirmed) remove.mutate();
  }

  return (
    <Panel title={t("Marka")}>
      <p className="mb-4 max-w-[70ch] text-ink-2">
        {t("Müşterileriniz ve onların alt hesapları panelde sizin adınızı ve logonuzu görür. Tanımlamazsanız sunucunun genel markası görünür.")}
      </p>
      {brand.isPending && <LoadingBlock />}
      {brand.isError && <ErrorBlock message={errorMessage(brand.error)} onRetry={() => void brand.refetch()} />}
      {brand.data && (
        <>
          <BrandingForm
            key={JSON.stringify(brand.data)}
            withDomain
            initial={brand.data}
            onSubmit={(values) => api.put(`/branding/resellers/${resellerId}`, values)}
            onSaved={refresh}
          />
          {brand.data.defined && (
            <div className="mt-4">
              <Button variant="ghost" onClick={() => void confirmRemove()} loading={remove.isPending}>
                {t("Markayı kaldır")}
              </Button>
            </div>
          )}
        </>
      )}
    </Panel>
  );
}

/** Yöneticinin Kullanıcılar'daki bayi düzenleme penceresinde gördüğü marka özeti. */
export function ResellerBrandSummary({ resellerId }: { resellerId: number }) {
  const feedback = useFeedback();
  const brand = useResellerBrand(resellerId);
  const remove = useRemoveBrand(resellerId);

  async function confirmRemove() {
    const confirmed = await feedback.confirm({
      title: t("Bayinin markası kaldırılsın mı?"),
      description: t("Bayinin müşterileri sunucunun genel markasını görmeye başlar; bayinin panel alan adı bağlantısı da kalkar."),
      confirmLabel: t("Markayı kaldır"),
      danger: true,
    });
    if (confirmed) remove.mutate();
  }

  return (
    <Panel title={t("Bayi markası")}>
      {brand.isPending && <LoadingBlock />}
      {brand.isError && <ErrorBlock message={errorMessage(brand.error)} onRetry={() => void brand.refetch()} />}
      {brand.data && !brand.data.defined && <p className="text-ink-2">{t("Bayi kendi markasını tanımlamamış; müşterileri sunucunun genel markasını görür.")}</p>}
      {brand.data?.defined && (
        <div className="flex flex-col gap-4">
          <div className="flex items-center gap-4">
            <div className="grid size-14 shrink-0 place-items-center rounded-lg border border-line bg-sunken/60">
              {brand.data.logo === "" ? <BrandMark className="size-7" /> : <img src={brand.data.logo} alt="" className="max-h-10 max-w-10 object-contain" />}
            </div>
            <dl className="flex min-w-0 flex-col gap-1 text-[14px]">
              <div className="flex gap-2">
                <dt className="text-ink-3">{t("Panel adı")}</dt>
                <dd className="min-w-0 truncate font-medium">{brand.data.name === "" ? "-" : brand.data.name}</dd>
              </div>
              <div className="flex gap-2">
                <dt className="text-ink-3">{t("Panel alan adı")}</dt>
                <dd className="min-w-0 truncate">{brand.data.panel_domain === "" ? "-" : <Mono>{brand.data.panel_domain}</Mono>}</dd>
              </div>
              <div className="flex gap-2">
                <dt className="text-ink-3">{t("Destek bağlantısı")}</dt>
                <dd className="min-w-0 truncate">{brand.data.support_url === "" ? "-" : <Mono>{brand.data.support_url}</Mono>}</dd>
              </div>
            </dl>
          </div>
          <div>
            <Button variant="ghost" onClick={() => void confirmRemove()} loading={remove.isPending}>
              {t("Markayı kaldır")}
            </Button>
          </div>
        </div>
      )}
    </Panel>
  );
}
