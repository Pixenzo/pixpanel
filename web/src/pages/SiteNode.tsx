// Site çalışma alanının Node.js bölümü: uygulamanın durumu, başlatma ayarları,
// ortam değişkenleri ve npm komutları.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Play, Plus, RotateCw, Square, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link } from "react-router";
import { useFeedback } from "../components/feedback";
import type { Tone } from "../components/ui";
import { Button, Checkbox, ErrorBlock, IconButton, Input, Modal, Mono, Panel, Select, Status } from "../components/ui";
import type { NodeSettings, NodeStatus, Site, SiteOptions } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { t } from "../lib/i18n";
import { JobLog } from "./Jobs";

const stateView: Record<string, { tone: Tone; label: string }> = {
  active: { tone: "ok", label: "Çalışıyor" },
  activating: { tone: "accent", label: "Başlatılıyor" },
  deactivating: { tone: "warn", label: "Durduruluyor" },
  failed: { tone: "bad", label: "Çöktü" },
  inactive: { tone: "idle", label: "Durdu" },
};

const rowInput = "h-9 min-w-0 flex-1 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13.5px]";

function StatusPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const queryKey = ["site-node", site.id];
  const status = useQuery({ queryKey, queryFn: () => api.get<NodeStatus>(`/sites/${site.id}/node`), refetchInterval: 4000 });
  const act = useMutation({
    mutationFn: (action: "start" | "stop" | "restart") => api.post<NodeStatus>(`/sites/${site.id}/node/${action}`),
    onSuccess: (next) => queryClient.setQueryData(queryKey, next),
    onError: (error) => feedback.error(error),
  });

  const data = status.data;
  const view = stateView[data?.state ?? ""] ?? { tone: "idle" as Tone, label: data?.state ?? "" };
  const running = data?.state === "active" || data?.state === "activating";
  return (
    <Panel>
      {status.isError && <ErrorBlock message={errorMessage(status.error)} />}
      {data && (
        <div className="flex flex-wrap items-center gap-x-6 gap-y-3">
          <div className="min-w-0 flex-1">
            <Status tone={view.tone}>{t(view.label)}</Status>
            <dl className="tabular mt-1 flex flex-wrap gap-x-5 gap-y-1 text-[13.5px] text-ink-2">
              <div className="flex gap-1.5">
                <dt>{t("Port")}</dt>
                <dd>
                  <Mono>{data.port}</Mono>
                </dd>
              </div>
              {running && data.pid > 0 && (
                <div className="flex gap-1.5">
                  <dt>{t("Süreç")}</dt>
                  <dd>
                    <Mono>{data.pid}</Mono>
                  </dd>
                </div>
              )}
              {running && data.memory_bytes !== null && (
                <div className="flex gap-1.5">
                  <dt>{t("Bellek")}</dt>
                  <dd>
                    <Mono>{formatBytes(data.memory_bytes)}</Mono>
                  </dd>
                </div>
              )}
              {data.restarts > 0 && (
                <div className="flex gap-1.5 text-warn">
                  <dt>{t("Kendiliğinden yeniden başlama")}</dt>
                  <dd>
                    <Mono>{data.restarts}</Mono>
                  </dd>
                </div>
              )}
            </dl>
            {data.state === "failed" && (
              <p className="mt-2 max-w-[70ch] text-[13.5px] text-bad">
                {t("Uygulama başlatılamıyor. Nedenini uygulama günlüğünde görebilirsiniz; çoğu zaman eksik bağımlılık ya da yanlış başlatma komutudur.")}
              </p>
            )}
          </div>
          <div className="flex flex-wrap gap-2">
            <Link
              to={`/logs?kaynak=${encodeURIComponent(`site:${site.id}:app`)}`}
              className="inline-flex h-9 items-center rounded-md border border-line-strong bg-surface px-3.5 font-medium hover:bg-sunken"
            >
              {t("Uygulama günlüğü")}
            </Link>
            {running ? (
              <>
                <Button icon={<Square className="size-3.5" />} loading={act.isPending && act.variables === "stop"} onClick={() => act.mutate("stop")}>
                  {t("Durdur")}
                </Button>
                <Button
                  variant="primary"
                  icon={<RotateCw className="size-4" />}
                  loading={act.isPending && act.variables === "restart"}
                  onClick={() => act.mutate("restart")}
                >
                  {t("Yeniden başlat")}
                </Button>
              </>
            ) : (
              <Button variant="primary" icon={<Play className="size-4" />} loading={act.isPending} onClick={() => act.mutate("start")}>
                {t("Başlat")}
              </Button>
            )}
          </div>
        </div>
      )}
    </Panel>
  );
}

