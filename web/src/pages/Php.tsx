// PHP sayfası: sürüm başına sunucu geneli varsayılanlar ve eklentiler.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useId, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useFeedback } from "../components/feedback";
import {
  Button,
  Checkbox,
  EmptyState,
  ErrorBlock,
  IconButton,
  Input,
  LoadingBlock,
  Modal,
  Mono,
  PageHeader,
  Panel,
  SearchInput,
  Status,
  Tabs,
} from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

// ---- Tipler ----

interface PhpDirective {
  key: string;
  value: string;
}

interface PhpGlobal {
  memory_limit: string;
  upload_max_filesize: string;
  post_max_size: string;
  max_execution_time: number;
  max_input_vars: number;
  display_errors: boolean;
  timezone: string;
  opcache: boolean;
  opcache_memory: number;
  custom: PhpDirective[];
}

interface PhpVersion {
  version: string;
  running: boolean;
  site_count: number;
  settings: PhpGlobal;
}

interface PhpExtensions {
  installed: { name: string; enabled: boolean }[];
  installable: string[];
}

const commonTimezones = [
  "Europe/Istanbul",
  "UTC",
  "Europe/London",
  "Europe/Berlin",
  "Europe/Paris",
  "Europe/Moscow",
  "America/New_York",
  "America/Chicago",
  "America/Los_Angeles",
  "Asia/Dubai",
  "Asia/Tokyo",
  "Asia/Singapore",
  "Australia/Sydney",
];

// ---- Küçük bileşenler ----

/** Erişilebilir aç/kapa anahtarı. */
function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={classNames(
        "relative inline-flex h-5 w-9 shrink-0 items-center rounded-full border transition-colors",
        "disabled:cursor-not-allowed disabled:opacity-55",
        checked ? "border-transparent bg-accent" : "border-line-strong bg-sunken",
      )}
    >
      <span
        aria-hidden
        className={classNames("size-3.5 rounded-full bg-white shadow-sm transition-transform", checked ? "translate-x-[18px]" : "translate-x-[3px]")}
      />
    </button>
  );
}

// ---- Genel ayarlar formu ----

