import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { Link, useNavigate } from "react-router";
import { useFeedback } from "../components/feedback";
import {
  Badge,
  Button,
  EmptyState,
  ErrorBlock,
  Input,
  LoadingBlock,
  Modal,
  Mono,
  PageHeader,
  Panel,
  Select,
  Status,
  Table,
  Td,
  Th,
} from "../components/ui";
import type { Site, SiteKind, SiteOptions } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { classNames } from "../lib/format";
import { t } from "../lib/i18n";
import { useMe } from "../lib/session";

export const kindLabels: Record<SiteKind, string> = {
  php: "PHP",
  static: "Statik",
  proxy: "Ters vekil",
};

const kindChoices: { id: SiteKind; title: string; description: string }[] = [
  { id: "php", title: "PHP", description: "WordPress, Laravel ve diğer PHP uygulamaları" },
  { id: "static", title: "Statik", description: "Yalnızca HTML, CSS ve JavaScript dosyaları" },
  { id: "proxy", title: "Ters vekil", description: "Başka bir adreste ya da portta çalışan uygulamaya yönlendirir" },
];

export function useSiteOptions() {
  return useQuery({ queryKey: ["site-options"], queryFn: () => api.get<SiteOptions>("/sites/options") });
}

function CreateSiteModal({ open, onClose, options }: { open: boolean; onClose: () => void; options: SiteOptions }) {
  const me = useMe();
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const [domain, setDomain] = useState("");
  const [kind, setKind] = useState<SiteKind>(options.php_versions.length > 0 ? "php" : "static");
  const [phpVersion, setPhpVersion] = useState(options.php_versions[0] ?? "");
  const [proxyTarget, setProxyTarget] = useState("http://127.0.0.1:3000");
  const [withWww, setWithWww] = useState(true);
  const [ownerId, setOwnerId] = useState(me.id);

  const isSubdomain = domain.trim().split(".").length > 2;
  const create = useMutation({
    mutationFn: () =>
      api.post<Site>("/sites", {
        domain,
        kind,
        php_version: kind === "php" ? phpVersion : undefined,
        proxy_target: kind === "proxy" ? proxyTarget : undefined,
        aliases: withWww && !isSubdomain && domain.trim() ? [`www.${domain.trim()}`] : [],
        user_id: options.owners.length > 0 ? ownerId : undefined,
      }),
    onSuccess: (site) => {
      feedback.success(t("{domain} oluşturuldu", { domain: site.domain }));
      void queryClient.invalidateQueries({ queryKey: ["sites"] });
      onClose();
      void navigate(`/sites/${site.id}`);
    },
  });

  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }

  const phpMissing = kind === "php" && options.php_versions.length === 0;

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t("Site oluştur")}
      wide
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="create-site" loading={create.isPending} disabled={phpMissing}>
            {t("Siteyi oluştur")}
          </Button>
        </>
      }
    >
      <form id="create-site" onSubmit={submit} className="flex flex-col gap-5">
        <Input
          label={t("Alan adı")}
          value={domain}
          onChange={(event) => setDomain(event.target.value)}
          placeholder="ornek.com"
          hint={t("Alt alan adı da yazabilirsiniz: blog.ornek.com")}
          autoCapitalize="none"
          spellCheck={false}
          autoFocus
          required
        />
        {!isSubdomain && domain.trim() && (
          <label className="-mt-2 flex items-center gap-2.5">
            <input type="checkbox" className="size-4 accent-(--accent)" checked={withWww} onChange={(event) => setWithWww(event.target.checked)} />
            <span>
              <Mono>www.{domain.trim()}</Mono> {t("adresi de bu siteyi açsın")}
            </span>
          </label>
        )}

        <fieldset>
          <legend className="mb-2 text-[13px] font-medium text-ink-2">{t("Site türü")}</legend>
          <div className="grid gap-2 sm:grid-cols-3">
            {kindChoices.map((choice) => (
              <label
                key={choice.id}
                className={classNames(
                  "flex cursor-pointer flex-col gap-1 rounded-lg border p-3 transition-colors",
                  kind === choice.id ? "border-accent bg-accent-soft" : "border-line-strong hover:bg-sunken",
                )}
              >
                <span className="flex items-center gap-2 font-medium">
                  <input type="radio" name="kind" className="accent-(--accent)" checked={kind === choice.id} onChange={() => setKind(choice.id)} />
                  {t(choice.title)}
                </span>
                <span className="text-[12.5px] text-ink-2">{t(choice.description)}</span>
              </label>
            ))}
          </div>
        </fieldset>

        {kind === "php" &&
          (phpMissing ? (
            <p className="rounded-md bg-warn-soft px-3 py-2 text-warn">
              {t("Henüz kurulu bir PHP sürümü yok.")}{" "}
              <Link to="/components" className="font-medium underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            </p>
          ) : (
            <Select label={t("PHP sürümü")} value={phpVersion} onChange={(event) => setPhpVersion(event.target.value)}>
              {options.php_versions.map((version) => (
                <option key={version} value={version}>
                  PHP {version}
                </option>
              ))}
            </Select>
          ))}

        {kind === "proxy" && (
          <Input
            label={t("Hedef adres")}
            value={proxyTarget}
            onChange={(event) => setProxyTarget(event.target.value)}
            hint={t("İsteklerin iletileceği uygulamanın adresi")}
            className="font-mono text-[13px]"
            spellCheck={false}
            required
          />
        )}

        {options.owners.length > 1 && (
          <Select label={t("Sahibi")} value={ownerId} onChange={(event) => setOwnerId(Number(event.target.value))} hint={t("Site bu hesabın Linux kullanıcısıyla çalışır.")}>
            {options.owners.map((owner) => (
              <option key={owner.id} value={owner.id}>
                {owner.username}
              </option>
            ))}
          </Select>
        )}

        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

