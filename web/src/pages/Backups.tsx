// Yedekler sayfası: ne yedeklendi, ne zaman yedeklenecek ve yedekler nerede duruyor.
// Üç sekme vardır (adres çubuğunda `sekme` olarak tutulur); asıl iş ilk sekmededir: yedekleri görmek ve geri yüklemek.

import { Plus } from "lucide-react";
import { useState } from "react";
import { Link, useSearchParams } from "react-router";
import { Button, EmptyState, ErrorBlock, LoadingBlock, PageHeader, Panel, Tabs } from "../components/ui";
import { errorMessage } from "../lib/api";
import { t } from "../lib/i18n";
import { PlansTab } from "./BackupPlans";
import { SnapshotsTab } from "./BackupSnapshots";
import { MonoSentence, Notice, useBackupOverview } from "./BackupShared";
import { TargetModal, TargetsTab } from "./BackupTargets";

type BackupTab = "snapshots" | "plans" | "targets";

const TAB_PARAMS: Record<BackupTab, string> = { snapshots: "yedekler", plans: "planlar", targets: "yerler" };

function FirstRun({ onAddTarget }: { onAddTarget: () => void }) {
  const steps = [t("Yedeklerin duracağı yeri ekleyin."), t("Bir plan oluşturun."), t("Gerisini panel halleder: yedekleri alır, eskilerini temizler.")];
  return (
    <Panel title={t("Yedeklemeye başlayın")}>
      <div className="flex max-w-xl flex-col gap-4">
        <ol className="flex flex-col gap-2">
          {steps.map((step, index) => (
            <li key={step} className="flex items-baseline gap-3">
              <span className="w-4 shrink-0 font-semibold text-accent tabular-nums">{index + 1}</span>
              <span>{step}</span>
            </li>
          ))}
        </ol>
        <div>
          <Button variant="primary" icon={<Plus className="size-4" />} onClick={onAddTarget}>
            {t("Yedek yeri ekle")}
          </Button>
        </div>
      </div>
    </Panel>
  );
}

export function BackupsPage() {
  const overview = useBackupOverview();
  const [searchParams, setSearchParams] = useSearchParams();
  const [addingTarget, setAddingTarget] = useState(false);

  const requested = (Object.keys(TAB_PARAMS) as BackupTab[]).find((item) => TAB_PARAMS[item] === searchParams.get("sekme"));
  const tab: BackupTab = requested ?? "snapshots";

  function changeTab(next: BackupTab) {
    setSearchParams(
      (current) => {
        const params = new URLSearchParams(current);
        if (next === "snapshots") params.delete("sekme");
        else params.set("sekme", TAB_PARAMS[next]);
        return params;
      },
      { replace: true },
    );
  }

  const header = (
    <PageHeader
      title={t("Yedekler")}
      description={t("Sitelerinizin, veritabanlarınızın ve postalarınızın şifreli kopyalarını düzenli olarak başka bir yere alır; gerektiğinde tek tıkla geri yükler.")}
    />
  );

  if (overview.isPending) {
    return (
      <>
        {header}
        <LoadingBlock />
      </>
    );
  }
  if (overview.isError) {
    return (
      <>
        {header}
        <ErrorBlock message={errorMessage(overview.error)} onRetry={() => void overview.refetch()} />
      </>
    );
  }

  const data = overview.data;

  if (!data.installed) {
    return (
      <>
        {header}
        <Panel flush>
          <EmptyState
            title={t("Yedekleme bileşeni kurulu değil")}
            description={t("Yedek alabilmek için önce ilgili bileşeni kurmanız gerekir.")}
            action={
              <Link to="/components" className="font-medium text-accent hover:underline">
                {t("Bileşenler sayfasından kurun")}
              </Link>
            }
          />
        </Panel>
      </>
    );
  }

  if (!data.supported) {
    return (
      <>
        {header}
        <Notice tone="warn">
          <MonoSentence template={t("Kurulu yedekleme bileşeninin sürümü ({version}) çok eski; yedek alınamıyor.")} values={{ version: data.version || "?" }} />{" "}
          <Link to="/components" className="font-medium underline">
            {t("Bileşenler sayfasından güncelleyin")}
          </Link>
        </Notice>
      </>
    );
  }

  const tabs: { id: BackupTab; label: string }[] = [
    { id: "snapshots", label: t("Yedekler") },
    { id: "plans", label: t("Planlar") },
    { id: "targets", label: t("Yedek yerleri") },
  ];

  return (
    <>
      {header}
      {data.targets.length === 0 ? (
        <FirstRun onAddTarget={() => setAddingTarget(true)} />
      ) : (
        <>
          <Tabs label={t("Yedek sekmeleri")} tabs={tabs} value={tab} onChange={changeTab} />
          {tab === "snapshots" && <SnapshotsTab overview={data} />}
          {tab === "plans" && <PlansTab overview={data} />}
          {tab === "targets" && <TargetsTab overview={data} onAdd={() => setAddingTarget(true)} />}
        </>
      )}
      {/* Pencere sayfa düzeyinde durur; ilk yer eklenince ilk ekran değişse de açık kalır. */}
      {addingTarget && <TargetModal editing={null} targets={data.targets} onClose={() => setAddingTarget(false)} />}
    </>
  );
}
