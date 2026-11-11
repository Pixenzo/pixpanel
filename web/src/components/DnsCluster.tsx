// DNS kümesi: bölgelerin ikincil ad sunucularına eşitlenmesi. Bu sunucu birincildir;
// ikinciller bölgeleri buradan çeker. İkincil de bir PixPanel ise bölge listesi ona
// kendiliğinden iletilir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, RefreshCw, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import type { DnsCluster as ClusterData, DnsClusterSecondary } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatRelative } from "../lib/format";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Status } from "./ui";

interface SecondaryBody {
  address: string;
  panel_url: string;
  token: string;
}

function toBody(item: DnsClusterSecondary): SecondaryBody {
  // Boş anahtar "kayıtlı olanı koru" demektir.
  return { address: item.address, panel_url: item.panel_url, token: "" };
}

/** Parmak izini okunur parçalara böler. */
function groupFingerprint(value: string): string {
  return (value.match(/.{1,4}/g) ?? []).join(" ");
}

function AddModal({ current, onClose }: { current: DnsClusterSecondary[]; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [address, setAddress] = useState("");
  const [panelUrl, setPanelUrl] = useState("");
  const [token, setToken] = useState("");

  const save = useMutation({
    mutationFn: () =>
      api.put<ClusterData>("/dns/cluster", {
        secondaries: [...current.map(toBody), { address: address.trim(), panel_url: panelUrl.trim(), token: token.trim() }],
      }),
    onSuccess: (updated) => {
      queryClient.setQueryData(["dns-cluster"], updated);
      feedback.success(t("{address} ikincil ad sunucusu olarak eklendi", { address: address.trim() }));
      onClose();
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Modal
      open
      onClose={onClose}
      wide
      title={t("İkincil ad sunucusu ekle")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="dns-secondary-form" loading={save.isPending}>
            {t("İkincili ekle")}
          </Button>
        </>
      }
    >
      <form id="dns-secondary-form" onSubmit={submit} className="flex flex-col gap-4">
        <Input
          label={t("İkincil sunucunun IP adresi")}
          value={address}
          onChange={(event) => setAddress(event.target.value)}
          placeholder="203.0.113.20"
          hint={t("Bu adrese bölge aktarımı izni verilir ve her değişiklikte bildirim gönderilir.")}
          className="font-mono text-[13px]"
          spellCheck={false}
          autoComplete="off"
          autoFocus
          required
        />
        <fieldset className="flex flex-col gap-4 border-t border-line pt-4">
          <legend className="sr-only">{t("İkincil sunucu PixPanel ise")}</legend>
          <p className="text-[13px] text-ink-2">
            {t("İkincil sunucuda da PixPanel kuruluysa panel adresini ve orada üretilmiş bir yönetici API anahtarını girin: bölgeler ikincilde kendiliğinden açılır ve silinir. Boş bırakırsanız her bölgeyi ikincilde elle tanımlamanız gerekir.")}
          </p>
          <Input
            label={t("İkincil panelin adresi")}
            value={panelUrl}
            onChange={(event) => setPanelUrl(event.target.value)}
            placeholder="https://203.0.113.20:8443"
            className="font-mono text-[13px]"
            spellCheck={false}
            autoComplete="off"
          />
          <Input
            label={t("İkincil panelin API anahtarı")}
            type="password"
            value={token}
            onChange={(event) => setToken(event.target.value)}
            placeholder="pix_…"
            hint={t("İkincil panelde Hesabım → API anahtarları bölümünden üretilir. İzinli adres olarak bu sunucunun adresini yazmanız önerilir.")}
            className="font-mono text-[13px]"
            autoComplete="new-password"
            required={panelUrl.trim() !== ""}
          />
        </fieldset>
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

function SecondaryCard({ item, onRemove, removing }: { item: DnsClusterSecondary; onRemove: () => void; removing: boolean }) {
  const hasPanel = item.panel_url !== "";
  return (
    <li className="flex flex-col gap-2 border-b border-line px-4 py-3 last:border-b-0">
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-1">
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
          <Mono className="font-medium">{item.address}</Mono>
          {!hasPanel ? (
            <Status tone="idle">{t("Yalnızca bölge aktarımı")}</Status>
          ) : item.last_error !== "" ? (
            <Status tone="bad">{t("Eşitlenemedi")}</Status>
          ) : item.conflicts.length > 0 ? (
            <Status tone="warn">{t("Kısmen eşitlendi")}</Status>
          ) : (
            <Status tone="ok">{t("Eşit")}</Status>
          )}
        </div>
        <IconButton label={t("{address} ikincilini kaldır", { address: item.address })} onClick={onRemove} disabled={removing}>
          <Trash2 className="size-4" />
        </IconButton>
      </div>
      {hasPanel ? (
        <dl className="grid gap-x-6 gap-y-1 text-[13px] sm:grid-cols-[auto_1fr]">
          <dt className="text-ink-3">{t("Panel")}</dt>
          <dd className="min-w-0 break-all font-mono">{item.panel_url}</dd>
          <dt className="text-ink-3">{t("Son eşitleme")}</dt>
          <dd>
            {item.last_sync_at === null
              ? t("Henüz yapılmadı")
              : t("{when}, {count} bölge", { when: formatRelative(item.last_sync_at), count: item.synced_zones ?? 0 })}
          </dd>
          <dt className="text-ink-3">{t("Sertifika parmak izi")}</dt>
          <dd className="min-w-0 break-all font-mono text-[12px] text-ink-2">{groupFingerprint(item.fingerprint)}</dd>
        </dl>
      ) : (
        <p className="text-[13px] text-ink-2">
          {t("Bu sunucu bölgeleri çekebilir; her bölgeyi orada ikincil olarak sizin tanımlamanız gerekir (birincil adresi olarak bu sunucuyu gösterin).")}
        </p>
      )}
      {item.last_error !== "" && <ErrorBlock message={item.last_error} />}
      {item.conflicts.length > 0 && (
        <p className="rounded-md bg-sunken px-3 py-2 text-[13px] text-ink-2">
          {t("İkincilde aynı adla başka bir bölge bulunduğu için açılamayanlar: {names}", { names: item.conflicts.join(", ") })}
        </p>
      )}
    </li>
  );
}

export function DnsCluster({ localInstalled }: { localInstalled: boolean }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [adding, setAdding] = useState(false);
  const cluster = useQuery({ queryKey: ["dns-cluster"], queryFn: () => api.get<ClusterData>("/dns/cluster"), enabled: localInstalled });

  const save = useMutation({
    mutationFn: (secondaries: SecondaryBody[]) => api.put<ClusterData>("/dns/cluster", { secondaries }),
    onSuccess: (updated) => queryClient.setQueryData(["dns-cluster"], updated),
    onError: (error) => feedback.error(error),
  });

  const sync = useMutation({
    mutationFn: () => api.post<ClusterData>("/dns/cluster/sync"),
    onSuccess: (updated) => {
      queryClient.setQueryData(["dns-cluster"], updated);
      const failed = updated.secondaries.filter((item) => item.last_error !== "").length;
      if (failed === 0) feedback.success(t("Bölge listesi ikincillere iletildi"));
      else feedback.error(new Error(t("{count} ikincil eşitlenemedi", { count: failed })));
    },
    onError: (error) => feedback.error(error),
  });

  const detach = useMutation({
    mutationFn: (primary: string) => api.delete<ClusterData>(`/dns/replica/${encodeURIComponent(primary)}`),
    onSuccess: (updated) => {
      queryClient.setQueryData(["dns-cluster"], updated);
      feedback.success(t("Birincille bağlantı kesildi"));
    },
    onError: (error) => feedback.error(error),
  });

  if (!localInstalled) {
    return (
      <Panel>
        <EmptyState
          title={t("DNS sunucusu kurulu değil")}
          description={t("Küme, bu sunucudaki DNS sunucusunun bölgelerini ikincil ad sunucularına eşitler. Önce Bileşenler sayfasından DNS sunucusunu kurun.")}
        />
      </Panel>
    );
  }
  if (cluster.isPending) return <LoadingBlock />;
  if (cluster.isError) return <ErrorBlock message={errorMessage(cluster.error)} onRetry={() => void cluster.refetch()} />;
  const data = cluster.data;

  async function confirmRemove(item: DnsClusterSecondary) {
    const confirmed = await feedback.confirm({
      title: t("{address} ikincil ad sunucularından çıkarılsın mı?", { address: item.address }),
      description: t("Bu sunucu ona artık bildirim göndermez ve bölge aktarımına izin vermez. İkincildeki bölgeler kendiliğinden silinmez; orada birincille bağlantıyı kesin."),
      confirmLabel: t("İkincili kaldır"),
      danger: true,
    });
    if (confirmed) save.mutate(data.secondaries.filter((other) => other.address !== item.address).map(toBody));
  }

  async function confirmDetach(primary: string, zones: number) {
    const confirmed = await feedback.confirm({
      title: t("{address} ile bağlantı kesilsin mi?", { address: primary }),
      description: t("Bu sunucunun o birincilden çektiği {count} bölge silinir ve bildirimleri artık kabul edilmez.", { count: zones }),
      confirmLabel: t("Bağlantıyı kes"),
      danger: true,
    });
    if (confirmed) detach.mutate(primary);
  }

  const hasPanel = data.secondaries.some((item) => item.panel_url !== "");

  return (
    <div className="flex flex-col gap-4">
      <Panel
        flush
        title={t("İkincil ad sunucuları")}
        actions={
          <>
            {hasPanel && (
              <Button size="sm" icon={<RefreshCw className="size-3.5" />} onClick={() => sync.mutate()} loading={sync.isPending}>
                {t("Şimdi eşitle")}
              </Button>
            )}
            <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setAdding(true)}>
              {t("İkincil ekle")}
            </Button>
          </>
        }
      >
        {data.secondaries.length === 0 ? (
          <EmptyState
            title={t("İkincil ad sunucusu yok")}
            description={t("Alan adlarının çoğu en az iki ad sunucusu ister. İkincil eklerseniz bu sunucu kapansa bile alan adlarınız çözülmeye devam eder; bölgelerdeki her değişiklik ikincile kendiliğinden ulaşır.")}
            action={
              <Button variant="primary" onClick={() => setAdding(true)}>
                {t("İkincil ekle")}
              </Button>
            }
          />
        ) : (
          <>
            <ul>
              {data.secondaries.map((item) => (
                <SecondaryCard key={item.address} item={item} onRemove={() => void confirmRemove(item)} removing={save.isPending} />
              ))}
            </ul>
            <p className="border-t border-line px-4 py-3 text-[13px] text-ink-2">
              {t("Alan adlarınızın ad sunucusu kayıtlarında ikincilin de yer aldığından emin olun. Bölgeye özel ad sunucusu kullanan bölgelerde ns2 kaydı ilk ikincilin adresini gösterir.")}
            </p>
          </>
        )}
      </Panel>

      {data.replica_of.length > 0 && (
        <Panel flush title={t("Bu sunucunun ikincili olduğu sunucular")}>
          <ul>
            {data.replica_of.map((item) => (
              <li key={item.primary} className="flex flex-wrap items-center justify-between gap-3 border-b border-line px-4 py-3 last:border-b-0">
                <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                  <Mono className="font-medium">{item.primary}</Mono>
                  <span className="text-[13px] text-ink-2">{t("{count} bölge çekiliyor", { count: item.zones })}</span>
                </div>
                <Button size="sm" onClick={() => void confirmDetach(item.primary, item.zones)} loading={detach.isPending}>
                  {t("Bağlantıyı kes")}
                </Button>
              </li>
            ))}
          </ul>
        </Panel>
      )}
      {adding && <AddModal current={data.secondaries} onClose={() => setAdding(false)} />}
    </div>
  );
}
