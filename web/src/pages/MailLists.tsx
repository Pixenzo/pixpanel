// Posta alanı sayfasının "Listeler" bölümü: `liste@alan` adresine gelen iletiyi üyelere dağıtan basit
// dağıtım listeleri. Liste oluşturma, gönderim kipi ve üyeleri toplu yapıştırarak ekleme/çıkarma.

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, Plus, Trash2 } from "lucide-react";
import type { FormEvent } from "react";
import { useState } from "react";
import { useFeedback } from "../components/feedback";
import { Badge, Button, Checkbox, EmptyState, ErrorBlock, IconButton, Input, LoadingBlock, Modal, Mono, Panel, Select, Table, Td, Textarea, Th } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import type { MailDomain } from "./Mail";

type ListMode = "anyone" | "members" | "admins";

interface MailList {
  id: number;
  domain_id: number;
  local: string;
  address: string;
  mode: ListMode;
  member_count: number;
  admin_count: number;
  created_at: number;
}

interface ListMember {
  id: number;
  address: string;
  is_admin: boolean;
}

interface ListDetail {
  list: MailList;
  members: ListMember[];
}

const modeLabels: Record<ListMode, string> = {
  anyone: "Herkes gönderebilir",
  members: "Yalnızca üyeler",
  admins: "Yalnızca yöneticiler",
};

const modeTones: Record<ListMode, "warn" | "accent" | "idle"> = { anyone: "warn", members: "accent", admins: "idle" };

const listsKey = (domainId: number) => ["mail-lists", domainId] as const;
const detailKey = (domainId: number, listId: number) => ["mail-list", domainId, listId] as const;