function GlobalSettingsForm({ version, initial }: { version: string; initial: PhpGlobal }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const timezoneListId = useId();
  const [settings, setSettings] = useState<PhpGlobal>(initial);

  const set = <K extends keyof PhpGlobal>(key: K, value: PhpGlobal[K]) => setSettings((current) => ({ ...current, [key]: value }));
  const setDirective = (index: number, field: "key" | "value", value: string) =>
    set(
      "custom",
      settings.custom.map((directive, i) => (i === index ? { ...directive, [field]: value } : directive)),
    );

  const save = useMutation({
    mutationFn: () => api.put<PhpGlobal>(`/php/${version}/settings`, settings),
    onSuccess: () => {
      feedback.success(t("PHP {version} ayarları kaydedildi", { version }));
      void queryClient.invalidateQueries({ queryKey: ["php"] });
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <div className="grid gap-4 sm:grid-cols-2">
        <Input label={t("Bellek sınırı")} value={settings.memory_limit} onChange={(e) => set("memory_limit", e.target.value)} hint="memory_limit" className="font-mono text-[13.5px]" />
        <Input
          label={t("Çalışma süresi (saniye)")}
          type="number"
          min={1}
          value={settings.max_execution_time}
          onChange={(e) => set("max_execution_time", Number(e.target.value))}
          hint="max_execution_time"
        />
        <Input
          label={t("En büyük yükleme boyutu")}
          value={settings.upload_max_filesize}
          onChange={(e) => set("upload_max_filesize", e.target.value)}
          hint="upload_max_filesize"
          className="font-mono text-[13.5px]"
        />
        <Input label={t("En büyük POST boyutu")} value={settings.post_max_size} onChange={(e) => set("post_max_size", e.target.value)} hint="post_max_size" className="font-mono text-[13.5px]" />
        <Input
          label={t("En fazla form alanı")}
          type="number"
          min={100}
          value={settings.max_input_vars}
          onChange={(e) => set("max_input_vars", Number(e.target.value))}
          hint="max_input_vars"
        />
        <div>
          <Input
            label={t("Saat dilimi")}
            value={settings.timezone}
            onChange={(e) => set("timezone", e.target.value)}
            hint="date.timezone"
            list={timezoneListId}
            spellCheck={false}
            autoComplete="off"
            className="font-mono text-[13.5px]"
          />
          <datalist id={timezoneListId}>
            {commonTimezones.map((zone) => (
              <option key={zone} value={zone} />
            ))}
          </datalist>
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <Checkbox
          label={t("Hataları sayfada göster")}
          hint={t("display_errors. Canlı sunucuda kapalı tutun.")}
          checked={settings.display_errors}
          onChange={(e) => set("display_errors", e.target.checked)}
        />
        <Checkbox
          label={t("OPcache açık")}
          hint={t("opcache.enable. Derlenmiş kodu bellekte tutarak siteleri hızlandırır.")}
          checked={settings.opcache}
          onChange={(e) => set("opcache", e.target.checked)}
        />
        <div className="max-w-60">
          <Input
            label={t("OPcache belleği (MB)")}
            type="number"
            min={8}
            value={settings.opcache_memory}
            onChange={(e) => set("opcache_memory", Number(e.target.value))}
            hint="opcache.memory_consumption"
            disabled={!settings.opcache}
          />
        </div>
      </div>

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Ek php.ini yönergeleri")}</legend>
        {settings.custom.map((directive, index) => (
          <div key={index} className="flex items-center gap-2">
            <input
              aria-label={t("Yönerge adı")}
              placeholder="realpath_cache_size"
              value={directive.key}
              onChange={(e) => setDirective(index, "key", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]"
              spellCheck={false}
            />
            <input
              aria-label={t("Değer")}
              placeholder="4M"
              value={directive.value}
              onChange={(e) => setDirective(index, "value", e.target.value)}
              className="h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]"
              spellCheck={false}
            />
            <IconButton
              label={t("Yönergeyi kaldır")}
              onClick={() =>
                set(
                  "custom",
                  settings.custom.filter((_, i) => i !== index),
                )
              }
            >
              <Trash2 className="size-4" />
            </IconButton>
          </div>
        ))}
        <div>
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => set("custom", [...settings.custom, { key: "", value: "" }])}>
            {t("Yönerge ekle")}
          </Button>
        </div>
      </fieldset>

      {save.isError && <ErrorBlock message={errorMessage(save.error)} />}

      <div className="flex flex-col gap-2 border-t border-line pt-4">
        <p className="text-[13px] text-ink-3">
          {t("Kendi PHP ayarlarında değer belirleyen siteler onu korur; diğerleri yeni değerleri hemen devralır. Kaydetme, bu sürümü kullanan sitelerin havuzlarını yeniden oluşturduğu için birkaç saniye sürebilir.")}
        </p>
        <div>
          <Button type="submit" variant="primary" loading={save.isPending}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </div>
      </div>
    </form>
  );
}

// ---- Eklentiler ----

function ExtensionsPanel({ version }: { version: string }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [search, setSearch] = useState("");
  const [watching, setWatching] = useState<{ jobId: number; name: string } | null>(null);
  const queryKey = ["php-extensions", version];

  const extensions = useQuery({
    queryKey,
    queryFn: () => api.get<PhpExtensions>(`/php/${version}/extensions`),
  });

  const toggle = useMutation({
    mutationFn: (input: { name: string; enabled: boolean }) =>
      api.post(`/php/${version}/extensions/${encodeURIComponent(input.name)}`, { enabled: input.enabled }),
    onError: (error) => feedback.error(error),
    onSettled: () => queryClient.invalidateQueries({ queryKey }),
  });

  const install = useMutation({
    mutationFn: (name: string) => api.post<{ job_id: number }>(`/php/${version}/extensions/${encodeURIComponent(name)}/install`),
    onSuccess: (result, name) => {
      setWatching({ jobId: result.job_id, name });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });

  function refresh() {
    void queryClient.invalidateQueries({ queryKey });
  }

  const needle = search.trim().toLowerCase();
  const installed = (extensions.data?.installed ?? []).filter((item) => item.name.toLowerCase().includes(needle));
  const installable = (extensions.data?.installable ?? []).filter((name) => name.toLowerCase().includes(needle));

  return (
    <Panel
      flush
      title={t("Eklentiler")}
      actions={<SearchInput aria-label={t("Eklenti ara")} placeholder={t("Eklenti ara")} value={search} onChange={(e) => setSearch(e.target.value)} />}
    >
      {extensions.isPending && <LoadingBlock />}
      {extensions.isError && (
        <div className="p-4">
          <ErrorBlock message={errorMessage(extensions.error)} onRetry={() => void extensions.refetch()} />
        </div>
      )}
      {extensions.data && (
        <>
          {installed.length === 0 ? (
            <p className="px-4 sm:px-6 py-6 text-ink-3">{needle ? t("Aramayla eşleşen eklenti yok.") : t("Kurulu eklenti yok.")}</p>
          ) : (
            <ul className="max-h-[28rem] overflow-y-auto">
              {installed.map((extension) => (
                <li key={extension.name} className="flex items-center justify-between gap-3 border-b border-line px-4 sm:px-6 py-2 last:border-b-0">
                  <Mono>{extension.name}</Mono>
                  <div className="flex items-center gap-2.5">
                    <span className="w-14 text-right text-[13px] text-ink-3">{extension.enabled ? t("Açık") : t("Kapalı")}</span>
                    <Switch
                      checked={extension.enabled}
                      label={t("{name} eklentisi", { name: extension.name })}
                      disabled={toggle.isPending && toggle.variables?.name === extension.name}
                      onChange={(next) => toggle.mutate({ name: extension.name, enabled: next })}
                    />
                  </div>
                </li>
              ))}
            </ul>
          )}

          {extensions.data.installable.length > 0 && (
            <div className="border-t border-line">
              <h3 className="bg-sunken/60 px-4 sm:px-6 py-2 text-[13px] font-medium text-ink-2">{t("Kurulabilir eklentiler")}</h3>
              {installable.length === 0 ? (
                <p className="px-4 sm:px-6 py-4 text-ink-3">{t("Aramayla eşleşen eklenti yok.")}</p>
              ) : (
                <ul className="max-h-64 overflow-y-auto">
                  {installable.map((name) => (
                    <li key={name} className="flex items-center justify-between gap-3 border-b border-line px-4 sm:px-6 py-1.5 last:border-b-0">
                      <Mono>{name}</Mono>
                      <Button
                        size="sm"
                        icon={<Download className="size-3.5" />}
                        loading={install.isPending && install.variables === name}
                        onClick={() => install.mutate(name)}
                      >
                        {t("Kur")}
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </>
      )}

      <Modal
        open={watching !== null}
        onClose={() => {
          setWatching(null);
          refresh();
        }}
        title={t("{name} eklentisi kurulumu", { name: watching?.name ?? "" })}
        wide
      >
        {watching && <JobLog jobId={watching.jobId} onDone={refresh} />}
      </Modal>
    </Panel>
  );
}

// ---- Sayfa ----

export function PhpPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const versions = useQuery({ queryKey: ["php"], queryFn: () => api.get<PhpVersion[]>("/php") });

  const list = versions.data ?? [];
  const requested = searchParams.get("surum");
  const current = list.find((item) => item.version === requested) ?? list[0];

  return (
    <>
      <PageHeader
        title={t("PHP")}
        description={t("Her PHP sürümü için sunucu geneli varsayılanlar. Siteler, kendi PHP ayarlarında bu değerlerden tek tek ayrılabilir.")}
      />
      {versions.isPending && <LoadingBlock />}
      {versions.isError && <ErrorBlock message={errorMessage(versions.error)} onRetry={() => void versions.refetch()} />}
      {versions.data && list.length === 0 && (
        <Panel>
          <EmptyState
            title={t("Kurulu PHP sürümü yok")}
            description={t("PHP sitelerini çalıştırmak için önce bir PHP sürümü kurun.")}
            action={
              <Link to="/components" className="font-medium text-accent hover:underline">
                {t("Bileşenlere git")}
              </Link>
            }
          />
        </Panel>
      )}
      {current && (
        <>
          <Tabs
            label={t("PHP sürümleri")}
            tabs={list.map((item) => ({ id: item.version, label: `PHP ${item.version}` }))}
            value={current.version}
            onChange={(next) => setSearchParams({ surum: next }, { replace: true })}
          />
          <div className="mb-4 flex flex-wrap items-center gap-x-6 gap-y-1">
            {current.running ? <Status tone="ok">{t("Çalışıyor")}</Status> : <Status tone="warn">{t("Durdu")}</Status>}
            <span className="text-ink-2">{t("{n} site bu sürümü kullanıyor", { n: current.site_count })}</span>
          </div>
          <div className="grid items-start gap-4 xl:grid-cols-2">
            <Panel title={t("Genel ayarlar")}>
              <GlobalSettingsForm key={current.version + JSON.stringify(current.settings)} version={current.version} initial={current.settings} />
            </Panel>
            <ExtensionsPanel key={current.version} version={current.version} />
          </div>
        </>
      )}
    </>
  );
}
