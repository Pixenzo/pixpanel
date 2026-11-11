// Sunucunun IP adresleri: var olanları gösterir, yenisini ekler, bir adresi hesaba ayırır.
// Siteler buradaki adreslerden birine bağlanabilir (site sayfası → Adresler ve yayın).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import type { ServerAddress, ServerAddresses, UserInfo } from "../lib/api";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { useFeedback } from "./feedback";
import { Badge, Button, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Select, Table, Td, Th } from "./ui";

function AddModal({ interfaces, onClose }: { interfaces: string[]; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [address, setAddress] = useState("");
  const [prefix, setPrefix] = useState("32");
  const [networkInterface, setNetworkInterface] = useState(interfaces[0] ?? "");
  const [label, setLabel] = useState("");

  const save = useMutation({
    mutationFn: () => api.post("/system/ips", { address: address.trim(), prefix: Number(prefix), interface: networkInterface, label }),
    onSuccess: () => {
      feedback.success(t("{address} sunucuya eklendi", { address: address.trim() }));
      void queryClient.invalidateQueries({ queryKey: ["server-ips"] });
      void queryClient.invalidateQueries({ queryKey: ["site-options"] });
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
      title={t("IP adresi ekle")}
      description={t("Adres sunucunun ağ arayüzüne eklenir ve sunucu yeniden başladığında kendiliğinden yeniden kurulur.")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="ip-add-form" loading={save.isPending}>
            {t("Adresi ekle")}
          </Button>
        </>
      }
    >
      <form id="ip-add-form" onSubmit={submit} className="flex flex-col gap-5">
        <div className="grid gap-4 sm:grid-cols-[1fr_7rem]">
          <Input
            label={t("IP adresi")}
            value={address}
            onChange={(event) => setAddress(event.target.value)}
            placeholder="203.0.113.10"
            className="font-mono text-[13.5px]"
            spellCheck={false}
            autoComplete="off"
            autoFocus
            required
          />
          <Input label={t("Ağ öneki")} type="number" min={1} max={128} inputMode="numeric" value={prefix} onChange={(event) => setPrefix(event.target.value)} required />
        </div>
        {interfaces.length > 1 && (
          <Select label={t("Ağ arayüzü")} value={networkInterface} onChange={(event) => setNetworkInterface(event.target.value)}>
            {interfaces.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        )}
        <Input label={t("Etiket")} value={label} onChange={(event) => setLabel(event.target.value)} placeholder={t("İsteğe bağlı; ör. müşteri adı")} maxLength={60} />
        <p className="rounded-md bg-sunken px-3 py-2 text-[13.5px] text-ink-2">
          {t("Yalnızca sağlayıcınızın bu sunucuya tanımladığı adresleri ekleyin. Size ait olmayan bir adres ağdaki başka bir sunucunun bağlantısını bozabilir.")}
        </p>
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

function EditModal({ item, onClose }: { item: ServerAddress; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [label, setLabel] = useState(item.label);
  const [publicAddress, setPublicAddress] = useState(item.public_address);
  const [userId, setUserId] = useState(item.user_id ?? 0);
  const users = useQuery({ queryKey: ["users"], queryFn: () => api.get<UserInfo[]>("/users") });

  const save = useMutation({
    mutationFn: () =>
      api.put(`/system/ips/${encodeURIComponent(item.address)}`, { label, public_address: publicAddress.trim(), user_id: userId === 0 ? null : userId }),
    onSuccess: () => {
      feedback.success(t("{address} güncellendi", { address: item.address }));
      void queryClient.invalidateQueries({ queryKey: ["server-ips"] });
      void queryClient.invalidateQueries({ queryKey: ["site-options"] });
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
      title={t("{address} adresini düzenle", { address: item.address })}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="ip-edit-form" loading={save.isPending}>
            {t("Değişiklikleri kaydet")}
          </Button>
        </>
      }
    >
      <form id="ip-edit-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input label={t("Etiket")} value={label} onChange={(event) => setLabel(event.target.value)} maxLength={60} autoFocus />
        <Select
          label={t("Ayrıldığı hesap")}
          value={userId}
          onChange={(event) => setUserId(Number(event.target.value))}
          hint={t("Hesaba ayrılan adresi o hesap kendi sitelerinde seçebilir; yeni siteleri bu adresle açılır.")}
        >
          <option value={0}>{t("Hiçbiri (yalnızca yönetici kullanır)")}</option>
          {users.data
            ?.filter((user) => user.role !== "member")
            .map((user) => (
              <option key={user.id} value={user.id}>
                {user.username}
              </option>
            ))}
        </Select>
        <Input
          label={t("Dış adres")}
          value={publicAddress}
          onChange={(event) => setPublicAddress(event.target.value)}
          placeholder={t("Boş: adresin kendisi")}
          hint={t("Sunucu bir yönlendiricinin arkasındaysa bu adresin internetten görünen karşılığı. DNS kayıtlarına bu yazılır.")}
          className="font-mono text-[13.5px]"
          spellCheck={false}
          autoComplete="off"
        />
        {save.isError && <ErrorBlock message={errorMessage(save.error)} />}
      </form>
    </Modal>
  );
}

export function IpAddresses() {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<ServerAddress | null>(null);
  const ips = useQuery({ queryKey: ["server-ips"], queryFn: () => api.get<ServerAddresses>("/system/ips") });

  const remove = useMutation({
    mutationFn: (item: ServerAddress) => api.delete(`/system/ips/${encodeURIComponent(item.address)}`),
    onSuccess: (_, item) => feedback.success(t("{address} sunucudan kaldırıldı", { address: item.address })),
    onError: (error) => feedback.error(error),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["server-ips"] });
      void queryClient.invalidateQueries({ queryKey: ["site-options"] });
    },
  });

  async function confirmRemove(item: ServerAddress) {
    const confirmed = await feedback.confirm({
      title: t("{address} sunucudan kaldırılsın mı?", { address: item.address }),
      description: t("Adres ağ arayüzünden silinir; bu adrese gelen bağlantılar artık sunucuya ulaşmaz."),
      confirmLabel: t("Adresi kaldır"),
      danger: true,
    });
    if (confirmed) remove.mutate(item);
  }

  function removeHint(item: ServerAddress): string {
    if (!item.managed) return t("Bu adresi panel eklemedi; sunucunun ağ ayarından kaldırılır");
    if (item.site_count > 0) return t("Site bağlı olan adres kaldırılamaz");
    return t("{address} adresini kaldır", { address: item.address });
  }

  return (
    <>
      <Panel
        flush
        title={t("IP adresleri")}
        actions={
          <Button size="sm" icon={<Plus className="size-3.5" />} onClick={() => setAdding(true)} disabled={!ips.data}>
            {t("IP adresi ekle")}
          </Button>
        }
      >
        {ips.isPending && <LoadingBlock />}
        {ips.isError && (
          <div className="p-4">
            <ErrorBlock message={errorMessage(ips.error)} onRetry={() => void ips.refetch()} />
          </div>
        )}
        {ips.data && (
          <>
            <p className="max-w-[75ch] px-4 sm:px-6 pt-3 pb-3 text-ink-2">
              {t("Siteler varsayılan olarak sunucunun bütün adreslerinden yayınlanır. Bir siteyi yalnızca belirli bir adresten yayınlamak için site sayfasındaki Adresler ve yayın bölümünden adres seçin.")}
            </p>
            <Table>
              <thead>
                <tr>
                  <Th>{t("Adres")}</Th>
                  <Th>{t("Etiket")}</Th>
                  <Th>{t("Ayrıldığı hesap")}</Th>
                  <Th align="right">{t("Site")}</Th>
                  <Th align="right">{t("İşlemler")}</Th>
                </tr>
              </thead>
              <tbody>
                {ips.data.addresses.map((item) => (
                  <tr key={item.address} className="hover:bg-sunken/50">
                    <Td>
                      <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                        <Mono>
                          {item.address}
                          <span className="text-ink-3">/{item.prefix}</span>
                        </Mono>
                        {item.address === ips.data.dns_address && <Badge tone="accent">{t("Ana adres")}</Badge>}
                        {!item.present && <Badge tone="bad">{t("Sunucuda yok")}</Badge>}
                      </div>
                      {item.public_address !== "" && (
                        <div className="mt-0.5 text-[13px] text-ink-3">
                          {t("Dış adres")}: <span className="font-mono">{item.public_address}</span>
                        </div>
                      )}
                    </Td>
                    <Td className={item.label === "" ? "text-ink-3" : undefined}>{item.label === "" ? "—" : item.label}</Td>
                    <Td className={item.owner === null ? "text-ink-3" : undefined}>{item.owner ?? t("Ortak")}</Td>
                    <Td align="right">{item.site_count}</Td>
                    <Td align="right" className="py-0.5">
                      <div className="flex justify-end gap-0.5">
                        <IconButton label={t("{address} adresini düzenle", { address: item.address })} onClick={() => setEditing(item)}>
                          <Pencil className="size-4" />
                        </IconButton>
                        <IconButton label={removeHint(item)} disabled={!item.managed || item.site_count > 0} onClick={() => void confirmRemove(item)}>
                          <Trash2 className="size-4" />
                        </IconButton>
                      </div>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          </>
        )}
      </Panel>
      {adding && ips.data && <AddModal interfaces={ips.data.interfaces} onClose={() => setAdding(false)} />}
      {editing && <EditModal item={editing} onClose={() => setEditing(null)} />}
    </>
  );
}