function SettingsPanel({ site, options }: { site: Site; options: SiteOptions }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [settings, setSettings] = useState<NodeSettings>(site.node);
  const save = useMutation({
    mutationFn: () => api.patch<Site>(`/sites/${site.id}`, { node: { ...settings, env: settings.env.filter((variable) => variable.key.trim() !== "") } }),
    onSuccess: (updated) => {
      queryClient.setQueryData(["site", String(site.id)], updated);
      void queryClient.invalidateQueries({ queryKey: ["site-node", site.id] });
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      feedback.success(t("Ayarlar kaydedildi; uygulama yeniden başlatıldı"));
    },
  });

  const set = <K extends keyof NodeSettings>(key: K, value: NodeSettings[K]) => setSettings((current) => ({ ...current, [key]: value }));
  const setEnv = (index: number, field: "key" | "value", value: string) =>
    set(
      "env",
      settings.env.map((variable, i) => (i === index ? { ...variable, [field]: value } : variable)),
    );
  const versions = options.node_versions.includes(settings.version) || !settings.version ? options.node_versions : [settings.version, ...options.node_versions];

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Panel title={t("Uygulama ayarları")}>
      <form onSubmit={submit} className="flex max-w-2xl flex-col gap-4">
        <div className="grid gap-4 sm:grid-cols-2">
          {site.kind === "python" ? (
            <Input label={t("Python sürümü")} value={`Python ${settings.version}`} readOnly disabled hint={t("Sunucudaki Python; site kendi sanal ortamını (.venv) kullanır.")} />
          ) : (
            <Select label={t("Node.js sürümü")} value={settings.version} onChange={(event) => set("version", event.target.value)}>
              {versions.map((version) => (
                <option key={version} value={version}>
                  Node.js {version}
                </option>
              ))}
            </Select>
          )}
          <Input
            label={t("Bellek sınırı (MB)")}
            type="number"
            min={0}
            value={settings.memory_mb}
            onChange={(event) => set("memory_mb", Number(event.target.value))}
            hint={t("0 sınırsız demektir. Sınır aşılırsa uygulama yeniden başlatılır.")}
          />
        </div>
        <Input
          label={t("Başlatma komutu")}
          value={settings.command}
          onChange={(event) => set("command", event.target.value)}
          hint={
            site.kind === "python"
              ? t("Site klasöründe, sanal ortam etkinken çalışır. Örnek: gunicorn app:app --bind 127.0.0.1:$PORT ya da uvicorn app:app --host 127.0.0.1 --port $PORT")
              : t("Site klasöründe, sitenin kullanıcısıyla çalışır. Örnek: npm start ya da node dist/server.js")
          }
          className="font-mono text-[13.5px]"
          spellCheck={false}
          required
        />
        <fieldset className="flex flex-col gap-2">
          <legend className="mb-2 text-[13.5px] font-medium text-ink-2">{t("Ortam değişkenleri")}</legend>
          {settings.env.map((variable, index) => (
            <div key={index} className="flex items-center gap-2">
              <input
                aria-label={t("Değişken adı")}
                placeholder="DATABASE_URL"
                value={variable.key}
                onChange={(event) => setEnv(index, "key", event.target.value)}
                className={rowInput}
                spellCheck={false}
              />
              <input
                aria-label={t("Değer")}
                value={variable.value}
                onChange={(event) => setEnv(index, "value", event.target.value)}
                className={rowInput}
                spellCheck={false}
              />
              <IconButton label={t("Değişkeni kaldır")} onClick={() => set("env", settings.env.filter((_, i) => i !== index))}>
                <Trash2 className="size-4" />
              </IconButton>
            </div>
          ))}
          <div>
            <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => set("env", [...settings.env, { key: "", value: "" }])}>
              {t("Değişken ekle")}
            </Button>
          </div>
          <p className="max-w-[75ch] text-[13px] text-ink-3">
            {site.kind === "python"
              ? t("PORT ve HOST değişkenlerini panel belirler; uygulamanız PORT değişkenindeki portu dinlemelidir.")
              : t("PORT ve HOST değişkenlerini panel belirler; uygulamanız PORT değişkenindeki portu dinlemelidir. NODE_ENV tanımlamazsanız production olur.")}
          </p>
        </fieldset>
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div>
          <Button type="submit" variant="primary" loading={save.isPending}>
            {t("Kaydet ve yeniden başlat")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function NpmPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [custom, setCustom] = useState("");
  const [restart, setRestart] = useState(true);
  const [watching, setWatching] = useState<{ jobId: number; title: string } | null>(null);
  const run = useMutation({
    mutationFn: (script: string) => api.post<{ job_id: number }>(`/sites/${site.id}/node/run`, { script, restart }),
    onSuccess: (result, script) => {
      setWatching({ jobId: result.job_id, title: script === "install" || script === "ci" ? `npm ${script}` : `npm run ${script}` });
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });
  const presets: { script: string; label: string; hint: string }[] = [
    { script: "install", label: "npm install", hint: t("Bağımlılıkları kurar") },
    { script: "ci", label: "npm ci", hint: t("Kilit dosyasına göre temiz kurulum") },
    { script: "build", label: "npm run build", hint: t("Uygulamayı derler") },
  ];
  return (
    <Panel title={t("npm komutları")}>
      <p className="mb-3 max-w-[75ch] text-ink-2">{t("Komutlar site klasöründe, sitenin kullanıcısıyla ve seçili Node.js sürümüyle çalışır; çıktısını canlı izlersiniz.")}</p>
      <div className="flex flex-wrap gap-2">
        {presets.map((preset) => (
          <Button key={preset.script} title={preset.hint} loading={run.isPending && run.variables === preset.script} onClick={() => run.mutate(preset.script)} className="font-mono text-[13.5px]">
            {preset.label}
          </Button>
        ))}
      </div>
      <form
        className="mt-4 flex max-w-md items-end gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (custom.trim()) run.mutate(custom.trim());
        }}
      >
        <div className="min-w-0 flex-1">
          <Input
            label={t("Başka bir betik")}
            value={custom}
            onChange={(event) => setCustom(event.target.value)}
            placeholder="migrate"
            hint={t("package.json içindeki betiğin adı; “npm run” ile çalıştırılır.")}
            className="font-mono text-[13.5px]"
            spellCheck={false}
          />
        </div>
        <Button type="submit" className="mb-[1.6rem]" disabled={!custom.trim()} loading={run.isPending && run.variables === custom.trim()}>
          {t("Çalıştır")}
        </Button>
      </form>
      <div className="mt-1">
        <Checkbox label={t("Komut bitince uygulamayı yeniden başlat")} checked={restart} onChange={(event) => setRestart(event.target.checked)} />
      </div>
      <Modal
        open={watching !== null}
        onClose={() => {
          setWatching(null);
          void queryClient.invalidateQueries({ queryKey: ["site-node", site.id] });
        }}
        title={watching?.title ?? ""}
        wide
      >
        {watching && <JobLog jobId={watching.jobId} onDone={() => void queryClient.invalidateQueries({ queryKey: ["site-node", site.id] })} />}
      </Modal>
    </Panel>
  );
}