function CreateListModal({ domain, onClose, onCreated }: { domain: MailDomain; onClose: () => void; onCreated: (list: MailList) => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [local, setLocal] = useState("");
  const [mode, setMode] = useState<ListMode>("members");
  const create = useMutation({
    mutationFn: () => api.post<MailList>(`/mail/domains/${domain.id}/lists`, { local: local.trim().toLowerCase(), mode }),
    onSuccess: (list) => {
      void queryClient.invalidateQueries({ queryKey: listsKey(domain.id) });
      void queryClient.invalidateQueries({ queryKey: ["mail-domain", String(domain.id)] });
      feedback.success(t("{address} oluşturuldu", { address: list.address }));
      onCreated(list);
    },
  });
  function submit(event: FormEvent) {
    event.preventDefault();
    create.mutate();
  }
  return (
    <Modal
      open
      onClose={onClose}
      title={t("Liste oluştur")}
      footer={
        <>
          <Button onClick={onClose}>{t("Vazgeç")}</Button>
          <Button variant="primary" type="submit" form="list-form" loading={create.isPending}>
            {t("Listeyi oluştur")}
          </Button>
        </>
      }
    >
      <form id="list-form" onSubmit={submit} className="flex flex-col gap-5">
        <Input label={t("Liste adresi")} value={local} onChange={(event) => setLocal(event.target.value)} placeholder="duyuru" hint={`@${domain.domain}`} autoFocus required spellCheck={false} />
        <Select label={t("Kimler gönderebilir?")} value={mode} onChange={(event) => setMode(event.target.value as ListMode)}>
          {(Object.keys(modeLabels) as ListMode[]).map((item) => (
            <option key={item} value={item}>
              {t(modeLabels[item])}
            </option>
          ))}
        </Select>
        {create.isError && <ErrorBlock message={errorMessage(create.error)} />}
      </form>
    </Modal>
  );
}

function ListEditor({ domain, list, onClose }: { domain: MailDomain; list: MailList; onClose: () => void }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const detail = useQuery({ queryKey: detailKey(domain.id, list.id), queryFn: () => api.get<ListDetail>(`/mail/domains/${domain.id}/lists/${list.id}`) });
  const [text, setText] = useState("");
  const [asAdmin, setAsAdmin] = useState(false);

  function refresh() {
    void queryClient.invalidateQueries({ queryKey: detailKey(domain.id, list.id) });
    void queryClient.invalidateQueries({ queryKey: listsKey(domain.id) });
  }
  const base = `/mail/domains/${domain.id}/lists/${list.id}`;
  const setMode = useMutation({
    mutationFn: (mode: ListMode) => api.patch<MailList>(base, { mode }),
    onSuccess: () => {
      refresh();
      feedback.success(t("Gönderim kipi güncellendi"));
    },
    onError: (error) => feedback.error(error),
  });
  const add = useMutation({
    mutationFn: () => api.post<{ added: number; existing: number }>(`${base}/members`, { text, admin: asAdmin }),
    onSuccess: (result) => {
      setText("");
      refresh();
      feedback.success(t("{added} üye eklendi, {existing} zaten üyeydi", { added: result.added, existing: result.existing }));
    },
  });
  const remove = useMutation({
    mutationFn: (body: string) => api.post<{ removed: number }>(`${base}/members/remove`, { text: body }),
    onSuccess: (result) => {
      refresh();
      feedback.success(t("{n} üye çıkarıldı", { n: result.removed }));
    },
    onError: (error) => feedback.error(error),
  });
  const toggleAdmin = useMutation({
    mutationFn: (member: ListMember) => api.patch(`${base}/members`, { text: member.address, admin: !member.is_admin }),
    onSuccess: refresh,
    onError: (error) => feedback.error(error),
  });

  const current = detail.data?.list ?? list;
  return (
    <Modal open onClose={onClose} title={current.address} wide footer={<Button onClick={onClose}>{t("Kapat")}</Button>}>
      <div className="flex flex-col gap-5">
        <Select
          label={t("Kimler gönderebilir?")}
          value={current.mode}
          disabled={setMode.isPending}
          onChange={(event) => setMode.mutate(event.target.value as ListMode)}
          hint={t("Kısıt, gönderenin zarf adresine bakar. Üyesiz bir liste hiçbir iletiyi kabul etmez.")}
        >
          {(Object.keys(modeLabels) as ListMode[]).map((item) => (
            <option key={item} value={item}>
              {t(modeLabels[item])}
            </option>
          ))}
        </Select>

        <div className="flex flex-col gap-3">
          <Textarea
            label={t("Üyeler (toplu yapıştırın)")}
            value={text}
            onChange={(event) => setText(event.target.value)}
            placeholder={"ali@ornek.com\nveli@baska.com, ayse@baska.com"}
            hint={t("Satır, boşluk, virgül ya da noktalı virgülle ayrılmış adresler. Biri geçersizse hiçbiri işlenmez. En fazla 1000 üye.")}
            spellCheck={false}
          />
          <Checkbox label={t("Eklenenler liste yöneticisi olsun")} checked={asAdmin} onChange={(event) => setAsAdmin(event.target.checked)} />
          {(add.isError || remove.isError) && <ErrorBlock message={errorMessage(add.error ?? remove.error)} />}
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" icon={<Plus className="size-3.5" />} loading={add.isPending} disabled={!text.trim()} onClick={() => add.mutate()}>
              {t("Üyeleri ekle")}
            </Button>
            <Button
              loading={remove.isPending && remove.variables === text}
              disabled={!text.trim()}
              onClick={() => remove.mutate(text, { onSuccess: () => setText("") })}
            >
              {t("Yapıştırılanları çıkar")}
            </Button>
          </div>
        </div>

        {detail.isPending ? (
          <LoadingBlock />
        ) : detail.isError ? (
          <ErrorBlock message={errorMessage(detail.error)} onRetry={() => void detail.refetch()} />
        ) : detail.data.members.length === 0 ? (
          <p className="text-ink-2">{t("Listede henüz üye yok.")}</p>
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Üye")}</Th>
                <Th>{t("Rol")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {detail.data.members.map((member) => (
                <tr key={member.id} className="hover:bg-sunken/50">
                  <Td className="break-all">
                    <Mono>{member.address}</Mono>
                  </Td>
                  <Td>
                    <button type="button" className="text-left" title={t("Yönetici işaretini değiştir")} disabled={toggleAdmin.isPending} onClick={() => toggleAdmin.mutate(member)}>
                      {member.is_admin ? <Badge tone="accent">{t("Yönetici")}</Badge> : <Badge>{t("Üye")}</Badge>}
                    </button>
                  </Td>
                  <Td align="right">
                    <IconButton label={t("Çıkar")} disabled={remove.isPending} onClick={() => remove.mutate(member.address)}>
                      <Trash2 className="size-4" />
                    </IconButton>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        )}
      </div>
    </Modal>
  );
}

export function MailLists({ domain }: { domain: MailDomain }) {
  const feedback = useFeedback();
  const queryClient = useQueryClient();
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<MailList | null>(null);
  const lists = useQuery({ queryKey: listsKey(domain.id), queryFn: () => api.get<MailList[]>(`/mail/domains/${domain.id}/lists`) });
  const remove = useMutation({
    mutationFn: (list: MailList) => api.delete(`/mail/domains/${domain.id}/lists/${list.id}`),
    onSuccess: (_, list) => {
      void queryClient.invalidateQueries({ queryKey: listsKey(domain.id) });
      feedback.success(t("{address} silindi", { address: list.address }));
    },
    onError: (error) => feedback.error(error),
  });

  async function confirmRemove(list: MailList) {
    const confirmed = await feedback.confirm({
      title: t("{address} silinsin mi?", { address: list.address }),
      description: t("Bu adrese gelen iletiler artık dağıtılmaz. Üyelerin kutularına dokunulmaz."),
      confirmLabel: t("Listeyi sil"),
      danger: true,
    });
    if (confirmed) remove.mutate(list);
  }

  return (
    <>
      <Panel
        flush
        title={t("Posta listeleri")}
        actions={
          <Button size="sm" variant="primary" icon={<Plus className="size-3.5" />} onClick={() => setCreating(true)}>
            {t("Liste oluştur")}
          </Button>
        }
      >
        {lists.isPending ? (
          <LoadingBlock />
        ) : lists.isError ? (
          <ErrorBlock message={errorMessage(lists.error)} onRetry={() => void lists.refetch()} />
        ) : lists.data.length === 0 ? (
          <EmptyState
            title={t("Henüz liste yok")}
            description={t("Liste, bir adrese gelen iletiyi üyelerin hepsine dağıtır; örneğin duyuru@ adresine yazılan herkese ulaşır. Kimlerin gönderebileceğini seçebilirsiniz.")}
            action={
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => setCreating(true)}>
                {t("Liste oluştur")}
              </Button>
            }
          />
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>{t("Liste")}</Th>
                <Th>{t("Kimler gönderebilir?")}</Th>
                <Th align="right">{t("Üye")}</Th>
                <Th align="right">
                  <span className="sr-only">{t("İşlemler")}</span>
                </Th>
              </tr>
            </thead>
            <tbody>
              {lists.data.map((list) => (
                <tr key={list.id} className="hover:bg-sunken/50">
                  <Td>
                    <Mono className="text-[14px] font-medium">{list.address}</Mono>
                  </Td>
                  <Td>
                    <Badge tone={modeTones[list.mode]}>{t(modeLabels[list.mode])}</Badge>
                  </Td>
                  <Td align="right" className="tabular">
                    {list.member_count}
                  </Td>
                  <Td align="right" className="whitespace-nowrap">
                    <div className="flex justify-end gap-1">
                      <IconButton label={t("Üyeler ve kip")} onClick={() => setEditing(list)}>
                        <Pencil className="size-4" />
                      </IconButton>
                      <IconButton label={t("Sil")} disabled={remove.isPending} onClick={() => void confirmRemove(list)}>
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
      {creating && (
        <CreateListModal
          domain={domain}
          onClose={() => setCreating(false)}
          onCreated={(list) => {
            setCreating(false);
            setEditing(list);
          }}
        />
      )}
      {editing && <ListEditor domain={domain} list={editing} onClose={() => setEditing(null)} />}
    </>
  );
}