export function SitesPage() {
  const me = useMe();
  const [creating, setCreating] = useState(false);
  const sites = useQuery({ queryKey: ["sites"], queryFn: () => api.get<Site[]>("/sites") });
  const options = useSiteOptions();
  const showOwner = me.role === "admin" || me.role === "reseller";

  const createButton = (
    <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)} disabled={!options.data?.nginx_installed}>
      {t("Site oluştur")}
    </Button>
  );

  return (
    <>
      <PageHeader title={t("Siteler")} description={t("Bu sunucuda yayınlanan alan adları ve alt alan adları.")} actions={createButton} />
      {sites.isError && <ErrorBlock message={errorMessage(sites.error)} onRetry={() => void sites.refetch()} />}
      {(sites.isPending || options.isPending) && <LoadingBlock />}
      {options.data && !options.data.nginx_installed && (
        <div className="mb-4 rounded-lg border border-warn/40 bg-warn-soft px-4 py-3 text-warn">
          {t("Site yayınlamak için nginx gerekli.")}{" "}
          {me.role === "admin" ? (
            <Link to="/components" className="font-medium underline">
              {t("Bileşenler sayfasından kurun")}
            </Link>
          ) : (
            t("Sunucu yöneticinizden kurmasını isteyin.")
          )}
        </div>
      )}
      {sites.data && options.data && (
        <Panel flush>
          {sites.data.length === 0 ? (
            <EmptyState
              title={t("Henüz site yok")}
              description={t("İlk sitenizi oluşturun; alan adını, türünü ve PHP sürümünü seçmeniz yeterli.")}
              action={options.data.nginx_installed ? createButton : undefined}
            />
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>{t("Alan adı")}</Th>
                  <Th>{t("Tür")}</Th>
                  <Th>{t("SSL")}</Th>
                  {showOwner && <Th>{t("Sahibi")}</Th>}
                  <Th>{t("Durum")}</Th>
                </tr>
              </thead>
              <tbody>
                {sites.data.map((site) => (
                  <tr key={site.id} className="hover:bg-sunken/50">
                    <Td>
                      <Link to={`/sites/${site.id}`} className="font-mono text-[13px] font-medium text-accent hover:underline">
                        {site.domain}
                      </Link>
                      {site.aliases.length > 0 && <p className="font-mono text-[12px] text-ink-3">{site.aliases.join(", ")}</p>}
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Badge>{site.kind === "php" ? `PHP ${site.php_version}` : t(kindLabels[site.kind])}</Badge>
                    </Td>
                    <Td className="whitespace-nowrap">
                      {site.ssl_mode === "none" ? <span className="text-ink-3">{t("Yok")}</span> : <Status tone="ok">{t("Etkin")}</Status>}
                    </Td>
                    {showOwner && (
                      <Td>
                        <Mono>{site.owner}</Mono>
                      </Td>
                    )}
                    <Td className="whitespace-nowrap">
                      {site.status === "active" ? <Status tone="ok">{t("Yayında")}</Status> : <Status tone="warn">{t("Askıda")}</Status>}
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
        </Panel>
      )}
      {options.data && creating && <CreateSiteModal open onClose={() => setCreating(false)} options={options.data} />}
    </>
  );
}