/** Python sitesinin bağımlılıklarını (requirements.txt) sanal ortama kurar. */
function PipPanel({ site }: { site: Site }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [restart, setRestart] = useState(true);
  const [jobId, setJobId] = useState<number | null>(null);
  const run = useMutation({
    mutationFn: () => api.post<{ job_id: number }>(`/sites/${site.id}/node/run`, { script: "install", restart }),
    onSuccess: (result) => {
      setJobId(result.job_id);
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
    onError: (error) => feedback.error(error),
  });
  const refresh = () => void queryClient.invalidateQueries({ queryKey: ["site-node", site.id] });
  return (
    <Panel title={t("Bağımlılıklar")}>
      <p className="mb-3 max-w-[75ch] text-ink-2">
        {t("Uygulamanızın gereksinimlerini site klasöründeki requirements.txt dosyasına yazın. Paketler yalnızca bu sitenin sanal ortamına kurulur; sunucudaki öteki uygulamaları etkilemez.")}
      </p>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <Button loading={run.isPending} onClick={() => run.mutate()} className="font-mono text-[13.5px]">
          pip install -r requirements.txt
        </Button>
        <Checkbox label={t("Komut bitince uygulamayı yeniden başlat")} checked={restart} onChange={(event) => setRestart(event.target.checked)} />
      </div>
      <Modal
        open={jobId !== null}
        onClose={() => {
          setJobId(null);
          refresh();
        }}
        title="pip install"
        wide
      >
        {jobId !== null && <JobLog jobId={jobId} onDone={refresh} />}
      </Modal>
    </Panel>
  );
}

export function NodeSection({ site, options }: { site: Site; options: SiteOptions }) {
  return (
    <div className="flex flex-col gap-5">
      <StatusPanel site={site} />
      <SettingsPanel key={JSON.stringify(site.node)} site={site} options={options} />
      {site.kind === "python" ? <PipPanel site={site} /> : <NpmPanel site={site} />}
    </div>
  );
}
