import { useQuery } from "@tanstack/react-query";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal as XTerm } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useSearchParams } from "react-router";
import type { Tone } from "../components/ui";
import { Button, ErrorBlock, LoadingBlock, PageHeader, Status } from "../components/ui";
import { api, errorMessage } from "../lib/api";
import { t } from "../lib/i18n";

interface Root {
  id: string;
  label: string;
}

type ConnectionState = "connecting" | "open" | "closed";

const stateView: Record<ConnectionState, { tone: Tone; label: string }> = {
  connecting: { tone: "warn", label: "Bağlanıyor" },
  open: { tone: "ok", label: "Bağlı" },
  closed: { tone: "idle", label: "Bağlantı kapandı" },
};

/** Panelin renk belirteçlerinden uçbirim renklerini türetir. */
function terminalTheme() {
  const style = getComputedStyle(document.documentElement);
  const color = (name: string) => style.getPropertyValue(name).trim();
  return {
    background: color("--sunken"),
    foreground: color("--ink"),
    cursor: color("--accent"),
    cursorAccent: color("--sunken"),
    selectionBackground: color("--accent-soft"),
  };
}

function Session({ scope, attempt }: { scope: string; attempt: number }) {
  const host = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<ConnectionState>("connecting");

  useEffect(() => {
    if (!host.current) return;
    setState("connecting");
    const terminal = new XTerm({
      fontFamily: '"IBM Plex Mono", ui-monospace, monospace',
      fontSize: 13,
      lineHeight: 1.25,
      cursorBlink: true,
      scrollback: 5000,
      theme: terminalTheme(),
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    fit.fit();

    const protocol = window.location.protocol === "https:" ? "wss" : "ws";
    const socket = new WebSocket(`${protocol}://${window.location.host}/api/v1/terminal/ws?scope=${encodeURIComponent(scope)}`);
    socket.binaryType = "arraybuffer";
    const encoder = new TextEncoder();
    const sendSize = () => {
      if (socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ type: "resize", cols: terminal.cols, rows: terminal.rows }));
      }
    };

    socket.onopen = () => {
      setState("open");
      sendSize();
      terminal.focus();
    };
    socket.onmessage = (event) => terminal.write(new Uint8Array(event.data as ArrayBuffer));
    socket.onclose = () => {
      setState("closed");
      terminal.write(`\r\n\x1b[2m${t("Oturum sona erdi.")}\x1b[0m\r\n`);
    };

    const input = terminal.onData((data) => socket.readyState === WebSocket.OPEN && socket.send(encoder.encode(data)));
    const resized = terminal.onResize(sendSize);
    // Pencere ya da yerleşim değişince uçbirim alanına yeniden sığdırılır.
    const observer = new ResizeObserver(() => fit.fit());
    observer.observe(host.current);

    return () => {
      observer.disconnect();
      input.dispose();
      resized.dispose();
      socket.close();
      terminal.dispose();
    };
  }, [scope, attempt]);

  const view = stateView[state];
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-2">
      <Status tone={view.tone}>{t(view.label)}</Status>
      <div className="min-h-0 flex-1 overflow-hidden rounded-lg border border-line bg-sunken p-2">
        <div ref={host} className="h-full" />
      </div>
    </div>
  );
}

export function TerminalPage() {
  const roots = useQuery({ queryKey: ["file-roots"], queryFn: () => api.get<Root[]>("/files/roots") });
  // Başka sayfalar terminali belirli bir hedefle açabilir (ör. `?kapsam=container:web`).
  const [searchParams] = useSearchParams();
  const [requested, setRequested] = useState(searchParams.get("kapsam") ?? "");
  const [attempt, setAttempt] = useState(0);

  if (roots.isPending) return <LoadingBlock />;
  if (roots.isError) return <ErrorBlock message={errorMessage(roots.error)} />;

  // Root en başta gelsin; hesaplar onun ardından listelenir.
  const targets = [...roots.data.filter((root) => root.id === "root"), ...roots.data.filter((root) => root.id !== "root")];
  // Yönetici olmayan hesapta "root" yoktur: ilk hedef (kendi hesabı) seçilir. İstenen hedef listede yoksa
  // (konteyner dışında) yine ilk hedefe dönülür; sunucu yetkisiz hedefi zaten reddeder.
  const scope = requested.startsWith("container:") || targets.some((target) => target.id === requested) ? requested : (targets[0]?.id ?? "root");
  const setScope = setRequested;

  return (
    <div className="flex h-[calc(100dvh-6.5rem)] flex-col">
      <PageHeader
        title={t("Terminal")}
        description={t("Sunucuda komut çalıştırın. Sekmeyi kapattığınızda oturum ve içinde çalışan komutlar sonlanır.")}
        actions={
          <>
            <select
              aria-label={t("Kimin adına")}
              value={scope}
              onChange={(event) => setScope(event.target.value)}
              className="h-9 rounded-md border border-line-strong bg-surface px-3"
            >
              {scope.startsWith("container:") && <option value={scope}>{t("Konteyner: {name}", { name: scope.slice("container:".length) })}</option>}
              {targets.map((target) => (
                <option key={target.id} value={target.id}>
                  {target.id === "root" ? "root" : target.label}
                </option>
              ))}
            </select>
            <Button icon={<RotateCw className="size-4" />} onClick={() => setAttempt((value) => value + 1)}>
              {t("Yeniden bağlan")}
            </Button>
          </>
        }
      />
      <Session scope={scope} attempt={attempt} />
    </div>
  );
}
