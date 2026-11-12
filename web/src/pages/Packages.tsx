// Hizmet paketleri: bir hesabın kaç site, veritabanı, posta kutusu... açabileceğini
// ve ne kadar disk kullanabileceğini belirler. Bu dosyada paket listesi, paket
// düzenleme penceresi, hesaba paket seçtiren alan ve kullanım özeti bulunur.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Meter, Modal, Panel, Select, Table, Td, Th } from "../components/ui";
import type { AccountUsage, PackageInfo, PackageLimits } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatRelative } from "../lib/format";
import { t } from "../lib/i18n";

/** Sınır alanları, formda ve tabloda gösterildiği sırayla. */
const LIMIT_FIELDS: { key: keyof PackageLimits; label: string; unit?: string }[] = [
  { key: "sites", label: "Site" },
  { key: "databases", label: "Veritabanı" },
  { key: "mail_domains", label: "Posta alan adı" },
  { key: "mailboxes", label: "Posta kutusu" },
  { key: "ftp_accounts", label: "FTP hesabı" },
  { key: "dns_zones", label: "DNS bölgesi" },
  { key: "disk_mb", label: "Disk", unit: "MB" },
];

/** İşlemci, bellek ve süreç sınırları; aralıklar sunucudaki doğrulamayla aynıdır. */
const RESOURCE_FIELDS: { key: keyof PackageLimits; label: string; unit: string; min: number; max: number; hint: string }[] = [
  {
    key: "cpu_percent",
    label: "İşlemci",
    unit: "%",
    min: 10,
    max: 3200,
    hint: "Bir çekirdeğin yüzdesi: 100 = bir tam çekirdek, 250 = iki buçuk çekirdek.",
  },
  {
    key: "memory_mb",
    label: "Bellek (RAM)",
    unit: "MB",
    min: 64,
    max: 262144,
    hint: "Hesabın bütün süreçlerinin (SSH, zamanlı görevler, Redis) toplam sınırı. PHP sitelerinde her sürecin memory_limit değeri bu sınıra kırpılır.",
  },
  {
    key: "php_workers",
    label: "PHP işçisi (site başına)",
    unit: "",
    min: 1,
    max: 200,
    hint: "Bir sitenin aynı anda çalıştırabileceği en çok PHP süreci (pm.max_children).",
  },
  {
    key: "processes",
    label: "Süreç sayısı",
    unit: "",
    min: 20,
    max: 10000,
    hint: "Hesabın en çok kaç süreç ve iş parçacığı çalıştırabileceği.",
  },
];

const ALL_FIELDS: { key: keyof PackageLimits }[] = [...LIMIT_FIELDS, ...RESOURCE_FIELDS];

function limitText(value: number | null, unit?: string): string {
  if (value === null) return t("Sınırsız");
  if (unit === "MB") return formatBytes(value * 1024 * 1024, 0);
  return String(value);
}

/** Paketin kaynak sınırlarını tek satırda özetler; hiçbiri yoksa "Sınırsız". */
function resourceSummary(limits: PackageLimits): string {
  const parts: string[] = [];
  if (limits.cpu_percent !== null) parts.push(t("İşlemci %{n}", { n: limits.cpu_percent }));
  if (limits.memory_mb !== null) parts.push(t("Bellek {size}", { size: formatBytes(limits.memory_mb * 1024 * 1024, 0) }));
  if (limits.processes !== null) parts.push(t("{n} süreç", { n: limits.processes }));
  if (limits.php_workers !== null) parts.push(t("{n} PHP işçisi", { n: limits.php_workers }));
  return parts.length === 0 ? t("Sınırsız") : parts.join(" · ");
}

