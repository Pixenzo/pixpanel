// Site çalışma alanının "Vekil kuralları" bölümü: sitenin belirli yollarını
// başka bir adrese (çoğunlukla bir Docker konteynerine) iletir.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Button, ErrorBlock, IconButton, Panel } from "../components/ui";
import type { ProxyRule, Site } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

interface ContainerPort {
  host_port: string;
  protocol: string;
}

interface ContainerSummary {
  name: string;
  state: string;
  ports: ContainerPort[];
}

const cell = "h-9 min-w-0 rounded-md border border-line-strong bg-surface px-3 font-mono text-[13px]";

export function ProxyRulesSection({ site }: { site: Site }) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [rules, setRules] = useState<ProxyRule[]>(site.proxy_rules);

  // Yönetici, hedefi çalışan konteynerlerin yayınladığı portlardan seçebilir.
  const containers = useQuery({
    queryKey: ["docker-containers"],
    queryFn: () => api.get<ContainerSummary[]>("/docker/containers"),
    enabled: me.role === "admin",
    retry: false,
  });
  const targets = (containers.data ?? [])
    .filter((container) => container.state === "running")
    .flatMap((container) =>
      container.ports.filter((port) => port.protocol === "tcp" && port.host_port).map((port) => ({ label: `${container.name} (${port.host_port})`, value: `http://127.0.0.1:${port.host_port}` })),
    );

  const save = useMutation({
    mutationFn: () => api.patch<Site>(`/sites/${site.id}`, { proxy_rules: rules }),
    onSuccess: (updated) => {
      queryClient.setQueryData(["site", String(site.id)], updated);
      feedback.success(t("Vekil kuralları kaydedildi"));
    },
  });

  const update = (index: number, changes: Partial<ProxyRule>) => setRules((current) => current.map((rule, i) => (i === index ? { ...rule, ...changes } : rule)));
  const dirty = JSON.stringify(rules) !== JSON.stringify(site.proxy_rules);

  function submit(event: FormEvent) {
    event.preventDefault();
    save.mutate();
  }

  return (
    <Panel title={t("Vekil kuralları")}>
      <p className="mb-4 max-w-[75ch] text-ink-2">
        {t(
          "Sitenin belirli bir yoluna gelen istekleri başka bir adrese iletir; örneğin /api/ yolunu bir Docker konteynerine. Sitenin geri kalanı olağan içeriğini sunmaya devam eder. Bütün siteyi iletmek için site türünü “Ters vekil” yapın.",
        )}
      </p>
      <form onSubmit={submit} className="flex flex-col gap-3">
        {rules.length === 0 && <p className="rounded-md bg-sunken/70 px-3 py-2.5 text-[13px] text-ink-2">{t("Henüz kural yok. “Kural ekle” ile ilk kuralınızı tanımlayın.")}</p>}
        {rules.map((rule, index) => (
          <div key={index} className="flex flex-col gap-2 rounded-lg border border-line p-3">
            <div className="flex flex-wrap items-center gap-2">
              <input
                aria-label={t("Yol")}
                placeholder="/api/"
                value={rule.path}
                onChange={(event) => update(index, { path: event.target.value })}
                className={`${cell} w-40`}
                spellCheck={false}
                required
              />
              <span aria-hidden className="text-ink-3">
                →
              </span>
              <input
                aria-label={t("Hedef adres")}
                placeholder="http://127.0.0.1:8080"
                value={rule.target}
                onChange={(event) => update(index, { target: event.target.value })}
                className={`${cell} flex-1`}
                spellCheck={false}
                required
              />
              {targets.length > 0 && (
                <select
                  aria-label={t("Konteynerden seç")}
                  value=""
                  onChange={(event) => event.target.value && update(index, { target: event.target.value })}
                  className="h-9 rounded-md border border-line-strong bg-surface px-2 text-[13px]"
                >
                  <option value="">{t("Konteynerden seç")}</option>
                  {targets.map((target) => (
                    <option key={target.label} value={target.value}>
                      {target.label}
                    </option>
                  ))}
                </select>
              )}
              <IconButton label={t("Kuralı kaldır")} onClick={() => setRules((current) => current.filter((_, i) => i !== index))}>
                <Trash2 className="size-4" />
              </IconButton>
            </div>
            <div className="flex flex-wrap gap-x-6 gap-y-1 text-[13px]">
              <label className="flex items-center gap-2">
                <input type="checkbox" className="size-4 accent-(--accent)" checked={rule.strip_prefix} onChange={(event) => update(index, { strip_prefix: event.target.checked })} />
                <span>
                  {t("Yolu hedefe iletirken öneki at")}{" "}
                  <span className="text-ink-3">
                    ({rule.strip_prefix ? t("{path}kullanici → /kullanici", { path: rule.path || "/api/" }) : t("{path}kullanici → {path}kullanici", { path: rule.path || "/api/" })})
                  </span>
                </span>
              </label>
              <label className="flex items-center gap-2">
                <input type="checkbox" className="size-4 accent-(--accent)" checked={rule.websocket} onChange={(event) => update(index, { websocket: event.target.checked })} />
                {t("WebSocket bağlantılarını ilet")}
              </label>
            </div>
          </div>
        ))}
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
        <div className="flex flex-wrap gap-2">
          <Button icon={<Plus className="size-4" />} onClick={() => setRules((current) => [...current, { path: "", target: "", websocket: true, strip_prefix: false }])}>
            {t("Kural ekle")}
          </Button>
          <Button type="submit" variant="primary" loading={save.isPending} disabled={!dirty}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </div>
      </form>
    </Panel>
  );
}