function PackageModal({ pack, onClose }: { pack: PackageInfo | null; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [name, setName] = useState(pack?.name ?? "");
  // Boş kutu "sınırsız" demektir; bu yüzden değerler metin olarak tutulur.
  const [values, setValues] = useState<Record<string, string>>(() =>
    Object.fromEntries(ALL_FIELDS.map((field) => [field.key, pack?.limits[field.key]?.toString() ?? ""])),
  );
  const support = useQuery({ queryKey: ["packages", "limits-support"], queryFn: () => api.get<{ enforced: boolean; warning: string | null }>("/packages/limits-support") });

  const save = useMutation({
    mutationFn: () => {
      const limits = Object.fromEntries(ALL_FIELDS.map((field) => [field.key, values[field.key].trim() === "" ? null : Number(values[field.key])]));
      const body = { name, limits };
      return pack ? api.patch(`/packages/${pack.id}`, body) : api.post("/packages", body);
    },
    onSuccess: () => {
      feedback.success(pack ? t("{name} paketi güncellendi", { name }) : t("{name} paketi oluşturuldu", { name }));
      void queryClient.invalidateQueries({ queryKey: ["packages"] });
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
      title={pack ? t("{name} paketini düzenle", { name: pack.name }) : t("Paket oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="package-form" loading={save.isPending}>
            {pack ? t("Değişiklikleri kaydet") : t("Paketi oluştur")}
          </Button>
        </>
      }
    >
      <form id="package-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input label={t("Paket adı")} value={name} onChange={(event) => setName(event.target.value)} placeholder={t("Başlangıç")} maxLength={60} autoFocus required />
        <fieldset>
          <legend className="text-[13.5px] font-medium text-ink-2">{t("Sınırlar")}</legend>
          <p className="mt-0.5 mb-3 text-[13px] text-ink-3">{t("Boş bıraktığınız alan sınırsız olur. 0 yazarsanız o kaynak pakette hiç bulunmaz.")}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            {LIMIT_FIELDS.map((field) => (
              <Input
                key={field.key}
                label={field.unit ? `${t(field.label)} (${field.unit})` : t(field.label)}
                type="number"
                min={0}
                inputMode="numeric"
                value={values[field.key]}
                onChange={(event) => setValues((current) => ({ ...current, [field.key]: event.target.value }))}
                placeholder={t("Sınırsız")}
              />
            ))}
          </div>
        </fieldset>
        <fieldset>
          <legend className="text-[13.5px] font-medium text-ink-2">{t("İşlemci ve bellek")}</legend>
          <p className="mt-0.5 mb-3 text-[13px] text-ink-3">{t("Hesabın sunucuda kullanabileceği işlemci ve bellek. Boş bıraktığınız alan sınırsız olur.")}</p>
          <div className="grid gap-3 sm:grid-cols-2">
            {RESOURCE_FIELDS.map((field) => (
              <Input
                key={field.key}
                label={field.unit ? `${t(field.label)} (${field.unit})` : t(field.label)}
                type="number"
                min={field.min}
                max={field.max}
                inputMode="numeric"
                value={values[field.key]}
                onChange={(event) => setValues((current) => ({ ...current, [field.key]: event.target.value }))}
                placeholder={t("Sınırsız")}
                hint={`${t(field.hint)} ${t("{min}–{max} arası.", { min: field.min, max: field.max })}`}
              />
            ))}
          </div>
          <p className="mt-3 text-[13px] text-ink-3">
            {t("Node.js ve Python uygulamaları ile hesabın Redis'i, sınır değiştikten sonra bir sonraki yeniden başlatılmalarında hesabın toplam sınırına dahil olur.")}
          </p>
          {support.data && !support.data.enforced && (
            <p className="mt-2 rounded-md bg-warn-soft px-3 py-2 text-[13.5px] text-warn">
              {t(support.data.warning ?? "Bu sunucuda işlemci ve bellek sınırları uygulanamıyor.")}
            </p>
          )}
        </fieldset>
        {pack && pack.accounts > 0 && (
          <p className="rounded-md bg-sunken px-3 py-2 text-[13.5px] text-ink-2">
            {t("Bu paketi {count} hesap kullanıyor. Sınırı düşürürseniz var olan kaynaklar silinmez; yalnızca yenisi açılamaz.", { count: pack.accounts })}
          </p>
        )}
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

export function PackagesPanel() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [editing, setEditing] = useState<PackageInfo | "new" | null>(null);
  const packages = useQuery({ queryKey: ["packages"], queryFn: () => api.get<PackageInfo[]>("/packages") });

  const remove = useMutation({
    mutationFn: (pack: PackageInfo) => api.delete(`/packages/${pack.id}`),
    onSuccess: (_, pack) => feedback.success(t("{name} paketi silindi", { name: pack.name })),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["packages"] }),
  });

  async function confirmRemove(pack: PackageInfo) {
    const confirmed = await feedback.confirm({
      title: t("{name} paketi silinsin mi?", { name: pack.name }),
      confirmLabel: t("Paketi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(pack);
  }

  if (packages.isPending) return <LoadingBlock />;
  if (packages.isError) return <ErrorBlock message={errorMessage(packages.error)} onRetry={() => void packages.refetch()} />;

  return (
    <>
      <Panel
        flush
        title={t("Paketler")}
        actions={
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setEditing("new")}>
            {t("Paket oluştur")}
          </Button>
        }
      >
        {packages.data.length === 0 ? (
          <EmptyState
            title={t("Henüz paket yok")}
            description={t("Paket, bir hesabın kaç site, veritabanı ve posta kutusu açabileceğini ve ne kadar disk kullanabileceğini belirler. Paketsiz hesapların sınırı yoktur.")}
            action={
              <Button variant="primary" onClick={() => setEditing("new")}>
                {t("Paket oluştur")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Paket")}</Th>
                {LIMIT_FIELDS.map((field) => (
                  <Th key={field.key} align="right">
                    {t(field.label)}
                  </Th>
                ))}
                <Th>{t("İşlemci ve bellek")}</Th>
                <Th align="right">{t("Hesap")}</Th>
                <Th align="right">{t("İşlemler")}</Th>
              </tr>
            </thead>
            <tbody>
              {packages.data.map((pack) => (
                <tr key={pack.id} className="hover:bg-sunken/50">
                  <Td>
                    <span className="font-medium">{pack.name}</span>
                  </Td>
                  {LIMIT_FIELDS.map((field) => (
                    <Td key={field.key} align="right" className={pack.limits[field.key] === null ? "text-ink-3" : undefined}>
                      {limitText(pack.limits[field.key], field.unit)}
                    </Td>
                  ))}
                  <Td className={resourceSummary(pack.limits) === t("Sınırsız") ? "text-ink-3" : undefined}>{resourceSummary(pack.limits)}</Td>
                  <Td align="right">{pack.accounts}</Td>
                  <Td align="right" className="py-0.5">
                    <div className="flex justify-end gap-0.5">
                      <IconButton label={t("{name} paketini düzenle", { name: pack.name })} onClick={() => setEditing(pack)}>
                        <Pencil className="size-4" />
                      </IconButton>
                      <IconButton
                        label={pack.accounts > 0 ? t("Kullanılan paket silinemez") : t("{name} paketini sil", { name: pack.name })}
                        disabled={pack.accounts > 0}
                        onClick={() => void confirmRemove(pack)}
                      >
                        <Trash2 className="size-4" />
                      </IconButton>
                    </div>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </Panel>
      {editing && <PackageModal pack={editing === "new" ? null : editing} onClose={() => setEditing(null)} />}
    </>
  );
}

/** Hesaba paket seçtiren alan. Değer paket kimliğidir; 0 "paket yok" demektir. */
export function PackageSelect({ value, onChange }: { value: number; onChange: (next: number) => void }) {
  const packages = useQuery({ queryKey: ["packages"], queryFn: () => api.get<PackageInfo[]>("/packages") });
  return (
    <Select
      label={t("Paket")}
      value={value}
      onChange={(event) => onChange(Number(event.target.value))}
      hint={packages.data?.length === 0 ? t("Henüz paket tanımlanmamış; Paketler sekmesinden oluşturabilirsiniz.") : t("Paket, hesabın açabileceği kaynakları sınırlar.")}
    >
      <option value={0}>{t("Paket yok (sınırsız)")}</option>
      {packages.data?.map((pack) => (
        <option key={pack.id} value={pack.id}>
          {pack.name}
        </option>
      ))}
    </Select>
  );
}

const USAGE_COUNTS: { key: "sites" | "databases" | "mail_domains" | "mailboxes" | "ftp_accounts" | "dns_zones"; label: string }[] = [
  { key: "sites", label: "Site" },
  { key: "databases", label: "Veritabanı" },
  { key: "mail_domains", label: "Posta alan adı" },
  { key: "mailboxes", label: "Posta kutusu" },
  { key: "ftp_accounts", label: "FTP hesabı" },
  { key: "dns_zones", label: "DNS bölgesi" },
];

function UsageRow({ label, used, limit, text }: { label: string; used: number; limit: number | null; text: string }) {
  return (
    <div>
      <div className="flex items-baseline justify-between gap-3">
        <dt className="text-ink-2">{label}</dt>
        <dd className="tabular font-mono text-[13.5px]">{text}</dd>
      </div>
      {limit !== null && limit > 0 && (
        <div className="mt-1">
          <Meter value={(used / limit) * 100} label={label} />
        </div>
      )}
    </div>
  );
}

/** İşlemci, bellek ve süreç sınırları ile (varsa) anlık kullanım. Hiç kaynak sınırı yoksa hiçbir şey göstermez. */
function ResourceUsage({ limits, runtime }: { limits: PackageLimits; runtime: AccountUsage["runtime"] }) {
  if (limits.cpu_percent === null && limits.memory_mb === null && limits.processes === null && limits.php_workers === null) return null;
  const memoryLimit = limits.memory_mb === null ? null : limits.memory_mb * 1024 * 1024;
  return (
    <div className="mt-4 border-t border-line pt-3">
      <h3 className="mb-3 text-[13.5px] font-medium text-ink-2">{t("İşlemci ve bellek")}</h3>
      {runtime && !runtime.enforced && (
        <p className="mb-3 rounded-md bg-warn-soft px-3 py-2 text-[13.5px] text-warn">{t(runtime.warning ?? "Bu sunucuda işlemci ve bellek sınırları uygulanamıyor.")}</p>
      )}
      <dl className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
        {limits.cpu_percent !== null && (
          <UsageRow
            label={t("İşlemci")}
            used={runtime?.cpu_percent ?? 0}
            limit={limits.cpu_percent}
            text={
              runtime?.cpu_percent != null
                ? `%${runtime.cpu_percent.toFixed(0)} / %${limits.cpu_percent}`
                : t("En çok %{n} (bir çekirdeğin yüzdesi)", { n: limits.cpu_percent })
            }
          />
        )}
        {memoryLimit !== null && (
          <UsageRow
            label={t("Bellek (RAM)")}
            used={runtime?.memory_bytes ?? 0}
            limit={memoryLimit}
            text={runtime?.memory_bytes != null ? `${formatBytes(runtime.memory_bytes)} / ${formatBytes(memoryLimit, 0)}` : t("En çok {size}", { size: formatBytes(memoryLimit, 0) })}
          />
        )}
        {limits.processes !== null && (
          <UsageRow
            label={t("Süreç sayısı")}
            used={runtime?.tasks ?? 0}
            limit={limits.processes}
            text={runtime?.tasks != null ? `${runtime.tasks} / ${limits.processes}` : t("En çok {n}", { n: limits.processes })}
          />
        )}
        {limits.php_workers !== null && (
          <UsageRow label={t("PHP işçisi (site başına)")} used={0} limit={null} text={t("En çok {n}", { n: limits.php_workers })} />
        )}
      </dl>
      {limits.memory_mb !== null && <p className="mt-3 text-[13px] text-ink-3">{t("PHP sitelerinde memory_limit değeri paketin bellek sınırını aşamaz; aşıyorsa sınıra düşürülür.")}</p>}
      <p className="mt-2 text-[13px] text-ink-3">{t("Kullanım, hesabın SSH oturumları, zamanlı görevleri ve (sınır uygulandıysa) Redis ile uygulamalarının toplamıdır; hesap etkin bir süreç çalıştırmıyorsa boş görünür.")}</p>
    </div>
  );
}

/** Bir hesabın paketine göre kullanımı. `path`: kullanımın okunacağı uç. */
export function UsagePanel({ path, title }: { path: string; title?: string }) {
  const usage = useQuery({ queryKey: ["usage", path], queryFn: () => api.get<AccountUsage>(path) });
  if (usage.isPending) return <LoadingBlock />;
  if (usage.isError) return null;
  const data = usage.data;
  const diskLimit = data.limits.disk_mb === null ? null : data.limits.disk_mb * 1024 * 1024;

  return (
    <Panel title={title ?? t("Paket ve kullanım")}>
      <p className="mb-4 text-ink-2">
        {data.package ? (
          <>
            {t("Paketiniz")}: <span className="font-medium text-ink">{data.package}</span>
          </>
        ) : (
          t("Bu hesaba paket atanmamış; kaynak sınırı yok.")
        )}
      </p>
      <dl className="grid gap-x-8 gap-y-4 sm:grid-cols-2">
        {USAGE_COUNTS.map((item) => {
          const limit = data.limits[item.key];
          return (
            <UsageRow key={item.key} label={t(item.label)} used={data[item.key]} limit={limit} text={limit === null ? String(data[item.key]) : `${data[item.key]} / ${limit}`} />
          );
        })}
        <UsageRow
          label={t("Disk")}
          used={data.disk_bytes ?? 0}
          limit={diskLimit}
          text={
            data.disk_bytes === null
              ? t("Henüz ölçülmedi")
              : diskLimit === null
                ? formatBytes(data.disk_bytes)
                : `${formatBytes(data.disk_bytes)} / ${formatBytes(diskLimit, 0)}`
          }
        />
      </dl>
      <ResourceUsage limits={data.limits} runtime={data.runtime} />
      {data.database_bytes !== null && (
        <p className="mt-4 flex items-baseline justify-between gap-3 border-t border-line pt-3">
          <span className="text-ink-2">{t("Disk kullanımının veritabanları payı")}</span>
          <span className="tabular font-mono text-[13.5px]">{formatBytes(data.database_bytes)}</span>
        </p>
      )}
      <p className="mt-4 flex items-baseline justify-between gap-3 border-t border-line pt-3">
        <span className="text-ink-2">{t("Aylık trafik")}</span>
        <span className="tabular font-mono text-[13.5px]">{formatBytes(data.traffic_bytes)}</span>
      </p>
      {data.disk_measured_at !== null && (
        <p className="mt-3 text-[13px] text-ink-3">{t("Disk kullanımı saatte bir ölçülür; son ölçüm {when}.", { when: formatRelative(data.disk_measured_at) })}</p>
      )}
    </Panel>
  );
}
